//! `impact`, `dossier` and `dead_code` driven end to end against a scripted gateway.
//!
//! `impact::analyze` and `dead_code::find_dead_code` only ever speak LSP to the gateway, so
//! [`ScriptedGateway`] from the shared testkit is enough for them. `dossier::diagnose` also runs
//! a remote command (`ExecRequest`/`ExecChunk`/`ExecExit`), which the shared testkit does not
//! answer; [`ExecGateway`] below is a small local gateway, built the same way the CLI's own
//! integration tests build theirs, that answers both the LSP methods and the exec run from one
//! script.

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::dead_code::{self, DeadCodeReport, DeadItem};
use prod_code_mcp::dossier::{self, DossierReport, FailureDossier, FailureSite, Suspect};
use prod_code_mcp::impact::{self, ImpactReport, Symbol};
use prod_code_protocol::{
    ExecChunk, ExecExit, ExecRequest, HandshakeResponse, PROTOCOL_VERSION, ProdCodeCodec,
    SyncProbeResponse, SyncResponse, WireMessage,
};
use prod_code_testkit::{Answer, ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// Answers an `ExecRequest` with (stdout, stderr, exit code).
type ExecAnswer = Arc<dyn Fn(&ExecRequest) -> (Vec<u8>, Vec<u8>, Option<i32>) + Send + Sync>;

/// A gateway that answers LSP methods from a script, same as [`ScriptedGateway`], and also
/// answers one remote command from a second script.
struct ExecGateway {
    addr: SocketAddr,
}

impl ExecGateway {
    async fn start(lsp: Answer, exec: ExecAnswer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let lsp = Arc::clone(&lsp);
                let exec = Arc::clone(&exec);
                tokio::spawn(async move {
                    let _ = Self::serve(socket, lsp, exec).await;
                });
            }
        });
        Self { addr }
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }

    async fn serve(socket: TcpStream, lsp: Answer, exec: ExecAnswer) -> anyhow::Result<()> {
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        while let Some(message) = framed.next().await {
            match message? {
                WireMessage::SyncProbeRequest(req) => {
                    framed
                        .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                            server_workspace_root: req.client_workspace_root.clone(),
                            seeded: false,
                            files_deleted: 0,
                            missing: Vec::new(),
                        }))
                        .await?;
                }
                WireMessage::SyncRequest(req) => {
                    framed
                        .send(WireMessage::SyncResponse(SyncResponse {
                            server_workspace_root: req.client_workspace_root.clone(),
                            files_updated: 0,
                            files_deleted: 0,
                            bytes_transferred: 0,
                            duration_ms: 0,
                            workspace_was_fresh: false,
                        }))
                        .await?;
                }
                WireMessage::HandshakeRequest(req) => {
                    framed
                        .send(WireMessage::HandshakeResponse(HandshakeResponse {
                            protocol_version: PROTOCOL_VERSION,
                            server_pid: std::process::id(),
                            session_id: 1,
                            server_workspace_root: req.client_workspace_root.clone(),
                            detected_engine: "rust".to_string(),
                        }))
                        .await?;
                }
                WireMessage::ExecRequest(req) => {
                    let (stdout, stderr, exit_code) = exec(&req);
                    if !stdout.is_empty() {
                        framed
                            .send(WireMessage::ExecChunk(ExecChunk {
                                stderr: false,
                                data: Some(stdout),
                            }))
                            .await?;
                    }
                    if !stderr.is_empty() {
                        framed
                            .send(WireMessage::ExecChunk(ExecChunk {
                                stderr: true,
                                data: Some(stderr),
                            }))
                            .await?;
                    }
                    framed
                        .send(WireMessage::ExecExit(ExecExit {
                            exit_code,
                            duration_ms: 1,
                            server_workspace_root: req.client_workspace_root.clone(),
                            timed_out: false,
                            error: None,
                            usage: None,
                            platform: None,
                        }))
                        .await?;
                }
                WireMessage::LspPayload(json) => {
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
                        continue;
                    };
                    let Some(id) = value.get("id").cloned() else {
                        continue;
                    };
                    let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
                    let params = value
                        .get("params")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let result = if method == "initialize" {
                        serde_json::json!({ "capabilities": { "hoverProvider": true } })
                    } else {
                        lsp(method, &params)
                    };
                    let response =
                        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
                    framed
                        .send(WireMessage::LspPayload(response.to_string()))
                        .await?;
                }
                WireMessage::Disconnect { .. } => break,
                _ => {}
            }
        }
        Ok(())
    }
}

fn no_lsp() -> Answer {
    Arc::new(|_method, _params| serde_json::Value::Null)
}

// ---------------------------------------------------------------------------------------------
// impact::changed_lines
// ---------------------------------------------------------------------------------------------

/// A change to a tracked file is reported as the 1-based line range the diff touched.
#[tokio::test]
async fn changed_lines_maps_a_tracked_diff_to_its_line_range() {
    let ws = Workspace::new(&[("src/lib.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")]);
    ws.write("src/lib.rs", "fn a() {}\nfn bb() {}\nfn c() {}\n");

    let ranges = impact::changed_lines(&ws.root(), None).expect("diff parses");

    assert_eq!(ranges.get("src/lib.rs"), Some(&vec![(2, 2)]));
}

/// A file that was never committed counts as fully changed: every line, start to end.
#[tokio::test]
async fn changed_lines_treats_an_untracked_file_as_fully_changed() {
    let ws = Workspace::new(&[("src/lib.rs", "fn a() {}\n")]);
    ws.write("src/new.rs", "fn z() {}\n");

    let ranges = impact::changed_lines(&ws.root(), None).expect("diff parses");

    assert_eq!(ranges.get("src/new.rs"), Some(&vec![(1, u32::MAX)]));
    assert!(
        !ranges.contains_key("src/lib.rs"),
        "the untouched file is not reported: {ranges:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// impact::ImpactReport::render / test_command
// ---------------------------------------------------------------------------------------------

fn sym(name: &str, file: &str, line: u32, col: u32) -> Symbol {
    Symbol {
        name: name.to_string(),
        file: file.to_string(),
        line,
        col,
    }
}

/// The report names every changed function, every caller it reaches, every test that reaches
/// it, and the command that runs only those tests, quoting an argument that needs it.
#[tokio::test]
async fn impact_report_render_lists_every_section_and_quotes_the_run_command() {
    let report = ImpactReport {
        language: "rust".to_string(),
        base: "HEAD".to_string(),
        changed_files: vec!["src/lib.rs".to_string()],
        changed: vec![sym("helper", "src/lib.rs", 1, 8)],
        callers: vec![sym("wrapper", "src/lib.rs", 5, 8)],
        tests: vec![sym("it works", "src/lib.rs", 12, 8)],
        test_command: Some(vec![
            "cargo".to_string(),
            "test".to_string(),
            "it works".to_string(),
        ]),
        unattributed_files: vec!["Cargo.toml".to_string()],
        index: None,
        reaches: vec![],
    };

    let text = report.render();

    assert!(text.contains("changed functions:"));
    assert!(text.contains("helper  src/lib.rs:1:8"));
    assert!(text.contains("reached callers:"));
    assert!(text.contains("wrapper  src/lib.rs:5:8"));
    assert!(text.contains("affected tests:"));
    assert!(text.contains("it works  src/lib.rs:12:8"));
    assert!(text.contains("changes outside functions"));
    assert!(text.contains("Cargo.toml"));
    // The test name has a space, so the run command quotes it.
    assert!(text.contains("run: cargo test 'it works'"));
}

/// When a function changed but nothing reaches it through the call hierarchy, the report says
/// so instead of silently omitting the section.
#[tokio::test]
async fn impact_report_render_says_when_no_test_reaches_the_change() {
    let report = ImpactReport {
        language: "rust".to_string(),
        base: "HEAD".to_string(),
        changed_files: vec!["src/lib.rs".to_string()],
        changed: vec![sym("helper", "src/lib.rs", 1, 8)],
        callers: vec![],
        tests: vec![],
        test_command: None,
        unattributed_files: vec![],
        index: None,
        reaches: vec![],
    };

    let text = report.render();

    assert!(text.contains("affected tests: none reach the changed functions"));
    assert!(!text.contains("run:"));
}

/// Swift finds callers only through the index a build leaves: the report says which build ran,
/// and when it failed, no test reaching the change means unknown, not none (#166).
#[tokio::test]
async fn impact_report_says_when_the_index_could_not_be_built() {
    let build = |ok| impact::IndexBuild {
        command: "swift build --build-tests".to_string(),
        ok,
        duration_ms: 900,
    };
    let report = |ok| ImpactReport {
        language: "swift".to_string(),
        base: "HEAD".to_string(),
        changed_files: vec!["Sources/MathKit/Math.swift".to_string()],
        changed: vec![sym("plus(_:_:)", "Sources/MathKit/Math.swift", 1, 13)],
        callers: vec![],
        tests: vec![],
        test_command: None,
        unattributed_files: vec![],
        index: Some(build(ok)),
        reaches: vec![],
    };

    let built = report(true).render();
    assert!(
        built.contains("index: `swift build --build-tests` (0.9s)"),
        "{built}"
    );
    assert!(built.contains("affected tests: none reach the changed functions"));

    let failed = report(false).render();
    assert!(
        failed.contains("failed, so the analyzer has no index"),
        "{failed}"
    );
    assert!(
        failed.contains("1 changed function(s), callers and tests unknown)"),
        "{failed}"
    );
    assert!(built.contains("0 caller(s), 0 test(s))"), "{built}");
    assert!(
        failed.contains("affected tests: unknown (no index)"),
        "{failed}"
    );
    assert!(!failed.contains("none reach"), "{failed}");
}

/// `test_command` for Swift strips the `()` a call-hierarchy name carries.
#[tokio::test]
async fn test_command_for_swift_strips_the_call_suffix() {
    let tools = prod_code_mcp::verify::ProjectTools::default();
    let tests = [sym("MathTests.testAdds()", "x", 1, 1)];

    let cmd = impact::test_command("swift", &tools, &tests).expect("swift selects tests");

    assert_eq!(cmd, vec!["swift", "test", "--filter", "MathTests.testAdds"]);
}

// ---------------------------------------------------------------------------------------------
// impact::analyze
// ---------------------------------------------------------------------------------------------

const IMPACT_LIB: &str = "pub fn helper(x: i32) -> i32 {\n    x + 1\n}\n\npub fn wrapper() -> i32 {\n    helper(41)\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_calls_wrapper() {\n        assert_eq!(wrapper(), 42);\n    }\n}\n";

/// A one-line change to `helper`'s body is attributed to `helper`; the call hierarchy is
/// walked up to `wrapper` (a plain caller) and then to the test that reaches it, and the depth
/// limit stops the walk there. The rust test command names exactly that test.
#[tokio::test]
async fn analyze_walks_the_call_hierarchy_from_a_changed_function_to_the_test_that_reaches_it() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", IMPACT_LIB)]);
    let root = ws.root();
    ws.write(
        "src/lib.rs",
        "pub fn helper(x: i32) -> i32 {\n    x + 2\n}\n\npub fn wrapper() -> i32 {\n    helper(41)\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_calls_wrapper() {\n        assert_eq!(wrapper(), 42);\n    }\n}\n",
    );
    let lib = ws.path("src/lib.rs");
    let uri = format!("file://{}", lib.display());

    let remote = ScriptedGateway::start_arc(Arc::new(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("helper", 12, 1, 3, 8),
            answers::document_symbol("wrapper", 12, 5, 7, 8),
        ]),
        "textDocument/prepareCallHierarchy" => {
            let line = params
                .pointer("/position/line")
                .and_then(|l| l.as_u64())
                .unwrap_or(u64::MAX);
            let id = if line == 0 {
                "helper"
            } else if line == 4 {
                "wrapper"
            } else {
                return serde_json::Value::Array(vec![]);
            };
            serde_json::json!([{ "name": id, "uri": uri, "_id": id }])
        }
        "callHierarchy/incomingCalls" => {
            let id = params
                .pointer("/item/_id")
                .and_then(|i| i.as_str())
                .unwrap_or("");
            match id {
                "helper" => serde_json::json!([{
                    "from": {
                        "name": "wrapper",
                        "uri": uri,
                        "selectionRange": { "start": { "line": 4, "character": 7 } }
                    }
                }]),
                "wrapper" => serde_json::json!([{
                    "from": {
                        "name": "it_calls_wrapper",
                        "uri": uri,
                        "selectionRange": { "start": { "line": 11, "character": 7 } }
                    },
                    "isTest": true
                }]),
                _ => serde_json::json!([]),
            }
        }
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();

    let report = impact::analyze(remote, &root, None, 2)
        .await
        .expect("analysis runs");

    assert_eq!(report.language, "rust");
    assert_eq!(report.changed, vec![sym("helper", "src/lib.rs", 1, 8)]);
    assert_eq!(report.callers, vec![sym("wrapper", "src/lib.rs", 5, 8)]);
    assert_eq!(
        report.tests,
        vec![sym("it_calls_wrapper", "src/lib.rs", 12, 8)]
    );
    assert!(report.unattributed_files.is_empty());
    assert_eq!(
        report.test_command,
        Some(vec![
            "cargo".to_string(),
            "test".to_string(),
            "--workspace".to_string(),
            "--".to_string(),
            "it_calls_wrapper".to_string()
        ])
    );
    // The test is two calls from the changed function: `helper` <- `wrapper` <- the test.
    assert_eq!(
        report.reaches,
        vec![impact::Reach {
            test: sym("it_calls_wrapper", "src/lib.rs", 12, 8),
            changed: sym("helper", "src/lib.rs", 1, 8),
            hops: 2,
        }]
    );
    assert_eq!(
        impact::suspects_for(&report.reaches, "tests::it_calls_wrapper"),
        vec![(sym("helper", "src/lib.rs", 1, 8), 2)]
    );
    assert!(impact::suspects_for(&report.reaches, "tests::other").is_empty());
}

/// Two changed functions reach one test: the nearer is listed first, each once, and a
/// qualified or call-style test name matches the bare one.
#[tokio::test]
async fn suspects_are_the_nearest_changed_functions_first() {
    let reach = |test: &str, changed: &str, hops| impact::Reach {
        test: sym(test, "src/lib.rs", 20, 8),
        changed: sym(changed, "src/math.rs", 1, 8),
        hops,
    };
    let reaches = [
        reach("doubles", "add", 3),
        reach("doubles", "scale", 1),
        reach("doubles", "add", 2),
        reach("triples", "mul", 1),
    ];
    let names: Vec<(String, usize)> = impact::suspects_for(&reaches, "tests::doubles")
        .into_iter()
        .map(|(s, h)| (s.name, h))
        .collect();
    assert_eq!(names, [("scale".to_string(), 1), ("add".to_string(), 2)]);
    assert_eq!(
        impact::suspects_for(&[reach("testAdds()", "add", 1)], "MathTests.testAdds()").len(),
        1
    );
}

/// A change to a file with no functions in it (a manifest, a README) is reported as
/// "unattributed" rather than silently dropped or crashing the symbol walk.
#[tokio::test]
async fn analyze_reports_a_change_outside_any_function_as_unattributed() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub fn helper() {}\n"),
        ("README.md", "hello\n"),
    ]);
    let root = ws.root();
    ws.write("README.md", "hello\nworld\n");

    let remote = ScriptedGateway::start_arc(no_lsp()).await.addr();

    let report = impact::analyze(remote, &root, None, 2)
        .await
        .expect("analysis runs");

    assert!(report.changed.is_empty());
    assert!(report.callers.is_empty());
    assert!(report.tests.is_empty());
    assert_eq!(report.test_command, None);
    assert_eq!(report.unattributed_files, vec!["README.md".to_string()]);
}

// ---------------------------------------------------------------------------------------------
// dead_code::find_dead_code
// ---------------------------------------------------------------------------------------------

const DEAD_LIB: &str = "pub fn used_fn() {}\n\nfn private_unreferenced() {}\n\npub fn public_unreferenced() {}\n\npub struct Shape;\n\nimpl Shape {\n    fn plain_method(&self) {}\n}\n\npub trait Drawable {\n    fn draw(&self);\n}\n\nimpl Drawable for Shape {\n    fn draw(&self) {}\n}\n\npub fn new() {}\n\nfn test_something() {}\n\nfn not_named_like_test_but_in_tests_mod() {}\n";

/// The document symbols `collect()` walks for [`DEAD_LIB`]: one candidate of each kind this
/// scan sorts differently, plus the compiler-reserved names and the tests-module item it must
/// never even query.
fn dead_symbols() -> serde_json::Value {
    serde_json::json!([
        { "name": "used_fn", "kind": 12,
          "selectionRange": { "start": { "line": 0, "character": 7 } } },
        { "name": "private_unreferenced", "kind": 12,
          "selectionRange": { "start": { "line": 2, "character": 3 } } },
        { "name": "public_unreferenced", "kind": 12,
          "selectionRange": { "start": { "line": 4, "character": 7 } } },
        { "name": "plain_method", "kind": 6,
          "selectionRange": { "start": { "line": 9, "character": 7 } } },
        { "name": "draw", "kind": 6, "containerName": "impl Drawable for Shape",
          "selectionRange": { "start": { "line": 17, "character": 7 } } },
        { "name": "new", "kind": 12,
          "selectionRange": { "start": { "line": 20, "character": 7 } } },
        { "name": "test_something", "kind": 12,
          "selectionRange": { "start": { "line": 22, "character": 3 } } },
        { "name": "not_named_like_test_but_in_tests_mod", "kind": 12, "containerName": "tests",
          "selectionRange": { "start": { "line": 24, "character": 3 } } },
    ])
}

/// A used symbol is never listed; an unreferenced private one is; a public one is folded into
/// `exported_unreferenced` unless the caller asks to see it; a plain method's zero references
/// puts it in `dead`, a trait method's puts it in the "maybe reached through a trait" bucket;
/// and the compiler-reserved names and anything inside a `tests` module are never even checked.
#[tokio::test]
async fn find_dead_code_sorts_symbols_into_the_right_bucket() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", DEAD_LIB)]);
    let root = ws.root();
    let path = ws.path("src/lib.rs");

    let remote = ScriptedGateway::start_arc(Arc::new(move |method, params| match method {
        "textDocument/documentSymbol" => dead_symbols(),
        "textDocument/references" => {
            let line = params
                .pointer("/position/line")
                .and_then(|l| l.as_u64())
                .unwrap_or(u64::MAX);
            if line == 0 {
                answers::locations(&path, &[(1, 8)]) // used_fn: referenced once
            } else {
                answers::locations(&path, &[]) // everyone else: zero references
            }
        }
        _ => serde_json::Value::Null,
    }))
    .await;
    let addr = remote.addr();

    let report = dead_code::find_dead_code(addr, &root, false, 100)
        .await
        .expect("scan runs");

    assert_eq!(report.language, "rust");
    assert_eq!(report.files_scanned, 1);
    // used_fn, private_unreferenced, public_unreferenced, plain_method, draw: `new`,
    // `test_something` and the tests-module item are never even queried.
    assert_eq!(report.symbols_checked, 5);
    assert_eq!(
        remote.calls(),
        6,
        "one documentSymbol call plus one references call per checked candidate"
    );

    let names: Vec<&str> = report.dead.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"private_unreferenced"));
    assert!(names.contains(&"plain_method"));
    assert!(
        !names.contains(&"used_fn"),
        "a referenced symbol is not dead: {names:?}"
    );
    assert!(
        !names.contains(&"public_unreferenced"),
        "an exported symbol is folded into the count, not listed, by default: {names:?}"
    );
    assert_eq!(report.exported_unreferenced, 1);

    let method_names: Vec<&str> = report
        .methods_unreferenced
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(method_names, vec!["draw"]);
    assert_eq!(report.methods_unreferenced[0].kind, "trait-method");

    // `new`, `test_something` and the module-scoped item never turn into candidates at all.
    assert!(!names.contains(&"new"));
    assert!(!names.contains(&"test_something"));
    assert!(!names.contains(&"not_named_like_test_but_in_tests_mod"));

    // Asking to see exported symbols moves the public one into `dead`, flagged.
    let report = dead_code::find_dead_code(addr, &root, true, 100)
        .await
        .expect("scan runs");
    let public = report
        .dead
        .iter()
        .find(|d| d.name == "public_unreferenced")
        .expect("now listed");
    assert!(public.exported);
    assert_eq!(report.exported_unreferenced, 0);
}

/// A scan capped below the number of source files stops early and says so.
#[tokio::test]
async fn find_dead_code_stops_at_the_file_limit_and_marks_the_scan_truncated() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "fn a() {}\n"),
        ("src/other.rs", "fn b() {}\n"),
    ]);
    let root = ws.root();

    let remote = ScriptedGateway::start_arc(Arc::new(|method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();

    let report = dead_code::find_dead_code(remote, &root, false, 1)
        .await
        .expect("scan runs");

    assert_eq!(report.files_scanned, 1);
    assert!(report.truncated);
}

/// [`DeadCodeReport::render`] names every dead symbol, flags the exported ones, lists the
/// methods that might still be reached through a trait, and reports the truncation.
#[tokio::test]
async fn dead_code_report_render_covers_every_section() {
    let report = DeadCodeReport {
        language: "rust".to_string(),
        files_scanned: 3,
        symbols_checked: 9,
        dead: vec![DeadItem {
            name: "orphan".to_string(),
            kind: "function".to_string(),
            file: "src/lib.rs".to_string(),
            line: 4,
            col: 1,
            exported: true,
        }],
        methods_unreferenced: vec![DeadItem {
            name: "draw".to_string(),
            kind: "trait-method".to_string(),
            file: "src/lib.rs".to_string(),
            line: 9,
            col: 5,
            exported: false,
        }],
        exported_unreferenced: 2,
        truncated: true,
    };

    let text = report.render();

    assert!(text.contains("3 file(s), 9 symbol(s) checked, 1 unreferenced"));
    assert!(text.contains("function orphan (exported)  src/lib.rs:4:1"));
    assert!(text.contains("methods without direct references (1"));
    assert!(text.contains("draw  src/lib.rs:9:5"));
    assert!(text.contains("2 exported symbol(s) are unreferenced"));
    assert!(text.contains("scan truncated by the file limit"));
}

// ---------------------------------------------------------------------------------------------
// dossier::locations_in / locations_in_with_hint
// ---------------------------------------------------------------------------------------------

/// Locations outside the checkout, malformed, or with a zero line are skipped; a bare file
/// name that exists in two directories is resolved to the one the failing test's name hints
/// at.
#[tokio::test]
async fn locations_in_with_hint_skips_the_unusable_and_resolves_the_ambiguous() {
    let ws = Workspace::new(&[
        ("moda/util_test.go", "package moda\n"),
        ("modb/util_test.go", "package modb\n"),
    ]);
    let root = ws.root();
    let root_str = root.display().to_string();

    let text = format!(
        "{root_str}/moda/util_test.go:0\n\
../outside.rs:3\n\
vendor/.cargo/pkg.rs:5\n\
lib/rustlib/src/rust/foo.rs:9\n\
http.txt:5\n\
noext:12\n\
util_test.go:12: failure here\n"
    );

    let locs = dossier::locations_in_with_hint(&root, &text, "modb.TestFoo");

    assert_eq!(locs, vec![("modb/util_test.go".to_string(), 12)]);
}

/// With no hint, an unambiguous bare file name still resolves; `locations_in` is
/// `locations_in_with_hint` with an empty hint.
#[tokio::test]
async fn locations_in_resolves_an_unambiguous_bare_file_name() {
    let ws = Workspace::new(&[("pkg/thing.go", "package pkg\n")]);
    let root = ws.root();

    let locs = dossier::locations_in(&root, "panic: boom\n\tthing.go:7 +0x1\n");

    assert_eq!(locs, vec![("pkg/thing.go".to_string(), 7)]);
}

// ---------------------------------------------------------------------------------------------
// dossier::diagnose
// ---------------------------------------------------------------------------------------------

const DOSSIER_LIB: &str = "pub fn helper(x: i32) -> i32 {\n    x + 1\n}\n\npub fn caller_of_helper() -> i32 {\n    helper(41)\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_fails() {\n        assert_eq!(2 + 2, 5);\n    }\n}\n";

const CARGO_TEST_FAILURE_STDOUT: &str = "running 1 test\ntest tests::it_fails ... FAILED\n\nfailures:\n\n---- tests::it_fails stdout ----\n\nthread 'tests::it_fails' panicked at src/lib.rs:13:9:\nassertion `left == right` failed\n  left: 4\n right: 5\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\nfailures:\n    tests::it_fails\n\ntest result: FAILED. 0 passed; 1 failed; 0 filtered out; finished in 0.00s\n";

/// A failing test gets a dossier: the failure site's enclosing function, its caller from the
/// call hierarchy, and the working-tree diff of the file it lives in — assembled from the
/// `cargo test` output and a handful of LSP queries, none of which needed a real analyzer.
#[tokio::test]
async fn diagnose_builds_a_dossier_with_the_site_its_caller_and_the_working_tree_diff() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", DOSSIER_LIB)]);
    let root = ws.root();
    // A change after the failing line: the diff is real but does not move line 13.
    ws.write("src/lib.rs", &format!("{DOSSIER_LIB}// trailing note\n"));
    let lib = ws.path("src/lib.rs");
    let lsp_uri = format!("file://{}", lib.display());

    let lsp: Answer = Arc::new(move |method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            { "name": "it_fails", "kind": 12,
              "range": { "start": { "line": 11 }, "end": { "line": 13 } },
              "selectionRange": { "start": { "line": 11, "character": 7 } } }
        ]),
        "textDocument/prepareCallHierarchy" => {
            serde_json::json!([{ "name": "it_fails", "uri": lsp_uri, "_id": "it_fails" }])
        }
        "callHierarchy/incomingCalls" => serde_json::json!([{
            "from": { "name": "test_runner", "uri": lsp_uri }
        }]),
        _ => serde_json::Value::Null,
    });
    let stdout = CARGO_TEST_FAILURE_STDOUT.as_bytes().to_vec();
    let exec: ExecAnswer = Arc::new(move |_req| (stdout.clone(), Vec::new(), Some(101)));

    let remote = ExecGateway::start(lsp, exec).await.addr();

    let report = dossier::diagnose(remote, &root, None, 60)
        .await
        .expect("diagnose runs");

    assert_eq!(report.tests_passed, 0);
    assert_eq!(report.tests_failed, 1);
    assert_eq!(report.dossiers.len(), 1);
    let dossier = &report.dossiers[0];
    assert_eq!(dossier.test, "tests::it_fails");
    assert!(dossier.output.contains("assertion `left == right` failed"));
    assert_eq!(dossier.sites.len(), 1);
    let site = &dossier.sites[0];
    assert_eq!(site.file, "src/lib.rs");
    assert_eq!(site.line, 13);
    assert_eq!(site.function.as_deref(), Some("it_fails"));
    assert_eq!(site.callers, vec!["test_runner".to_string()]);
    assert!(site.snippet.contains(">13"));
    let diff = site.diff.as_ref().expect("the file changed on disk");
    assert!(diff.starts_with("@@"));
    assert!(diff.contains("+// trailing note"));

    let rendered = report.render();
    assert!(rendered.contains("0 passed, 1 failed"));
    assert!(rendered.contains("=== tests::it_fails ==="));
    assert!(rendered.contains("in it_fails"));
    assert!(rendered.contains("callers: test_runner"));
    assert!(rendered.contains("changed in the working tree:"));
}

/// When the run does not produce a parsed test failure at all but the compiler reported an
/// error, the dossier carries that error instead of an empty failure list, and the render falls
/// back to the raw output tail.
#[tokio::test]
async fn diagnose_surfaces_a_build_error_when_there_is_no_test_failure_to_pin_it_to() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub fn f() {}\n"),
    ]);
    let root = ws.root();

    let exec: ExecAnswer = Arc::new(|_req| {
        (
            Vec::new(),
            b"error[E0308]: mismatched types\n  --> src/lib.rs:3:5\n".to_vec(),
            Some(101),
        )
    });
    let remote = ExecGateway::start(no_lsp(), exec).await.addr();

    let report = dossier::diagnose(remote, &root, None, 60)
        .await
        .expect("diagnose runs");

    assert!(report.dossiers.is_empty());
    assert_eq!(report.tests_passed, 0);
    assert_eq!(report.tests_failed, 0);
    assert_eq!(
        report.build_errors,
        vec!["mismatched types (src/lib.rs:3)".to_string()]
    );

    let text = report.render();
    assert!(text.contains("build errors:"));
    assert!(text.contains("mismatched types"));
    assert!(text.contains("--- output tail ---"));
}

/// A run with nothing wrong renders as "no failures" rather than an empty, ambiguous report.
#[tokio::test]
async fn dossier_report_render_says_no_failures_when_the_run_was_clean() {
    let report = DossierReport {
        command: vec!["cargo".to_string(), "test".to_string()],
        changed_files: vec![],
        tests_passed: 3,
        tests_failed: 0,
        dossiers: vec![],
        build_errors: vec![],
        suggested_fixes: vec![],
        tail: String::new(),
    };

    assert!(report.render().contains("no failures"));

    // Tests that did not build: the compiler's own fixes for the errors are suggested.
    let broken = DossierReport {
        build_errors: vec!["mismatched types (src/lib.rs:4)".to_string()],
        suggested_fixes: vec!["src/lib.rs:4: mismatched types".to_string()],
        ..report
    };
    let text = broken.render();
    assert!(text.contains("suggested fixes"), "{text}");
    assert!(text.contains("  src/lib.rs:4: mismatched types"), "{text}");
    assert!(text.contains("prod-code check --fix"), "{text}");
}

/// [`FailureSite`] and [`FailureDossier`] are plain data the tools above assemble; `render`
/// reflects exactly the fields that were set, including a caller list and a captured diff.
#[tokio::test]
async fn dossier_report_render_includes_the_caller_list_and_the_diff() {
    let report = DossierReport {
        command: vec!["cargo".to_string(), "test".to_string()],
        changed_files: vec!["src/lib.rs".to_string()],
        tests_passed: 0,
        tests_failed: 1,
        dossiers: vec![FailureDossier {
            test: "it_fails".to_string(),
            output: "assertion failed\n".to_string(),
            sites: vec![FailureSite {
                file: "src/lib.rs".to_string(),
                line: 3,
                snippet: " 2 | fn f() {}\n>3 |   boom();\n".to_string(),
                function: Some("f".to_string()),
                callers: vec!["main".to_string()],
                diff: Some("@@ -1,1 +1,1 @@\n-old\n+new\n".to_string()),
            }],
            suspects: vec![Suspect {
                function: "add".to_string(),
                file: "src/math.rs".to_string(),
                line: 1,
                hops: 2,
                diff: Some("@@ -2 +2 @@\n-    a + b\n+    a + b + 1\n".to_string()),
            }],
        }],
        build_errors: vec![],
        suggested_fixes: vec![],
        tail: String::new(),
    };

    let text = report.render();

    assert!(text.contains("changed in the working tree: src/lib.rs"));
    assert!(text.contains("--- src/lib.rs:3  in f"));
    assert!(text.contains("callers: main"));
    assert!(text.contains("changed in the working tree:\n@@ -1,1 +1,1 @@"));
    assert!(text.contains(
        "suspects (changed functions that reach this test, nearest first):\n  • add  src/math.rs:1  (2 calls away)"
    ));
    assert!(text.contains("changed in src/math.rs:\n@@ -2 +2 @@"));
}

/// An unreachable gateway is reported as a run that failed to start, not a panic or a hang.
#[tokio::test]
async fn diagnose_fails_cleanly_when_the_gateway_is_unreachable() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub fn f() {}\n"),
    ]);
    let root = ws.root();
    let unreachable: SocketAddr = "127.0.0.1:1".parse().unwrap();

    let err = dossier::diagnose(unreachable, &root, None, 5)
        .await
        .expect_err("connecting to a closed port fails");

    assert!(
        format!("{err:#}").contains("failed to connect to remote gateway"),
        "{err:#}"
    );
}
