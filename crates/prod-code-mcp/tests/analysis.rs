/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

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
use prod_code_mcp::impact::{self, CiRun, Gap, ImpactReport, Symbol};
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
                            stale_paths: Vec::new(),
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
                            stale_paths: Vec::new(),
                            engine_age_ms: None,
                            index_gated: false,
                            capabilities: None,
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

/// The text of a tool's answer.
fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            prod_code_mcp::protocol::McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A Swift reference search that finds nothing in a package never built on its node builds the
/// package's index once and asks again: sourcekit-lsp finds uses in other files only through a
/// build's index (#358). A second search does not build again.
#[tokio::test]
async fn a_swift_reference_search_builds_the_packages_index_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let ws = Workspace::new(&[
        ("Package.swift", "// swift-tools-version:5.9\n"),
        (
            "Sources/App/Session.swift",
            "public final class Session {}\n",
        ),
        ("Sources/App/Use.swift", "let session = Session()\n"),
    ]);
    let use_file = ws.path("Sources/App/Use.swift");
    let builds = Arc::new(AtomicUsize::new(0));
    let built = Arc::clone(&builds);
    let counted = Arc::clone(&builds);
    let lsp: Answer = Arc::new(move |method, _| match method {
        "textDocument/references" if built.load(Ordering::SeqCst) > 0 => {
            answers::locations(&use_file, &[(1, 15)])
        }
        "textDocument/references" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    });
    let exec: ExecAnswer = Arc::new(move |req| {
        if req.command == ["swift", "build", "--build-tests"] {
            counted.fetch_add(1, Ordering::SeqCst);
        }
        (b"Build complete!\n".to_vec(), Vec::new(), Some(0))
    });
    let gateway = ExecGateway::start(lsp, exec).await;
    let root = ws.root();
    let ask = || {
        prod_code_mcp::tools::execute_tool(
            gateway.addr(),
            &root,
            "code_references",
            serde_json::json!({ "path": "Sources/App/Session.swift", "line": 1, "character": 20 }),
        )
    };

    let first = text_of(&ask().await.expect("the search runs"));
    assert!(first.contains("built the package first"), "{first}");
    assert!(first.contains("Sources/App/Use.swift"), "{first}");
    let _ = ask().await.expect("the search runs again");
    assert_eq!(builds.load(Ordering::SeqCst), 1, "one build per process");
}

/// A package whose build fails says why, instead of an empty answer that reads like "nothing
/// uses this" (#358): its error line, not the tail of a message spread over several lines.
#[tokio::test]
async fn a_swift_reference_search_says_why_the_index_could_not_be_built() {
    let ws = Workspace::new(&[
        ("Package.swift", "// swift-tools-version:5.9\n"),
        (
            "Sources/App/Session.swift",
            "public final class Session {}\n",
        ),
    ]);
    let lsp: Answer = Arc::new(|method, _| match method {
        "textDocument/references" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    });
    let exec: ExecAnswer = Arc::new(|_| {
        (
            Vec::new(),
            b"error: GitShellError(result: <ProcessResult: exit: terminated(code: 128), output:\n \n>)\n"
                .to_vec(),
            Some(1),
        )
    });
    let gateway = ExecGateway::start(lsp, exec).await;
    let text = text_of(
        &prod_code_mcp::tools::execute_tool(
            gateway.addr(),
            &ws.root(),
            "code_references",
            serde_json::json!({ "path": "Sources/App/Session.swift", "line": 1, "character": 20 }),
        )
        .await
        .expect("the search runs"),
    );
    assert!(
        text.contains("failed (exit 1: error: GitShellError"),
        "the build's error line: {text}"
    );
    assert!(text.contains("No references found."), "{text}");
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
        incomplete: vec![],
        signature_warnings: vec![],
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
        incomplete: vec![],
        signature_warnings: vec![],
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
        incomplete: vec![],
        signature_warnings: vec![],
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
    let uri = prod_code_protocol::path::file_uri(lib.as_path());

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

/// When a function signature is updated in one file, unadjusted call sites in sibling files
/// are proactively detected and reported as signature warnings before compilation (Roadmap 8.1).
#[tokio::test]
async fn analyze_proactively_warns_when_signature_change_leaves_unadjusted_sibling_call_site() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            "pub fn helper(x: i32) -> i32 {\n    x + 1\n}\n",
        ),
        (
            "src/sibling.rs",
            "pub fn caller() -> i32 {\n    crate::helper(10)\n}\n",
        ),
    ]);
    let root = ws.root();
    // Signature change in src/lib.rs: helper(x: i32) -> helper(x: i32, extra: bool)
    ws.write(
        "src/lib.rs",
        "pub fn helper(x: i32, extra: bool) -> i32 {\n    if extra { x + 2 } else { x + 1 }\n}\n",
    );
    // src/sibling.rs is left unadjusted!
    let lib_uri = prod_code_protocol::path::file_uri(ws.path("src/lib.rs").as_path());
    let sib_uri = prod_code_protocol::path::file_uri(ws.path("src/sibling.rs").as_path());

    let remote = ScriptedGateway::start_arc(Arc::new(move |method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("helper", 12, 1, 3, 8),
        ]),
        "textDocument/prepareCallHierarchy" => {
            serde_json::json!([{ "name": "helper", "uri": lib_uri, "_id": "helper" }])
        }
        "callHierarchy/incomingCalls" => {
            serde_json::json!([{
                "from": {
                    "name": "caller",
                    "uri": sib_uri,
                    "selectionRange": { "start": { "line": 0, "character": 7 } }
                },
                "fromRanges": [
                    { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 17 } }
                ]
            }])
        }
        "textDocument/references" => {
            serde_json::json!([
                {
                    "uri": sib_uri,
                    "range": {
                        "start": { "line": 1, "character": 4 },
                        "end": { "line": 1, "character": 17 }
                    }
                }
            ])
        }
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();

    let report = impact::analyze(remote, &root, None, 2)
        .await
        .expect("analysis runs");

    assert_eq!(report.signature_warnings.len(), 1);
    let warn = &report.signature_warnings[0];
    assert_eq!(warn.symbol.name, "helper");
    assert_eq!(warn.symbol.file, "src/lib.rs");
    assert!(warn.old_signature.contains("helper(x: i32)"));
    assert!(warn.new_signature.contains("helper(x: i32, extra: bool)"));
    assert_eq!(warn.unadjusted_call_sites.len(), 1);
    let site = &warn.unadjusted_call_sites[0];
    assert_eq!(site.file, "src/sibling.rs");
    assert_eq!(site.line, 2);
    assert_eq!(site.col, 5);
    assert_eq!(site.caller.as_deref(), Some("caller"));
    assert!(site.is_sibling);

    let rendered = report.render();
    assert!(rendered.contains("signature warnings (unadjusted call sites before full compilation):"));
    assert!(rendered.contains("`helper` signature changed in src/lib.rs:1:8"));
    assert!(rendered.contains("old: pub fn helper(x: i32) -> i32"));
    assert!(rendered.contains("new: pub fn helper(x: i32, extra: bool) -> i32"));
    assert!(rendered.contains("unadjusted sibling call sites (1):"));
    assert!(rendered.contains("• [sibling] src/sibling.rs:2:5 in `caller`"));

    let ci = report.ci_summary(None, "no tests affected");
    assert!(ci.contains("⚠️ **Signature Warnings**: updated signatures left unadjusted call sites:"));
    assert!(ci.contains("[sibling] `src/sibling.rs:2:5` in `caller`"));
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
// impact: what the analysis could not establish decides the CI run (#434)
// ---------------------------------------------------------------------------------------------

/// One incoming call from `name`, declared (0-based) at `line` of the file at `uri`.
fn call_from(name: &str, uri: &str, line: u64) -> serde_json::Value {
    serde_json::json!({
        "from": {
            "name": name,
            "uri": uri,
            "selectionRange": { "start": { "line": line, "character": 7 } }
        }
    })
}

/// A script for [`IMPACT_LIB`]: its three functions, a call-hierarchy item for `helper` and
/// `wrapper` (none for the test), and `incoming` answering for the item it is given.
fn impact_script(
    lib: &std::path::Path,
    incoming: impl Fn(&str, &str) -> serde_json::Value + Send + Sync + 'static,
) -> Answer {
    let uri = prod_code_protocol::path::file_uri(lib);
    Arc::new(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("helper", 12, 1, 3, 8),
            answers::document_symbol("wrapper", 12, 5, 7, 8),
            answers::document_symbol("it_calls_wrapper", 12, 12, 14, 8),
        ]),
        "textDocument/prepareCallHierarchy" => {
            let id = match params.pointer("/position/line").and_then(|l| l.as_u64()) {
                Some(0) => "helper",
                Some(4) => "wrapper",
                _ => return serde_json::json!([]),
            };
            serde_json::json!([{ "name": id, "uri": uri, "_id": id }])
        }
        "callHierarchy/incomingCalls" => incoming(
            params
                .pointer("/item/_id")
                .and_then(|i| i.as_str())
                .unwrap_or(""),
            &uri,
        ),
        _ => serde_json::Value::Null,
    })
}

/// `helper` is called by `wrapper`, which the test calls: the test is found by its attribute.
fn the_real_callers(id: &str, uri: &str) -> serde_json::Value {
    match id {
        "helper" => serde_json::json!([call_from("wrapper", uri, 4)]),
        "wrapper" => serde_json::json!([call_from("it_calls_wrapper", uri, 11)]),
        _ => serde_json::json!([]),
    }
}

fn selecting(tests: &[&str]) -> CiRun {
    let mut command: Vec<String> = ["cargo", "test", "--workspace", "--"]
        .iter()
        .map(|w| w.to_string())
        .collect();
    command.extend(tests.iter().map(|t| t.to_string()));
    CiRun::Selected(command)
}

/// A deleted source file is a gap: whatever called its functions changed with it, and no
/// analyzer can be asked about a file that is gone. Before, the diff parser dropped the file
/// and `impact --ci` ran nothing.
#[tokio::test]
async fn a_deleted_file_makes_ci_run_the_whole_suite() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", IMPACT_LIB),
        ("src/gone.rs", "pub fn gone() -> i32 {\n    1\n}\n"),
    ]);
    let root = ws.root();
    std::fs::remove_file(ws.path("src/gone.rs")).expect("delete");

    let lines = impact::changed_lines(&root, None).expect("diff parses");
    assert_eq!(lines.get("src/gone.rs"), Some(&vec![]), "{lines:?}");

    let remote = ScriptedGateway::start_arc(no_lsp()).await.addr();
    let report = impact::analyze(remote, &root, None, 4)
        .await
        .expect("analysis runs");

    assert_eq!(report.changed_files, vec!["src/gone.rs".to_string()]);
    assert_eq!(
        report.incomplete,
        vec![Gap::Deleted {
            file: "src/gone.rs".to_string()
        }]
    );
    let decision = report.ci_decision();
    assert_eq!(decision.run, CiRun::WholeSuite);
    assert!(
        decision.why.contains("src/gone.rs was deleted"),
        "{}",
        decision.why
    );
    assert!(report.render().contains("incomplete analysis"));
}

const MIXED_LIB: &str = "pub const LIMIT: i32 = 1;\n\npub fn helper(x: i32) -> i32 {\n    x + LIMIT\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_calls_helper() {\n        assert_eq!(super::helper(1), 2);\n    }\n}\n";

/// A change to a function and to a module-level constant in one file: the function's tests are
/// found, but the constant may be used anywhere, so the file still needs the whole suite. Each
/// hunk is judged on its own; before, one changed function vouched for the whole file. A new
/// function added with the blank line before it is inside a function, and so is the selection.
#[tokio::test]
async fn a_module_level_change_beside_a_function_change_runs_the_whole_suite() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", MIXED_LIB)]);
    let root = ws.root();
    let lib = ws.path("src/lib.rs");
    let uri = prod_code_protocol::path::file_uri(lib.as_path());
    let remote = ScriptedGateway::start_arc(Arc::new(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("helper", 12, 3, 5, 8),
            answers::document_symbol("it_calls_helper", 12, 10, 12, 8),
            answers::document_symbol("extra", 12, 15, 17, 8),
        ]),
        "textDocument/prepareCallHierarchy" => {
            let id = match params.pointer("/position/line").and_then(|l| l.as_u64()) {
                Some(2) => "helper",
                Some(14) => "extra",
                _ => return serde_json::json!([]),
            };
            serde_json::json!([{ "name": id, "uri": uri, "_id": id }])
        }
        "callHierarchy/incomingCalls" => match params.pointer("/item/_id").and_then(|i| i.as_str())
        {
            Some("helper") => serde_json::json!([call_from("it_calls_helper", &uri, 9)]),
            _ => serde_json::json!([]),
        },
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();
    let body = MIXED_LIB.replace("x + LIMIT", "x + LIMIT + 0");

    ws.write(
        "src/lib.rs",
        &body.replace("LIMIT: i32 = 1", "LIMIT: i32 = 2"),
    );
    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    assert_eq!(report.changed, vec![sym("helper", "src/lib.rs", 3, 8)]);
    assert_eq!(
        report.tests,
        vec![sym("it_calls_helper", "src/lib.rs", 10, 8)]
    );
    assert_eq!(report.unattributed_files, vec!["src/lib.rs".to_string()]);
    let decision = report.ci_decision();
    assert_eq!(decision.run, CiRun::WholeSuite);
    assert!(
        decision.why.contains("outside any function"),
        "{}",
        decision.why
    );

    // The function alone: its test is the selection.
    ws.write("src/lib.rs", &body);
    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    assert!(report.unattributed_files.is_empty());
    assert_eq!(report.ci_decision().run, selecting(&["it_calls_helper"]));

    // A new function with the blank line that separates it is not module-level code.
    ws.write(
        "src/lib.rs",
        &format!("{body}\npub fn extra() -> i32 {{\n    3\n}}\n"),
    );
    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    assert_eq!(
        report.changed,
        vec![
            sym("helper", "src/lib.rs", 3, 8),
            sym("extra", "src/lib.rs", 15, 8)
        ]
    );
    assert!(
        report.unattributed_files.is_empty(),
        "{:?}",
        report.unattributed_files
    );
    assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
    assert_eq!(report.ci_decision().run, selecting(&["it_calls_helper"]));

    // A removed constant took something at module level with it.
    ws.write(
        "src/lib.rs",
        &body.replace("pub const LIMIT: i32 = 1;\n", ""),
    );
    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    assert_eq!(report.unattributed_files, vec!["src/lib.rs".to_string()]);
    assert_eq!(report.ci_decision().run, CiRun::WholeSuite);
}

/// A changed test is an affected test, whatever calls it. Before, only callers were tests, so
/// an edit to a test alone selected nothing and `impact --ci` ran nothing.
#[tokio::test]
async fn a_directly_changed_test_is_selected() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", IMPACT_LIB)]);
    let root = ws.root();
    ws.write(
        "src/lib.rs",
        &IMPACT_LIB.replace(
            "assert_eq!(wrapper(), 42);",
            "assert_eq!(wrapper(), 42, \"the answer\");",
        ),
    );
    let remote =
        ScriptedGateway::start_arc(impact_script(&ws.path("src/lib.rs"), the_real_callers))
            .await
            .addr();

    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");

    let test = sym("it_calls_wrapper", "src/lib.rs", 12, 8);
    assert_eq!(report.changed, vec![test.clone()]);
    assert_eq!(report.tests, vec![test.clone()]);
    assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
    assert_eq!(
        report.reaches,
        vec![impact::Reach {
            test: test.clone(),
            changed: test,
            hops: 0
        }]
    );
    assert_eq!(report.ci_decision().run, selecting(&["it_calls_wrapper"]));
}

/// A call-hierarchy request that fails is not "no callers": the function's tests are unknown,
/// and CI runs the whole suite. Before, the error read as an empty answer and CI ran nothing.
/// A failed symbol listing is a gap of its own.
#[tokio::test]
async fn a_failed_request_makes_ci_run_the_whole_suite() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", IMPACT_LIB)]);
    let root = ws.root();
    let lib = ws.path("src/lib.rs");
    ws.write("src/lib.rs", &IMPACT_LIB.replace("x + 1", "x + 2"));

    let remote = ScriptedGateway::start_arc(impact_script(&lib, |id, _| match id {
        "helper" => answers::failure("the analyzer crashed"),
        _ => serde_json::json!([]),
    }))
    .await
    .addr();
    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    assert_eq!(report.changed, vec![sym("helper", "src/lib.rs", 1, 8)]);
    assert!(report.tests.is_empty());
    match report.incomplete.as_slice() {
        [Gap::Callers { symbol, error }] => {
            assert_eq!(symbol.name, "helper");
            assert!(error.contains("the analyzer crashed"), "{error}");
        }
        other => panic!("one gap expected: {other:?}"),
    }
    let decision = report.ci_decision();
    assert_eq!(decision.run, CiRun::WholeSuite);
    assert!(
        decision.why.contains("the callers of helper"),
        "{}",
        decision.why
    );
    assert!(
        report
            .render()
            .contains("affected tests: unknown (the analysis is incomplete)"),
        "{}",
        report.render()
    );

    let remote = ScriptedGateway::start(|method, _| match method {
        "textDocument/documentSymbol" => answers::failure("the analyzer is gone"),
        _ => serde_json::Value::Null,
    })
    .await
    .addr();
    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    match report.incomplete.as_slice() {
        [Gap::Symbols { file, error }] => {
            assert_eq!(file, "src/lib.rs");
            assert!(error.contains("the analyzer is gone"), "{error}");
        }
        other => panic!("one gap expected: {other:?}"),
    }
    assert_eq!(report.ci_decision().run, CiRun::WholeSuite);
}

/// An answer that is not the shape the protocol gives it (an object for a list, a call without
/// its caller, a string for the items) is not "no callers" either.
#[tokio::test]
async fn a_malformed_reply_makes_ci_run_the_whole_suite() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", IMPACT_LIB)]);
    let root = ws.root();
    let lib = ws.path("src/lib.rs");
    ws.write("src/lib.rs", &IMPACT_LIB.replace("x + 1", "x + 2"));

    for bad in [
        serde_json::json!({ "calls": 1 }),
        serde_json::json!([{ "fromRanges": [] }]),
        serde_json::json!([{ "from": { "name": "wrapper" } }]),
    ] {
        let answer = bad.clone();
        let remote = ScriptedGateway::start_arc(impact_script(&lib, move |id, _| match id {
            "helper" => answer.clone(),
            _ => serde_json::json!([]),
        }))
        .await
        .addr();
        let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
        match report.incomplete.as_slice() {
            [Gap::Callers { symbol, error }] => {
                assert_eq!(symbol.name, "helper", "{bad}");
                assert!(error.contains("cannot read"), "{bad}: {error}");
            }
            other => panic!("{bad}: one gap expected: {other:?}"),
        }
        assert_eq!(report.ci_decision().run, CiRun::WholeSuite, "{bad}");
    }

    for malformed_item in [
        serde_json::json!("helper"),
        serde_json::json!([{}]),
        serde_json::json!([{"name": ""}]),
    ] {
        let remote = ScriptedGateway::start_arc(Arc::new(move |method, _| match method {
            "textDocument/documentSymbol" => {
                serde_json::json!([answers::document_symbol("helper", 12, 1, 3, 8)])
            }
            "textDocument/prepareCallHierarchy" => malformed_item.clone(),
            _ => serde_json::Value::Null,
        }))
        .await
        .addr();
        let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
        assert!(
            matches!(report.incomplete.as_slice(), [Gap::Callers { error, .. }] if error.contains("prepareCallHierarchy")),
            "{:?}",
            report.incomplete
        );
        assert_eq!(report.ci_decision().run, CiRun::WholeSuite);
    }
}

/// The walk stops at the depth limit; a function there that still has callers not yet seen is
/// a gap, since a test may lie beyond it. Before, the walk stopped silently and CI ran nothing.
#[tokio::test]
async fn a_walk_cut_by_the_depth_limit_makes_ci_run_the_whole_suite() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", IMPACT_LIB)]);
    let root = ws.root();
    let lib = ws.path("src/lib.rs");
    ws.write("src/lib.rs", &IMPACT_LIB.replace("x + 1", "x + 2"));
    let remote = ScriptedGateway::start_arc(impact_script(&lib, the_real_callers))
        .await
        .addr();

    let report = impact::analyze(remote, &root, None, 1).await.expect("runs");
    assert!(report.tests.is_empty());
    assert_eq!(
        report.incomplete,
        vec![Gap::Depth {
            symbol: sym("wrapper", "src/lib.rs", 5, 8),
            depth: 1
        }]
    );
    let decision = report.ci_decision();
    assert_eq!(decision.run, CiRun::WholeSuite);
    assert!(
        decision.why.contains("stopped at depth 1"),
        "{}",
        decision.why
    );

    // One level more reaches the test, and nothing is left beyond it.
    let report = impact::analyze(remote, &root, None, 2).await.expect("runs");
    assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
    assert_eq!(report.ci_decision().run, selecting(&["it_calls_wrapper"]));
}

/// `script` with `method` answered by `answer` instead.
fn answering(
    script: Answer,
    method: &'static str,
    answer: impl Fn(&serde_json::Value) -> serde_json::Value + Send + Sync + 'static,
) -> Answer {
    Arc::new(move |m, params| {
        if m == method {
            answer(params)
        } else {
            script(m, params)
        }
    })
}

/// Git quotes a path with a byte beyond ASCII, a tab, a quote or a backslash in its diff
/// headers (`"a/\303\274 x.rs"`), and follows a name with a space with a tab. Every such path is
/// read back as it is on disk, a deleted one included; before, a quoted path was dropped and a
/// spaced one carried the tab.
#[cfg(unix)]
#[tokio::test]
async fn changed_lines_reads_quoted_and_unicode_paths_and_their_deletions() {
    let modified = ["src/ü x.rs", "src/sp ace.rs", "src/t\tq\\\"z.rs"];
    let mut files: Vec<(&str, &str)> = modified.iter().map(|f| (*f, "a\nb\n")).collect();
    files.extend([("src/plain.rs", "a\nb\n"), ("src/gône.rs", "a\n")]);
    let ws = Workspace::new(&files);
    for file in modified {
        ws.write(file, "a\nc\n");
    }
    std::fs::remove_file(ws.path("src/gône.rs")).expect("delete");
    ws.write("src/nëw \"u\".rs", "n\n");

    let lines = impact::changed_lines(&ws.root(), None).expect("diff parses");

    let mut expected: std::collections::BTreeMap<String, Vec<(u32, u32)>> = modified
        .iter()
        .map(|f| (f.to_string(), vec![(2, 2)]))
        .collect();
    expected.insert("src/gône.rs".to_string(), vec![]);
    expected.insert("src/nëw \"u\".rs".to_string(), vec![(1, u32::MAX)]);
    assert_eq!(lines, expected);
}

/// A change under a path git quotes is analyzed like any other, and the deletion of one is a
/// gap. Before, both were dropped: the change selected nothing and CI ran nothing.
#[cfg(unix)]
#[tokio::test]
async fn a_quoted_path_is_analyzed_and_its_deletion_runs_the_whole_suite() {
    let lib_name = "src/ünï \"q\"\tt.rs";
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (lib_name, IMPACT_LIB),
        ("src/gône.rs", "pub fn gone() -> i32 {\n    1\n}\n"),
    ]);
    let root = ws.root();
    ws.write(lib_name, &IMPACT_LIB.replace("x + 1", "x + 2"));
    let remote = ScriptedGateway::start_arc(impact_script(&ws.path(lib_name), the_real_callers))
        .await
        .addr();

    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    assert_eq!(report.changed_files, vec![lib_name.to_string()]);
    assert_eq!(report.changed, vec![sym("helper", lib_name, 1, 8)]);
    assert_eq!(report.tests, vec![sym("it_calls_wrapper", lib_name, 12, 8)]);
    assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
    assert_eq!(report.ci_decision().run, selecting(&["it_calls_wrapper"]));

    std::fs::remove_file(ws.path("src/gône.rs")).expect("delete");
    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    assert_eq!(
        report.incomplete,
        vec![Gap::Deleted {
            file: "src/gône.rs".to_string()
        }]
    );
    let decision = report.ci_decision();
    assert_eq!(decision.run, CiRun::WholeSuite);
    assert!(
        decision.why.contains("src/gône.rs was deleted"),
        "{}",
        decision.why
    );
}

/// A binary change to a source file has no text hunks, and what it touches is unknown: a gap,
/// not a mode change. Before, it read as a mode change and CI ran nothing. A real mode change
/// still changes no function.
#[cfg(unix)]
#[tokio::test]
async fn a_binary_source_change_runs_the_whole_suite_and_a_mode_change_does_not() {
    use std::os::unix::fs::PermissionsExt;
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", IMPACT_LIB),
        ("src/blob.rs", "\0binary\n"),
    ]);
    let root = ws.root();
    let remote =
        ScriptedGateway::start_arc(impact_script(&ws.path("src/lib.rs"), the_real_callers))
            .await
            .addr();

    std::fs::set_permissions(
        ws.path("src/lib.rs"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("chmod");
    let lines = impact::changed_lines(&root, None).expect("diff parses");
    assert_eq!(lines.get("src/lib.rs"), Some(&vec![]), "{lines:?}");
    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    assert_eq!(report.changed_files, vec!["src/lib.rs".to_string()]);
    assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
    assert_eq!(report.ci_decision().run, CiRun::Nothing);

    ws.write("src/blob.rs", "\0binary, changed\n");
    let lines = impact::changed_lines(&root, None).expect("diff parses");
    assert_eq!(lines.get("src/blob.rs"), Some(&vec![(1, u32::MAX)]));
    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    match report.incomplete.as_slice() {
        [Gap::Diff { file, error }] => {
            assert_eq!(file, "src/blob.rs");
            assert!(error.contains("binary"), "{error}");
        }
        other => panic!("one gap expected: {other:?}"),
    }
    let decision = report.ci_decision();
    assert_eq!(decision.run, CiRun::WholeSuite);
    assert!(decision.why.contains("src/blob.rs"), "{}", decision.why);
}

/// A symbol list with an entry that is not a symbol, at any depth, is not a list of the file's
/// functions: the file is a gap. Before, the entry was skipped (a position out of range was cut
/// to line 1) and the selection was trusted. An empty list is an answer; `null` says nothing.
#[tokio::test]
async fn a_malformed_symbol_list_makes_ci_run_the_whole_suite() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", IMPACT_LIB)]);
    let root = ws.root();
    let lib = ws.path("src/lib.rs");
    ws.write("src/lib.rs", &IMPACT_LIB.replace("x + 1", "x + 2"));
    let hidden = |start: serde_json::Value| {
        serde_json::json!({
            "name": "hidden", "kind": 12,
            "range": { "start": { "line": start, "character": 0 }, "end": { "line": 6, "character": 1 } },
            "selectionRange": { "start": { "line": 4, "character": 7 }, "end": { "line": 4, "character": 13 } }
        })
    };
    for bad in [
        serde_json::json!(7),
        serde_json::json!({ "name": "hidden", "kind": 12 }),
        serde_json::json!({ "name": 5, "kind": 12 }),
        serde_json::json!({ "name": "m", "kind": 2, "children": { "hidden": 1 } }),
        serde_json::json!({ "name": "m", "kind": 2, "children": [hidden(serde_json::json!(-1))] }),
        hidden(serde_json::json!(4_294_967_296u64)),
    ] {
        let symbols = serde_json::json!([
            answers::document_symbol("helper", 12, 1, 3, 8),
            answers::document_symbol("wrapper", 12, 5, 7, 8),
            answers::document_symbol("it_calls_wrapper", 12, 12, 14, 8),
            bad.clone(),
        ]);
        let remote = ScriptedGateway::start_arc(answering(
            impact_script(&lib, the_real_callers),
            "textDocument/documentSymbol",
            move |_| symbols.clone(),
        ))
        .await
        .addr();
        let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
        match report.incomplete.as_slice() {
            [Gap::Symbols { file, error }] => {
                assert_eq!(file, "src/lib.rs", "{bad}");
                assert!(error.contains("cannot read"), "{bad}: {error}");
            }
            other => panic!("{bad}: one gap expected: {other:?}"),
        }
        assert_eq!(report.ci_decision().run, CiRun::WholeSuite, "{bad}");
    }

    for (answer, gap) in [
        (serde_json::json!([]), false),
        (serde_json::Value::Null, true),
    ] {
        let remote = ScriptedGateway::start_arc(answering(
            impact_script(&lib, the_real_callers),
            "textDocument/documentSymbol",
            move |_| answer.clone(),
        ))
        .await
        .addr();
        let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
        assert!(report.changed.is_empty());
        if gap {
            assert!(
                matches!(report.incomplete.as_slice(), [Gap::Symbols { error, .. }] if error.contains("null")),
                "{:?}",
                report.incomplete
            );
        } else {
            // No function: the change lies outside any, which the whole suite covers.
            assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
            assert_eq!(report.unattributed_files, vec!["src/lib.rs".to_string()]);
        }
        assert_eq!(report.ci_decision().run, CiRun::WholeSuite);
    }
}

/// A call whose caller has no name, or whose position is not a line and column (negative,
/// beyond u32, missing), or whose test flag is not a flag, is not "no caller". Before, a call
/// without a name was skipped and a bad position became line 1.
#[tokio::test]
async fn a_call_without_a_readable_caller_makes_ci_run_the_whole_suite() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", IMPACT_LIB)]);
    let root = ws.root();
    let lib = ws.path("src/lib.rs");
    ws.write("src/lib.rs", &IMPACT_LIB.replace("x + 1", "x + 2"));
    // Each call as the analyzer sends it, but for the caller's file, filled in below.
    let from = |name: Option<&str>, start: serde_json::Value| {
        let mut from = serde_json::json!({ "selectionRange": { "start": start } });
        if let Some(name) = name {
            from["name"] = name.into();
        }
        serde_json::json!({ "from": from })
    };
    let good = serde_json::json!({ "line": 4, "character": 7 });
    let mut flagged = from(Some("wrapper"), good.clone());
    flagged["isTest"] = "no".into();
    let cases = [
        from(None, good.clone()),
        from(Some(""), good.clone()),
        from(
            Some("wrapper"),
            serde_json::json!({ "line": -1, "character": 7 }),
        ),
        from(
            Some("wrapper"),
            serde_json::json!({ "line": 4_294_967_300u64, "character": 7 }),
        ),
        from(Some("wrapper"), serde_json::json!({ "line": 4 })),
        flagged,
    ];
    for (n, case) in cases.into_iter().enumerate() {
        let remote = ScriptedGateway::start_arc(impact_script(&lib, move |id, uri| match id {
            "helper" => {
                let mut call = case.clone();
                call["from"]["uri"] = uri.into();
                serde_json::json!([call])
            }
            _ => serde_json::json!([]),
        }))
        .await
        .addr();
        let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
        match report.incomplete.as_slice() {
            [Gap::Callers { symbol, error }] => {
                assert_eq!(symbol.name, "helper", "case {n}");
                assert!(error.contains("cannot read"), "case {n}: {error}");
            }
            other => panic!("case {n}: one gap expected: {other:?}"),
        }
        assert!(report.tests.is_empty(), "case {n}");
        assert_eq!(report.ci_decision().run, CiRun::WholeSuite, "case {n}");
    }
}

#[tokio::test]
async fn an_invalid_caller_uri_is_unknown_not_a_selected_test() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", IMPACT_LIB)]);
    let root = ws.root();
    let lib = ws.path("src/lib.rs");
    ws.write("src/lib.rs", &IMPACT_LIB.replace("x + 1", "x + 2"));
    for bad_uri in ["not-a-uri", "https://example.invalid/test.rs"] {
        let remote = ScriptedGateway::start_arc(impact_script(&lib, move |id, _| match id {
            "helper" => serde_json::json!([call_from("test_lost", bad_uri, 4)]),
            _ => serde_json::json!([]),
        }))
        .await
        .addr();
        let report = impact::analyze(remote, &root, None, 4).await.unwrap();
        assert_eq!(report.ci_decision().run, CiRun::WholeSuite, "{bad_uri}");
        assert!(report.tests.is_empty(), "{bad_uri}");
        assert!(
            matches!(report.incomplete.as_slice(), [Gap::Callers { .. }]),
            "{bad_uri}: {:?}",
            report.incomplete
        );
    }
}

/// A name can stand for several call-hierarchy items (a declaration and its definition): the
/// callers of each are callers of the function. Before, only the first was asked, and a test
/// reaching the second was missed: CI ran nothing.
#[tokio::test]
async fn the_callers_of_every_call_hierarchy_item_are_walked() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", IMPACT_LIB)]);
    let root = ws.root();
    let lib = ws.path("src/lib.rs");
    ws.write("src/lib.rs", &IMPACT_LIB.replace("x + 1", "x + 2"));
    let uri = prod_code_protocol::path::file_uri(lib.as_path());
    let items = move |params: &serde_json::Value, second: serde_json::Value| match params
        .pointer("/position/line")
        .and_then(|l| l.as_u64())
    {
        Some(0) => serde_json::json!([
            { "name": "helper", "uri": uri, "_id": "helper-declaration" },
            second,
        ]),
        Some(4) => serde_json::json!([{ "name": "wrapper", "uri": uri, "_id": "wrapper" }]),
        _ => serde_json::json!([]),
    };
    let definition = serde_json::json!({ "name": "helper", "_id": "helper" });

    let both = {
        let (items, definition) = (items.clone(), definition.clone());
        move |params: &serde_json::Value| items(params, definition.clone())
    };
    let remote = ScriptedGateway::start_arc(answering(
        impact_script(&lib, the_real_callers),
        "textDocument/prepareCallHierarchy",
        both,
    ))
    .await
    .addr();
    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
    assert_eq!(report.callers, vec![sym("wrapper", "src/lib.rs", 5, 8)]);
    assert_eq!(
        report.tests,
        vec![sym("it_calls_wrapper", "src/lib.rs", 12, 8)]
    );
    assert_eq!(report.ci_decision().run, selecting(&["it_calls_wrapper"]));

    // The second item's callers cannot be asked: unknown, whatever the first said.
    let remote = ScriptedGateway::start_arc(answering(
        impact_script(&lib, |id, uri| match id {
            "helper" => answers::failure("the definition's callers are gone"),
            other => the_real_callers(other, uri),
        }),
        "textDocument/prepareCallHierarchy",
        move |params| items(params, definition.clone()),
    ))
    .await
    .addr();
    let report = impact::analyze(remote, &root, None, 4).await.expect("runs");
    assert!(
        matches!(report.incomplete.as_slice(), [Gap::Callers { error, .. }] if error.contains("gone")),
        "{:?}",
        report.incomplete
    );
    assert_eq!(report.ci_decision().run, CiRun::WholeSuite);
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

/// [`DEAD_LIB`] with the analyzer failing in every way it can: a request that fails, a `null`
/// (no result), an answer of the wrong shape, and a second file whose symbols
/// cannot be listed. Only the successful empty list makes a symbol unreferenced. `safe_deletes`
/// counts the deletions pruning asks for.
async fn failing_dead_code_gateway(
    safe_deletes: Arc<std::sync::atomic::AtomicUsize>,
) -> SocketAddr {
    ScriptedGateway::start_arc(Arc::new(move |method, params| match method {
        "textDocument/documentSymbol" => {
            let uri = params
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            if uri.ends_with("other.rs") {
                answers::failure("documentSymbol timed out")
            } else {
                dead_symbols()
            }
        }
        "textDocument/references" => {
            match params.pointer("/position/line").and_then(|l| l.as_u64()) {
                Some(0) => serde_json::json!([{ "uri": "file:///x.rs", "range": {} }]),
                Some(2) => answers::failure("references failed"),
                Some(4) => serde_json::Value::Null,
                Some(9) => serde_json::json!({ "unexpected": true }),
                _ => serde_json::json!([]),
            }
        }
        "prodCode/safeDelete" => {
            safe_deletes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            serde_json::Value::Null
        }
        _ => serde_json::Value::Null,
    }))
    .await
    .addr()
}

/// An error or an unreadable answer is not proof that nothing references a symbol: it is
/// unverified, not dead, and pruning keeps it (#435). Before, each of them read as zero
/// references, and `private_unreferenced` and `plain_method` were listed dead.
#[tokio::test]
async fn a_symbol_the_analyzer_could_not_answer_for_is_unverified_not_dead() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", DEAD_LIB),
        ("src/other.rs", "fn lonely() {}\n"),
    ]);
    let root = ws.root();
    let safe_deletes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let remote = failing_dead_code_gateway(Arc::clone(&safe_deletes)).await;

    let report = dead_code::find_dead_code(remote, &root, false, 100)
        .await
        .expect("scan runs");

    assert!(report.dead.is_empty(), "{:?}", report.dead);
    assert_eq!(report.exported_unreferenced, 0);
    let methods: Vec<&str> = report
        .methods_unreferenced
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(
        methods,
        ["draw"],
        "the successful empty answer still counts"
    );
    let unverified: Vec<(&str, Option<&str>)> = report
        .unverified
        .iter()
        .map(|u| (u.file.as_str(), u.name.as_deref()))
        .collect();
    assert_eq!(
        unverified,
        [
            ("src/lib.rs", Some("private_unreferenced")),
            ("src/lib.rs", Some("public_unreferenced")),
            ("src/lib.rs", Some("plain_method")),
            ("src/other.rs", None),
        ]
    );
    assert!(report.unverified[0].reason.contains("references failed"));
    assert!(report.unverified[1].reason.contains("null"));
    // `null` is the protocol's "no result": it does not prove nothing was searched for.
    assert!(
        !report.unverified[1]
            .reason
            .contains("no symbol was searched"),
        "{}",
        report.unverified[1].reason
    );
    assert!(report.unverified[2].reason.contains("cannot read"));
    assert!(
        report.unverified[3]
            .reason
            .contains("documentSymbol timed out")
    );
    assert_eq!(report.files_scanned, 1);
    assert!(!report.complete());
    let text = report.render();
    assert!(text.contains("4 could not be checked"), "{text}");
    assert!(
        text.contains("private_unreferenced  src/lib.rs:3:4"),
        "{text}"
    );
    let json = serde_json::to_value(&report).expect("serializes");
    assert_eq!(json["unverified"].as_array().map(|a| a.len()), Some(4));

    let pruned = prod_code_mcp::prune::prune_orphans(remote, &root, 100, true, false)
        .await
        .expect("prune runs");
    assert!(pruned.removed.is_empty(), "{:?}", pruned.removed);
    assert!(!pruned.applied);
    assert_eq!(pruned.unverified.len(), 4);
    assert_eq!(
        safe_deletes.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "nothing unverified is offered for deletion"
    );
    assert_eq!(ws.read("src/lib.rs"), DEAD_LIB);
    let text = pruned.render();
    assert!(
        text.contains("kept private_unreferenced (src/lib.rs:3): its references are unknown"),
        "{text}"
    );
    assert!(text.contains("kept everything in src/other.rs"), "{text}");
    assert!(text.contains("nothing proven orphaned"), "{text}");
}

/// A symbol list with an entry that cannot be read, at any depth, leaves its file unverified:
/// the scan is not complete, and pruning deletes nothing in it. Before, the entry was skipped or
/// its position cut to line 1, the scan claimed to be complete, and `lonely` was pruned from an
/// answer that never placed it. An empty list is an answer; `null` leaves its file unverified.
#[tokio::test]
async fn a_malformed_symbol_list_leaves_its_file_unverified_and_unpruned() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    const LONELY: &str = "fn lonely() {}\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", LONELY),
        ("src/empty.rs", "\n"),
        ("src/quiet.rs", "fn quiet() {}\n"),
    ]);
    let root = ws.root();
    let lonely = |start: serde_json::Value| serde_json::json!({ "name": "lonely", "kind": 12, "selectionRange": { "start": start } });
    let at = |line: serde_json::Value| serde_json::json!({ "line": line, "character": 3 });
    let gateway = |lib: serde_json::Value, quiet: serde_json::Value| {
        let deletes = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&deletes);
        let script: Answer = Arc::new(move |method, params| match method {
            "textDocument/documentSymbol" => {
                let uri = params
                    .pointer("/textDocument/uri")
                    .and_then(|u| u.as_str())
                    .unwrap_or("");
                if uri.ends_with("lib.rs") {
                    lib.clone()
                } else if uri.ends_with("quiet.rs") {
                    quiet.clone()
                } else {
                    serde_json::json!([])
                }
            }
            "textDocument/references" => serde_json::json!([]),
            "prodCode/safeDelete" => {
                counted.fetch_add(1, Ordering::SeqCst);
                serde_json::Value::Null
            }
            _ => serde_json::Value::Null,
        });
        (script, deletes)
    };

    // The control: a readable list makes `lonely` dead, and an empty one is a complete answer.
    let (script, _) = gateway(
        serde_json::json!([lonely(at(serde_json::json!(0)))]),
        serde_json::json!([]),
    );
    let remote = ScriptedGateway::start_arc(script).await.addr();
    let report = dead_code::find_dead_code(remote, &root, false, 100)
        .await
        .expect("scan runs");
    let dead: Vec<(&str, u32, u32)> = report
        .dead
        .iter()
        .map(|d| (d.name.as_str(), d.line, d.col))
        .collect();
    assert_eq!(dead, [("lonely", 1, 4)]);
    assert!(report.complete(), "{:?}", report.unverified);

    for bad in [
        serde_json::json!([lonely(at(serde_json::json!(4_294_967_296u64)))]),
        serde_json::json!([lonely(at(serde_json::json!(-1)))]),
        serde_json::json!([lonely(serde_json::json!({ "line": 0 }))]),
        serde_json::json!([{ "name": "lonely", "kind": 12 }]),
        serde_json::json!([{ "name": "", "kind": 12 }]),
        serde_json::json!([{ "name": "lonely", "kind": 99 }]),
        serde_json::json!([42, lonely(at(serde_json::json!(0)))]),
        serde_json::json!([{ "name": "Holder", "kind": 23,
            "selectionRange": { "start": { "line": 0, "character": 0 } },
            "children": { "lonely": 1 } }]),
        serde_json::json!([{ "name": "m", "kind": 2,
            "children": [lonely(at(serde_json::json!(0))), { "kind": 12 }] }]),
    ] {
        let (script, deletes) = gateway(bad.clone(), serde_json::Value::Null);
        let remote = ScriptedGateway::start_arc(script).await.addr();
        let report = dead_code::find_dead_code(remote, &root, false, 100)
            .await
            .expect("scan runs");
        assert!(report.dead.is_empty(), "{bad}: {:?}", report.dead);
        let unverified: Vec<(&str, Option<&str>)> = report
            .unverified
            .iter()
            .map(|u| (u.file.as_str(), u.name.as_deref()))
            .collect();
        assert_eq!(
            unverified,
            [("src/lib.rs", None), ("src/quiet.rs", None)],
            "{bad}"
        );
        assert!(
            report.unverified[0].reason.contains("cannot read"),
            "{bad}: {}",
            report.unverified[0].reason
        );
        assert!(report.unverified[1].reason.contains("null"), "{bad}");
        assert_eq!(report.files_scanned, 1, "{bad}: only the empty list counts");
        assert!(!report.complete(), "{bad}");

        let pruned = prod_code_mcp::prune::prune_orphans(remote, &root, 100, true, false)
            .await
            .expect("prune runs");
        assert!(pruned.removed.is_empty(), "{bad}: {:?}", pruned.removed);
        assert!(!pruned.applied, "{bad}");
        assert_eq!(pruned.unverified.len(), 2, "{bad}");
        assert_eq!(deletes.load(Ordering::SeqCst), 0, "{bad}: nothing offered");
        assert_eq!(ws.read("src/lib.rs"), LONELY, "{bad}");
    }
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
        unverified: vec![],
        reachability: None,
        unreachable_clusters: vec![],
        root_entry_points: vec![],
    };

    let text = report.render();

    assert!(text.contains("3 file(s), 9 symbol(s) checked, 1 unreferenced"));
    assert!(text.contains("function orphan (exported)  src/lib.rs:4:1"));
    assert!(text.contains("methods without direct references (1"));
    assert!(text.contains("draw  src/lib.rs:9:5"));
    assert!(text.contains("2 exported symbol(s) are unreferenced"));
    assert!(text.contains("scan truncated by the file limit"));
}

/// [`Pruned::render`] displays removed orphans, Git commit information, and Git commit patch sections.
#[tokio::test]
async fn pruned_render_and_git_patch_contains_all_sections() {
    let pruned = prod_code_mcp::prune::Pruned {
        root: std::path::PathBuf::from("/workspace"),
        removed: vec![DeadItem {
            name: "unused_func".to_string(),
            kind: "function".to_string(),
            file: "src/lib.rs".to_string(),
            line: 10,
            col: 1,
            exported: false,
        }],
        skipped: vec![],
        rewritten: vec![("/workspace/src/lib.rs".to_string(), "fn active() {}\n".to_string())],
        diagnostics: vec![],
        applied: true,
        symbols_checked: 25,
        unverified: vec![],
        git_patch: Some(
            "From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001\nFrom: Alexander Panasenko <alex@prod.codes>\n".to_string(),
        ),
        git_commit: Some("abcdef1234567890".to_string()),
    };

    let text = pruned.render();
    assert!(text.contains("1 orphan(s) of 25 symbol(s) checked"));
    assert!(text.contains("- function unused_func (src/lib.rs:10)"));
    assert!(text.contains("[committed: abcdef1234567890] created Git commit with author Alexander Panasenko <alex@prod.codes>"));
    assert!(text.contains("--- Git Commit Patch ---"));
    assert!(text.contains("From: Alexander Panasenko <alex@prod.codes>"));
}

/// [`prune_orphans_opts`] with `commit: true` creates a commit using an isolated index,
/// preserving ambient staged changes in unrelated files without committing them,
/// and refuses to commit if touched files have uncommitted changes relative to HEAD.
#[tokio::test]
async fn prune_commit_isolates_index_and_preserves_ambient_staged_work() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    let run_git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .expect("git execution");
        assert!(status.success(), "git {:?}", args);
    };

    run_git(&["init"]);
    run_git(&["config", "user.name", "Alexander Panasenko"]);
    run_git(&["config", "user.email", "alex@prod.codes"]);

    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let lib_rs = src_dir.join("lib.rs");
    std::fs::write(&lib_rs, "pub fn keep() {}\nfn dead() {}\n").unwrap();
    let unrelated = root.join("unrelated.txt");
    std::fs::write(&unrelated, "initial unrelated\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), CARGO_TOML).unwrap();

    run_git(&["add", "."]);
    run_git(&["commit", "-m", "initial"]);

    // User stages unrelated change
    std::fs::write(&unrelated, "modified unrelated staged\n").unwrap();
    run_git(&["add", "unrelated.txt"]);

    let lib_uri = url::Url::from_file_path(&lib_rs).unwrap().to_string();
    let lib_uri_clone = lib_uri.clone();

    // Fake LSP gateway that returns documentSymbol, zero references, safeDelete, and clean diagnostics
    let lsp: Answer = Arc::new(move |method, _| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            {
                "name": "dead",
                "kind": 12,
                "selectionRange": {
                    "start": { "line": 1, "character": 3 },
                    "end": { "line": 1, "character": 11 }
                }
            }
        ]),
        "textDocument/references" => serde_json::json!([]),
        "prodCode/safeDelete" => serde_json::json!({
            "changes": {
                &lib_uri_clone: [{
                    "range": {
                        "start": { "line": 1, "character": 0 },
                        "end": { "line": 2, "character": 0 }
                    },
                    "newText": ""
                }]
            }
        }),
        "textDocument/diagnostic" => serde_json::json!({ "items": [] }),
        _ => serde_json::Value::Null,
    });
    let exec: ExecAnswer = Arc::new(|_| (Vec::new(), Vec::new(), Some(0)));
    let gateway = ExecGateway::start(lsp, exec).await;

    // Prune with commit: true and git_patch: true
    let pruned = prod_code_mcp::prune::prune_orphans_opts(
        gateway.addr(),
        root,
        prod_code_mcp::dead_code::DeadCodeOptions {
            include_exported: false,
            max_files: 100,
            reachability: false,
        },
        true,  // apply
        false, // force
        true,  // git_patch
        true,  // commit
    )
    .await
    .expect("prune with commit succeeds");

    assert!(pruned.applied);
    let commit_sha = pruned.git_commit.expect("commit was created");
    assert!(!commit_sha.is_empty());

    // Verify commit contains only src/lib.rs, NOT unrelated.txt
    let show_out = std::process::Command::new("git")
        .current_dir(root)
        .args(["show", "--stat", &commit_sha])
        .output()
        .expect("git show");
    let show_text = String::from_utf8_lossy(&show_out.stdout);
    assert!(show_text.contains("src/lib.rs"), "{show_text}");
    assert!(!show_text.contains("unrelated.txt"), "{show_text}");

    // Verify git status: unrelated.txt is STILL staged M, src/lib.rs is clean
    let status_out = std::process::Command::new("git")
        .current_dir(root)
        .args(["status", "--porcelain"])
        .output()
        .expect("git status");
    let status_text = String::from_utf8_lossy(&status_out.stdout);
    assert!(status_text.contains("M  unrelated.txt"), "{status_text}");
    assert!(!status_text.contains("src/lib.rs"), "{status_text}");

    // Now test that dirty touched files are refused before applying
    std::fs::write(&lib_rs, "pub fn keep() {}\n// dirty edit\n").unwrap();
    let err = prod_code_mcp::prune::prune_orphans_opts(
        gateway.addr(),
        root,
        prod_code_mcp::dead_code::DeadCodeOptions {
            include_exported: false,
            max_files: 100,
            reachability: false,
        },
        true,
        false,
        true,
        true,
    )
    .await
    .expect_err("dirty touched file must be refused");
    assert!(err.to_string().contains("touched file(s) have uncommitted changes"), "{err}");
}

/// [`prune_orphans_opts`] with `commit: true` functions in linked Git worktrees where `<root>/.git`
/// is a file pointing to the main Git directory rather than a directory.
#[tokio::test]
async fn prune_commit_works_in_linked_worktree() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let main_repo = tmp.path().join("main_repo");
    std::fs::create_dir_all(&main_repo).unwrap();

    let run_git = |dir: &std::path::Path, args: &[&str]| {
        let status = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .status()
            .expect("git execution");
        assert!(status.success(), "git {:?} in {}", args, dir.display());
    };

    run_git(&main_repo, &["init"]);
    run_git(&main_repo, &["config", "user.name", "Alexander Panasenko"]);
    run_git(&main_repo, &["config", "user.email", "alex@prod.codes"]);

    let src_dir = main_repo.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let lib_rs = src_dir.join("lib.rs");
    std::fs::write(&lib_rs, "pub fn keep() {}\nfn dead() {}\n").unwrap();
    std::fs::write(main_repo.join("Cargo.toml"), CARGO_TOML).unwrap();

    run_git(&main_repo, &["add", "."]);
    run_git(&main_repo, &["commit", "-m", "initial"]);

    // Create a linked worktree
    let wt_dir = tmp.path().join("linked_worktree");
    run_git(
        &main_repo,
        &["worktree", "add", wt_dir.to_str().unwrap(), "-b", "wt-test-branch"],
    );

    // Verify .git in worktree is a file, not a directory
    assert!(wt_dir.join(".git").is_file());

    let wt_lib_rs = wt_dir.join("src/lib.rs");
    let lib_uri = url::Url::from_file_path(&wt_lib_rs).unwrap().to_string();

    let lsp: Answer = Arc::new(move |method, _| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            {
                "name": "dead",
                "kind": 12,
                "selectionRange": {
                    "start": { "line": 1, "character": 3 },
                    "end": { "line": 1, "character": 11 }
                }
            }
        ]),
        "textDocument/references" => serde_json::json!([]),
        "prodCode/safeDelete" => serde_json::json!({
            "changes": {
                &lib_uri: [{
                    "range": {
                        "start": { "line": 1, "character": 0 },
                        "end": { "line": 2, "character": 0 }
                    },
                    "newText": ""
                }]
            }
        }),
        "textDocument/diagnostic" => serde_json::json!({ "items": [] }),
        _ => serde_json::Value::Null,
    });
    let exec: ExecAnswer = Arc::new(|_| (Vec::new(), Vec::new(), Some(0)));
    let gateway = ExecGateway::start(lsp, exec).await;

    // Run prune with commit: true on the linked worktree root
    let pruned = prod_code_mcp::prune::prune_orphans_opts(
        gateway.addr(),
        &wt_dir,
        prod_code_mcp::dead_code::DeadCodeOptions {
            include_exported: false,
            max_files: 100,
            reachability: false,
        },
        true,  // apply
        false, // force
        true,  // git_patch
        true,  // commit
    )
    .await
    .expect("prune in linked worktree succeeds");

    assert!(pruned.applied);
    let commit_sha = pruned.git_commit.expect("commit was created in linked worktree");
    assert!(!commit_sha.is_empty());

    // Verify HEAD in worktree matches created commit
    let rev_out = std::process::Command::new("git")
        .current_dir(&wt_dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse HEAD in worktree");
    let wt_head = String::from_utf8_lossy(&rev_out.stdout).trim().to_string();
    assert_eq!(wt_head, commit_sha);

    // Verify the file was pruned on disk
    let content = std::fs::read_to_string(&wt_lib_rs).unwrap();
    assert_eq!(content, "pub fn keep() {}\n");
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

    let report = dossier::diagnose(remote, &root, None, None, 60)
        .await
        .expect("diagnose runs");

    assert_eq!(report.tests_passed, 0);
    assert_eq!(report.tests_failed, 1);
    assert_eq!(report.dossiers.len(), 1);
    let dossier = &report.dossiers[0];
    assert_eq!(dossier.test, "tests::it_fails");
    assert!(dossier.output.contains("assertion `left == right` failed"));
    let assertion = dossier
        .assertion
        .as_ref()
        .expect("assertion evidence captured");
    assert_eq!(assertion.format, "assert_eq");
    assert_eq!(assertion.expression.as_deref(), Some("left == right"));
    assert_eq!(assertion.left.as_deref(), Some("4"));
    assert_eq!(assertion.right.as_deref(), Some("5"));
    assert_eq!(assertion.actual, None);
    assert_eq!(assertion.expected, None);
    assert_eq!(assertion.operands, vec!["4".to_string(), "5".to_string()]);
    assert!(assertion.excerpt.contains("left: 4"));
    assert!(assertion.excerpt.contains("right: 5"));
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
    assert!(rendered.contains("assertion [assert_eq (left == right)]: left: 4, right: 5\n"));
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

    let report = dossier::diagnose(remote, &root, None, None, 60)
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
            assertion: None,
            panic_line: Some(3),
            expression: None,
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

    let err = dossier::diagnose(unreachable, &root, None, None, 5)
        .await
        .expect_err("connecting to a closed port fails");

    assert!(
        format!("{err:#}").contains("failed to connect to remote gateway"),
        "{err:#}"
    );
}

#[tokio::test]
async fn diagnose_builds_a_dossier_for_rust_multiline_colored_assertion() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", DOSSIER_LIB)]);
    let root = ws.root();
    let output = b"running 1 test\ntest tests::it_fails ... FAILED\n\nfailures:\n\n---- tests::it_fails stdout ----\n\n\x1b[1m\x1b[31mthread 'tests::it_fails' panicked at \x1b[0msrc/lib.rs:13:9:\n\x1b[1m\x1b[31massertion `left == right` failed\x1b[0m\n\x1b[1m\x1b[31m  left: \x1b[0mFoo {\n    x: 1,\n    y: 2,\n}\n\x1b[1m\x1b[31m right: \x1b[0mFoo {\n    x: 1,\n    y: 3,\n}\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\nfailures:\n    tests::it_fails\n\ntest result: FAILED. 0 passed; 1 failed; 0 filtered out; finished in 0.00s\n".to_vec();
    let exec: ExecAnswer = Arc::new(move |_req| (output.clone(), Vec::new(), Some(101)));
    let remote = ExecGateway::start(no_lsp(), exec).await.addr();

    let report = dossier::diagnose(remote, &root, None, None, 60)
        .await
        .expect("diagnose runs");

    assert_eq!(report.dossiers.len(), 1);
    let dossier = &report.dossiers[0];
    let assertion = dossier
        .assertion
        .as_ref()
        .expect("assertion evidence captured");
    assert_eq!(assertion.format, "assert_eq");
    assert_eq!(
        assertion.left.as_deref(),
        Some("Foo {\n    x: 1,\n    y: 2,\n}")
    );
    assert_eq!(
        assertion.right.as_deref(),
        Some("Foo {\n    x: 1,\n    y: 3,\n}")
    );
    assert_eq!(assertion.operands.len(), 2);
    assert_eq!(assertion.actual, None);
    // Escape sequences stay in the excerpt as the runner printed them.
    assert!(
        assertion
            .excerpt
            .contains("\x1b[1m\x1b[31m  left: \x1b[0mFoo {\n")
    );
}

// Failure output captured from real runs on a Linux build node (cargo 1.97, jest 29.7, vitest
// 2.1.9, Node 22.23) of probe tests that mix an ordinary panic or thrown `Error` with
// assertions failing on different values, in `tests/fixtures/assertions/`.
const CARGO_MULTI: &str = include_str!("fixtures/assertions/cargo_test_multi.txt");
const JEST_NODE_ASSERT: &str = include_str!("fixtures/assertions/jest_node_assert.txt");
const JEST_NODE_ASSERT_EDGE: &str = include_str!("fixtures/assertions/jest_node_assert_edge.txt");
const JEST_NODE_ASSERT_COLORS: &str =
    include_str!("fixtures/assertions/jest_node_assert_colors.txt");
const VITEST_NODE_ASSERT: &str = include_str!("fixtures/assertions/vitest_node_assert.txt");
const NODE_TEST_ERROR_FIELDS: &str = include_str!("fixtures/assertions/node_test_error_fields.txt");

/// The probe crate behind `cargo_test_multi.txt`.
const PROBE_LIB: &str = r#"pub fn add(a: i32, b: i32) -> i32 { a + b }
#[derive(Debug, PartialEq)]
pub struct Foo { pub x: i32, pub y: Vec<i32> }
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_plain_panic() { panic!("boom without values"); }
    #[test]
    fn b_add_mismatch() { assert_eq!(add(2, 2), 5); }
    #[test]
    fn c_reversed_mismatch() { assert_eq!(20, add(5, 5), "totals differ"); }
    #[test]
    fn d_struct_mismatch() { assert_eq!(Foo { x: 1, y: vec![2] }, Foo { x: 1, y: vec![3] }); }
    #[test]
    fn e_ne() { assert_ne!(add(1, 1), 2); }
    #[test]
    fn f_bool() { assert!(add(1, 1) == 3); }
    #[test]
    fn g_string_with_newline() { assert_eq!("a\nb".to_string(), "a\nc"); }
    #[test]
    fn h_ok() {}
}
"#;

fn dossier_of<'a>(report: &'a DossierReport, test: &str) -> &'a FailureDossier {
    report
        .dossiers
        .iter()
        .find(|d| d.test == test)
        .unwrap_or_else(|| panic!("no dossier for {test}"))
}

async fn diagnose_output(ws: &Workspace, stdout: Vec<u8>, code: i32) -> DossierReport {
    let exec: ExecAnswer = Arc::new(move |_req| (stdout.clone(), Vec::new(), Some(code)));
    let remote = ExecGateway::start(no_lsp(), exec).await.addr();
    dossier::diagnose(remote, &ws.root(), None, None, 60)
        .await
        .expect("diagnose runs")
}

/// Every probe failure gets only its own evidence: the ordinary panic and the plain `assert!`
/// get none although the run's tail holds the other tests' assertions, and `assert_eq!`
/// operands keep their left/right names without being called actual or expected.
fn assert_cargo_probe_dossiers(report: &DossierReport) {
    assert_eq!(report.tests_failed, 7);
    assert_eq!(report.dossiers.len(), 7);
    for plain in ["tests::a_plain_panic", "tests::f_bool"] {
        assert_eq!(dossier_of(report, plain).assertion, None, "{plain}");
    }
    let expected = [
        ("tests::b_add_mismatch", "assert_eq", "4", "5"),
        ("tests::c_reversed_mismatch", "assert_eq", "20", "10"),
        (
            "tests::d_struct_mismatch",
            "assert_eq",
            "Foo { x: 1, y: [2] }",
            "Foo { x: 1, y: [3] }",
        ),
        ("tests::e_ne", "assert_ne", "2", "2"),
        (
            "tests::g_string_with_newline",
            "assert_eq",
            "\"a\\nb\"",
            "\"a\\nc\"",
        ),
    ];
    for (test, format, left, right) in expected {
        let d = dossier_of(report, test);
        let a = d
            .assertion
            .as_ref()
            .unwrap_or_else(|| panic!("no evidence for {test}"));
        assert_eq!(a.format, format, "{test}");
        assert_eq!(a.left.as_deref(), Some(left), "{test}");
        assert_eq!(a.right.as_deref(), Some(right), "{test}");
        assert_eq!(a.actual, None, "{test}");
        assert_eq!(a.expected, None, "{test}");
        assert_eq!(a.operands, vec![left, right], "{test}");
        assert!(d.output.contains(&a.excerpt), "{test}: {}", a.excerpt);
    }
    let rendered = report.render();
    assert!(rendered.contains("assertion [assert_eq (left == right)]: left: 20, right: 10\n"));
    assert!(!rendered.contains("actual:"));
}

#[tokio::test]
async fn diagnose_keeps_every_cargo_failure_to_its_own_assertion_evidence() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", PROBE_LIB)]);
    let report = diagnose_output(&ws, CARGO_MULTI.as_bytes().to_vec(), 101).await;
    assert!(report.tail.contains("  left: 4\n right: 5"));
    assert_cargo_probe_dossiers(&report);
}

fn probe_workspace(lib: &str) -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
        ),
        ("src/lib.rs", lib),
    ])
}

/// `cargo test` of a probe crate through the cargo running this test, with only the
/// toolchain's own environment and `env`: the runner's `RUST_BACKTRACE`, `RUST_TEST_NOCAPTURE`
/// and the like change what libtest prints.
fn live_probe_run(
    ws: &Workspace,
    target: &std::path::Path,
    env: &[(&str, &str)],
    test_args: &[&str],
) -> Vec<u8> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let toolchain_env = [
        "PATH",
        "HOME",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "TMPDIR",
    ]
    .into_iter()
    .filter_map(|key| std::env::var_os(key).map(|value| (key, value)));
    let run = std::process::Command::new(cargo)
        .args(["test", "--offline", "--no-fail-fast", "--lib", "--"])
        .args(test_args)
        .current_dir(ws.root())
        .env_clear()
        .envs(toolchain_env)
        .env("CARGO_TARGET_DIR", target)
        .envs(env.iter().copied())
        .output()
        .expect("cargo runs");
    assert_eq!(
        run.status.code(),
        Some(101),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    run.stdout
}

/// The same probe crate through the cargo running this test, so the parser is held to the
/// current toolchain's real output and not only to the recorded one; then again with
/// backtraces, which end each panic message with a trailer of their own.
#[tokio::test]
async fn diagnose_keeps_every_live_cargo_failure_to_its_own_assertion_evidence() {
    let ws = probe_workspace(PROBE_LIB);
    let target = tempfile::tempdir().expect("target dir");
    let stdout = live_probe_run(&ws, target.path(), &[], &[]);
    let report = diagnose_output(&ws, stdout, 101).await;
    assert!(report.tail.contains("  left: 4\n right: 5"));
    assert_cargo_probe_dossiers(&report);

    let stdout = live_probe_run(&ws, target.path(), &[("RUST_BACKTRACE", "1")], &[]);
    assert!(String::from_utf8_lossy(&stdout).contains("\nstack backtrace:\n"));
    let report = diagnose_output(&ws, stdout, 101).await;
    assert_cargo_probe_dossiers(&report);
}

/// Real output of `cargo test -- --test-threads=1` on this probe crate (cargo 1.97, Linux build
/// node): hand-written `Debug` values with blank lines, ended by the backtrace note (the first
/// failure), by libtest's one-line separator, or by the two before its `failures:` list.
const CARGO_BLANK_DEBUG: &str = include_str!("fixtures/assertions/cargo_test_blank_debug.txt");

const BLANK_DEBUG_LIB: &str = r#"use std::fmt;
#[derive(PartialEq)]
pub struct Doc(pub &'static str);
impl fmt::Debug for Doc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Doc {{\n\n    {}\n\n}}", self.0)
    }
}
#[derive(PartialEq)]
pub struct Text(pub &'static str);
impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Text({})", self.0)
    }
}
#[derive(PartialEq)]
pub struct Trailing(pub u8);
impl fmt::Debug for Trailing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Trailing({})\n\n", self.0)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_first_blank_lines() { assert_eq!(Doc("one"), Doc("two")); }
    #[test]
    fn b_middle_blank_lines() { assert_eq!(Doc("three"), Doc("four")); }
    #[test]
    fn c_blank_lines_with_message() { assert_eq!(Doc("x"), Doc("y"), "docs differ"); }
    #[test]
    fn d_right_only_blank_lines() { assert_eq!(Text("flat"), Text("para one\n\npara two")); }
    #[test]
    fn e_trailing_blank_lines() { assert_eq!(Trailing(1), Trailing(2)); }
    #[test]
    fn f_ne_blank_lines() { assert_ne!(Doc("same"), Doc("same")); }
    #[test]
    fn g_last_blank_lines() { assert_eq!(Doc("five"), Doc("six")); }
}
"#;

/// Blank lines inside an operand do not end it: each value is the complete one `Debug` printed.
/// A value that ends in blank lines of its own cannot be told from libtest's separators, so it
/// gets no evidence rather than a shortened value.
fn assert_blank_debug_dossiers(report: &DossierReport) {
    assert_eq!(report.tests_failed, 7);
    assert_eq!(report.dossiers.len(), 7);
    let doc = |s: &str| format!("Doc {{\n\n    {s}\n\n}}");
    let expected = [
        (
            "tests::a_first_blank_lines",
            "assert_eq",
            doc("one"),
            doc("two"),
        ),
        (
            "tests::b_middle_blank_lines",
            "assert_eq",
            doc("three"),
            doc("four"),
        ),
        (
            "tests::c_blank_lines_with_message",
            "assert_eq",
            doc("x"),
            doc("y"),
        ),
        (
            "tests::d_right_only_blank_lines",
            "assert_eq",
            "Text(flat)".to_string(),
            "Text(para one\n\npara two)".to_string(),
        ),
        (
            "tests::f_ne_blank_lines",
            "assert_ne",
            doc("same"),
            doc("same"),
        ),
        (
            "tests::g_last_blank_lines",
            "assert_eq",
            doc("five"),
            doc("six"),
        ),
    ];
    for (test, format, left, right) in &expected {
        let d = dossier_of(report, test);
        let a = d
            .assertion
            .as_ref()
            .unwrap_or_else(|| panic!("no evidence for {test}"));
        assert_eq!(a.format, *format, "{test}");
        assert_eq!(a.left.as_ref(), Some(left), "{test}");
        assert_eq!(a.right.as_ref(), Some(right), "{test}");
        assert_eq!(a.actual, None, "{test}");
        assert_eq!(a.expected, None, "{test}");
        assert_eq!(a.operands, vec![left.clone(), right.clone()], "{test}");
        assert!(d.output.contains(&a.excerpt), "{test}: {}", a.excerpt);
        assert!(a.excerpt.ends_with(right.as_str()), "{test}: {}", a.excerpt);
    }
    assert_eq!(
        dossier_of(report, "tests::e_trailing_blank_lines").assertion,
        None
    );
    let rendered = report.render();
    assert!(rendered.contains(
        "assertion [assert_eq (left == right)]: left: Doc { … (5 lines), right: Doc { … (5 lines)\n"
    ));
    assert!(!rendered.contains("actual:"));
}

#[tokio::test]
async fn diagnose_keeps_blank_lines_inside_rust_debug_operands() {
    let ws = probe_workspace(BLANK_DEBUG_LIB);
    let report = diagnose_output(&ws, CARGO_BLANK_DEBUG.as_bytes().to_vec(), 101).await;
    assert_blank_debug_dossiers(&report);
}

#[tokio::test]
async fn diagnose_keeps_blank_lines_inside_live_rust_debug_operands() {
    let ws = probe_workspace(BLANK_DEBUG_LIB);
    let target = tempfile::tempdir().expect("target dir");
    let stdout = live_probe_run(&ws, target.path(), &[], &["--test-threads=1"]);
    let report = diagnose_output(&ws, stdout, 101).await;
    assert_blank_debug_dossiers(&report);
}

/// Output cut at any line of a recorded block gives the block's complete operands or nothing.
#[test]
fn a_cut_cargo_block_gives_complete_operands_or_nothing() {
    let (_, failed, failures) = prod_code_mcp::verify::parse_cargo_test_text(CARGO_BLANK_DEBUG);
    assert_eq!(failed, 7);
    for failure in &failures {
        let full = dossier::parse_assertion_evidence(&failure.output);
        let lines: Vec<&str> = failure.output.split_inclusive('\n').collect();
        for cut in 1..lines.len() {
            let prefix = lines[..cut].concat();
            let got = dossier::parse_assertion_evidence(&prefix);
            // The one exception: a cut after one of the blank lines that end `Trailing(2)\n\n`
            // looks exactly like a block libtest closed, and nothing in it shows otherwise.
            if failure.name == "tests::e_trailing_blank_lines"
                && prefix.trim_end().ends_with(" right: Trailing(2)")
            {
                continue;
            }
            assert!(
                got.is_none() || got == full,
                "{}: {prefix:?} gave {got:?}",
                failure.name
            );
        }
    }
}

/// A node:test error-fields block cut anywhere after its opening `{` gives nothing, not the
/// short `2 !== 3` message above it.
#[test]
fn a_cut_node_error_fields_block_is_not_read_from_its_short_message() {
    let block = NODE_TEST_ERROR_FIELDS
        .split("\ntest at ")
        .skip(1)
        .find(|b| b.contains("✖ strict equal numbers ("))
        .expect("block");
    let full = dossier::parse_assertion_evidence(block).expect("complete block");
    assert_eq!(full.actual.as_deref(), Some("2"));
    let lines: Vec<&str> = block.split_inclusive('\n').collect();
    let open = lines
        .iter()
        .position(|l| l.trim_start().starts_with("at ") && l.trim_end().ends_with(" {"))
        .expect("opening brace");
    let close = lines
        .iter()
        .position(|l| l.trim_end() == "  }")
        .expect("closing brace");
    for cut in 1..=close {
        let got = dossier::parse_assertion_evidence(&lines[..cut].concat());
        if cut > open {
            assert_eq!(got, None, "cut at line {cut}");
        } else if let Some(got) = got {
            // Before the fields, only the complete short message may answer.
            assert_eq!(
                (got.actual.as_deref(), got.expected.as_deref()),
                (Some("2"), Some("3"))
            );
        }
    }
}

#[tokio::test]
async fn diagnose_keeps_every_jest_failure_to_its_own_node_assert_values() {
    let ws = Workspace::new(&[
        (
            "package.json",
            r#"{"name":"probe","devDependencies":{"jest":"^29.7.0"}}"#,
        ),
        ("jest.config.js", "module.exports = {};\n"),
        ("test/a.test.js", "test('probe', () => {});\n"),
    ]);
    let report = diagnose_output(&ws, JEST_NODE_ASSERT.as_bytes().to_vec(), 1).await;

    assert_eq!(report.tests_failed, 5);
    assert_eq!(report.dossiers.len(), 5);
    assert!(
        report
            .tail
            .contains("Expected value to strictly be equal to:")
    );
    // A thrown Error and a jest matcher failure carry no Node assert values.
    for plain in ["plain error", "jest matcher"] {
        assert_eq!(dossier_of(&report, plain).assertion, None, "{plain}");
    }
    let expected = [
        ("strict equal numbers", "strictEqual", "2", "3"),
        (
            "strict equal strings",
            "strictEqual",
            "\"left side\"",
            "\"right side\"",
        ),
        (
            "deep strict equal",
            "deepStrictEqual",
            r#"{"a": 1, "b": [2], "nested": {"expected": "x", "operator": "y"}}"#,
            r#"{"a": 1, "b": [3], "nested": {"expected": "x", "operator": "y"}}"#,
        ),
    ];
    for (test, format, actual, expected) in expected {
        let d = dossier_of(&report, test);
        let a = d
            .assertion
            .as_ref()
            .unwrap_or_else(|| panic!("no evidence for {test}"));
        assert_eq!(a.format, format, "{test}");
        assert_eq!(a.expression, None, "{test}");
        assert_eq!(a.actual.as_deref(), Some(actual), "{test}");
        assert_eq!(a.expected.as_deref(), Some(expected), "{test}");
        assert_eq!(a.left.as_deref(), Some(actual), "{test}");
        assert_eq!(a.right.as_deref(), Some(expected), "{test}");
        assert_eq!(a.operands, vec![actual, expected], "{test}");
        assert!(d.output.contains(&a.excerpt), "{test}: {}", a.excerpt);
    }
    assert!(
        report
            .render()
            .contains("assertion [strictEqual]: actual: 2, expected: 3\n")
    );
}

#[tokio::test]
async fn diagnose_takes_vitests_short_node_message_and_nothing_from_a_diff() {
    let ws = Workspace::new(&[
        (
            "package.json",
            r#"{"name":"probe","type":"module","devDependencies":{"vitest":"^2.1.0"}}"#,
        ),
        ("test/a.test.js", "test('probe', () => {});\n"),
    ]);
    let report = diagnose_output(&ws, VITEST_NODE_ASSERT.as_bytes().to_vec(), 1).await;

    assert_eq!(report.tests_failed, 3);
    assert_eq!(report.dossiers.len(), 3);
    assert_eq!(
        dossier_of(&report, "test/a.test.js > plain error").assertion,
        None
    );
    // vitest prints a deep-equal failure only as a diff, never the complete values.
    assert_eq!(
        dossier_of(&report, "test/a.test.js > deep strict equal").assertion,
        None
    );
    let a = dossier_of(&report, "test/a.test.js > strict equal numbers")
        .assertion
        .as_ref()
        .expect("short message evidence");
    assert_eq!(a.format, "strictEqual");
    assert_eq!(a.expression.as_deref(), Some("2 !== 3"));
    assert_eq!(a.actual.as_deref(), Some("2"));
    assert_eq!(a.expected.as_deref(), Some("3"));
}

#[test]
fn node_test_runner_error_fields_are_bound_at_the_top_level_only() {
    let blocks: Vec<&str> = NODE_TEST_ERROR_FIELDS.split("\ntest at ").skip(1).collect();
    assert_eq!(blocks.len(), 5);
    let parse = |name: &str| {
        let block = blocks
            .iter()
            .find(|b| b.contains(&format!("✖ {name} (")))
            .unwrap_or_else(|| panic!("no block for {name}"));
        dossier::parse_assertion_evidence(block)
    };
    // util.inspect printed `deeper: [Object]`: the values are incomplete.
    assert_eq!(parse("multiline nested"), None);
    let cases = [
        (
            "strict equal numbers",
            "strictEqual",
            Some("2 !== 3"),
            "2",
            "3",
        ),
        (
            "long strings",
            "strictEqual",
            None,
            "'abcdefghijk'",
            "'abcdefghijz'",
        ),
        ("custom message", "strictEqual", None, "1", "2"),
        (
            "deep strict equal",
            "deepStrictEqual",
            None,
            "{ a: 1, b: [ 2 ], nested: { expected: 'x', operator: 'y' } }",
            "{ a: 1, b: [ 3 ], nested: { expected: 'x', operator: 'y' } }",
        ),
    ];
    for (name, format, expression, actual, expected) in cases {
        let a = parse(name).unwrap_or_else(|| panic!("no evidence for {name}"));
        assert_eq!(a.format, format, "{name}");
        assert_eq!(a.expression.as_deref(), expression, "{name}");
        assert_eq!(a.actual.as_deref(), Some(actual), "{name}");
        assert_eq!(a.expected.as_deref(), Some(expected), "{name}");
        assert!(a.excerpt.ends_with("diff: 'simple'\n  }"), "{name}");
    }
}

#[test]
fn jest_node_assert_edge_cases_keep_complete_values_and_refuse_elided_ones() {
    let (_, failed, failures) = prod_code_mcp::verify::parse_jest_text(JEST_NODE_ASSERT_EDGE);
    assert_eq!(failed, 4);
    let parse = |name: &str| {
        let failure = failures
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no failure {name}"));
        dossier::parse_assertion_evidence(&failure.output)
    };
    // jest's maxWidth printed `10, …]`: the arrays are not complete.
    assert_eq!(parse("long array"), None);
    let cases = [
        (
            "multiline string",
            "\"line one\nline two\"",
            "\"line one\nline 2\"",
        ),
        ("custom message", "1", "2"),
        ("received label text", "\"Received:\"", "\"x\""),
    ];
    for (name, actual, expected) in cases {
        let a = parse(name).unwrap_or_else(|| panic!("no evidence for {name}"));
        assert_eq!(a.format, "strictEqual", "{name}");
        assert_eq!(a.actual.as_deref(), Some(actual), "{name}");
        assert_eq!(a.expected.as_deref(), Some(expected), "{name}");
    }
}

#[test]
fn colored_jest_output_gives_the_same_values_and_keeps_its_escapes() {
    // With `--colors` jest escapes the `●` marker too, so blocks split on the stripped text.
    let mut blocks: Vec<(String, Vec<&str>)> = Vec::new();
    for line in JEST_NODE_ASSERT_COLORS.lines() {
        let plain = dossier::strip_ansi(line);
        if let Some(name) = plain.trim().strip_prefix("● ") {
            blocks.push((name.to_string(), Vec::new()));
        } else if let Some((_, lines)) = blocks.last_mut() {
            lines.push(line);
        }
    }
    let parse = |name: &str| {
        let (_, lines) = blocks
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no block {name}"));
        dossier::parse_assertion_evidence(&lines.join("\n"))
    };
    assert_eq!(parse("plain error"), None);
    assert_eq!(parse("jest matcher"), None);
    let a = parse("strict equal numbers").expect("colored evidence");
    assert_eq!(a.actual.as_deref(), Some("2"));
    assert_eq!(a.expected.as_deref(), Some("3"));
    assert!(a.excerpt.contains("\x1b[32m3\x1b[39m"));
    assert!(a.excerpt.contains("\x1b[31m2\x1b[39m"));
}

#[tokio::test]
async fn find_dead_code_reachability_detects_circular_dead_cycle() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/main.rs",
            "fn main() {\n    active();\n}\n\nfn active() {}\n",
        ),
        (
            "src/cycle.rs",
            "fn cycle_a() {\n    cycle_b();\n}\n\nfn cycle_b() {\n    cycle_a();\n}\n",
        ),
    ]);
    let root = ws.root();
    let main_path = ws.path("src/main.rs");
    let cycle_path = ws.path("src/cycle.rs");

    let main_p = main_path.clone();
    let cycle_p = cycle_path.clone();

    let remote = ScriptedGateway::start_arc(Arc::new(move |method, params| match method {
        "textDocument/documentSymbol" => {
            let uri = params
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            if uri.contains("main.rs") {
                serde_json::json!([
                    answers::document_symbol("main", 12, 1, 3, 4),
                    answers::document_symbol("active", 12, 5, 5, 4)
                ])
            } else {
                serde_json::json!([
                    answers::document_symbol("cycle_a", 12, 1, 3, 4),
                    answers::document_symbol("cycle_b", 12, 5, 7, 4)
                ])
            }
        }
        "textDocument/references" => {
            let uri = params
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let line = params
                .pointer("/position/line")
                .and_then(|l| l.as_u64())
                .unwrap_or(u64::MAX);

            if uri.contains("main.rs") {
                if line == 0 {
                    // main: no callers
                    answers::locations(&main_p, &[])
                } else {
                    // active: called by main inside line 2
                    answers::locations(&main_p, &[(2, 5)])
                }
            } else if line == 0 {
                // cycle_a: called by cycle_b at line 6
                answers::locations(&cycle_p, &[(6, 5)])
            } else {
                // cycle_b: called by cycle_a at line 2
                answers::locations(&cycle_p, &[(2, 5)])
            }
        }
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();

    // 1. Under legacy reference counting: both cycle_a and cycle_b have reference count 1,
    // so legacy dead code scan says NEITHER is dead!
    let legacy_report = dead_code::find_dead_code_opts(
        remote,
        &root,
        dead_code::DeadCodeOptions {
            include_exported: false,
            max_files: 100,
            reachability: false,
        },
    )
    .await
    .expect("legacy scan runs");

    assert!(
        legacy_report.dead.is_empty(),
        "legacy reference counting misses the dead cycle: {:?}",
        legacy_report.dead
    );

    // 2. Under whole-program reachability analysis from entry points:
    // main() is identified as root; active() is reached from main().
    // Neither cycle_a nor cycle_b is reachable from main().
    // They are correctly detected as unreachable and clustered as a circular dead cycle!
    let reach_report = dead_code::find_dead_code_opts(
        remote,
        &root,
        dead_code::DeadCodeOptions {
            include_exported: false,
            max_files: 100,
            reachability: true,
        },
    )
    .await
    .expect("reachability scan runs");

    let dead_names: Vec<&str> = reach_report.dead.iter().map(|d| d.name.as_str()).collect();
    assert!(dead_names.contains(&"cycle_a"), "cycle_a is dead: {dead_names:?}");
    assert!(dead_names.contains(&"cycle_b"), "cycle_b is dead: {dead_names:?}");
    assert!(!dead_names.contains(&"main"), "main is entry point: {dead_names:?}");
    assert!(!dead_names.contains(&"active"), "active is reached: {dead_names:?}");

    let summary = reach_report.reachability.as_ref().expect("reachability summary exists");
    assert_eq!(summary.roots_count, 1); // main
    assert_eq!(summary.reachable_count, 2); // main, active
    assert_eq!(summary.unreachable_count, 2); // cycle_a, cycle_b
    assert_eq!(summary.cluster_count, 1);

    assert_eq!(reach_report.unreachable_clusters.len(), 1);
    let cluster = &reach_report.unreachable_clusters[0];
    assert!(cluster.cycle, "cycle must be detected");
    assert_eq!(cluster.symbols.len(), 2);
    assert_eq!(cluster.internal_calls.len(), 2);

    let rendered = reach_report.render();
    assert!(rendered.contains("whole-program reachability scan"));
    assert!(rendered.contains("cycle detected"));
}

#[tokio::test]
async fn reachability_preserves_unverified_symbols_under_contract_435() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            "fn unverified_fn() {\n    helper_fn();\n}\n\nfn helper_fn() {}\n",
        ),
    ]);
    let root = ws.root();
    let lib_p = ws.path("src/lib.rs");

    let remote = ScriptedGateway::start_arc(Arc::new(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("unverified_fn", 12, 1, 3, 4),
            answers::document_symbol("helper_fn", 12, 5, 5, 4)
        ]),
        "textDocument/references" => {
            let line = params
                .pointer("/position/line")
                .and_then(|l| l.as_u64())
                .unwrap_or(u64::MAX);

            if line == 0 {
                // Analyzer fails for unverified_fn!
                serde_json::Value::Null
            } else {
                // helper_fn called by unverified_fn
                answers::locations(&lib_p, &[(2, 5)])
            }
        }
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();

    let report = dead_code::find_dead_code_opts(
        remote,
        &root,
        dead_code::DeadCodeOptions {
            include_exported: false,
            max_files: 100,
            reachability: true,
        },
    )
    .await
    .expect("scan runs");

    // helper_fn is reachable from unverified_fn, which is conservatively protected.
    // Therefore, NEITHER is marked dead.
    assert!(report.dead.is_empty(), "unverified dependencies are preserved: {:?}", report.dead);
    assert_eq!(report.unverified.len(), 1);
    assert_eq!(report.unverified[0].name.as_deref(), Some("unverified_fn"));
}

/// In a mixed Go/JavaScript repository, JavaScript edits (including arrow functions and nested
/// declarations) are correctly attributed to their functions rather than treated as edits outside
/// any function (#800).
#[tokio::test]
async fn analyze_mixed_repo_attributes_javascript_functions_and_nested_declarations() {
    let ws = Workspace::new(&[
        ("go.mod", "module example.com/mixed\ngo 1.22\n"),
        (
            "extension/background.js",
            "export async function handleTabCreated(tab) {\n    function addChildTabHandoff(child) {\n        return child.id;\n    }\n    return addChildTabHandoff(tab);\n}\n\nexport const routeMethod = (req) => {\n    return req.method;\n};\n",
        ),
    ]);
    let root = ws.root();
    // Edit inside handleTabCreated (and its nested addChildTabHandoff) and routeMethod
    ws.write(
        "extension/background.js",
        "export async function handleTabCreated(tab) {\n    function addChildTabHandoff(child) {\n        return child.id + 1;\n    }\n    return addChildTabHandoff(tab) || null;\n}\n\nexport const routeMethod = (req) => {\n    return req.method.toLowerCase();\n};\n",
    );

    let js_uri = prod_code_protocol::path::file_uri(ws.path("extension/background.js").as_path());
    let remote = ScriptedGateway::start_arc(Arc::new(move |method, params| match method {
        "textDocument/documentSymbol" => {
            let uri = params.pointer("/textDocument/uri").and_then(|u| u.as_str()).unwrap_or("");
            if uri == js_uri {
                serde_json::json!([
                    {
                        "name": "handleTabCreated",
                        "kind": 12,
                        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 5, "character": 1 } },
                        "selectionRange": { "start": { "line": 0, "character": 22 }, "end": { "line": 0, "character": 38 } },
                        "children": [
                            {
                                "name": "addChildTabHandoff",
                                "kind": 12,
                                "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 3, "character": 5 } },
                                "selectionRange": { "start": { "line": 1, "character": 13 }, "end": { "line": 1, "character": 31 } }
                            }
                        ]
                    },
                    {
                        "name": "routeMethod",
                        "kind": 14,
                        "detail": "(req: any) => any",
                        "range": { "start": { "line": 7, "character": 0 }, "end": { "line": 9, "character": 2 } },
                        "selectionRange": { "start": { "line": 7, "character": 13 }, "end": { "line": 7, "character": 24 } }
                    }
                ])
            } else {
                serde_json::Value::Array(vec![])
            }
        }
        "textDocument/prepareCallHierarchy" => {
            serde_json::json!([{
                "name": "mock",
                "kind": 12,
                "uri": js_uri,
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
            }])
        }
        "callHierarchy/incomingCalls" => {
            serde_json::json!([])
        }
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();

    let report = impact::analyze(remote, &root, None, 2)
        .await
        .expect("analysis runs");

    assert_eq!(report.language, "go");
    assert_eq!(report.changed_files, vec!["extension/background.js".to_string()]);
    let changed_names: Vec<&str> = report.changed.iter().map(|s| s.name.as_str()).collect();
    assert!(
        changed_names.contains(&"handleTabCreated"),
        "expected handleTabCreated in changed: {:?}",
        changed_names
    );
    assert!(
        changed_names.contains(&"addChildTabHandoff"),
        "expected nested addChildTabHandoff in changed: {:?}",
        changed_names
    );
    assert!(
        changed_names.contains(&"routeMethod"),
        "expected routeMethod in changed: {:?}",
        changed_names
    );
    assert!(
        report.unattributed_files.is_empty(),
        "unattributed files must be empty but was: {:?}",
        report.unattributed_files
    );
    assert_eq!(report.full_suite_reason(), None);
}

/// In a mixed Go/JavaScript repository, when the language server for JavaScript is unavailable,
/// the report records an explicit incomplete Gap and does not claim lines changed outside functions (#800).
#[tokio::test]
async fn analyze_mixed_repo_handles_unsupported_language_without_claiming_outside_functions() {
    let ws = Workspace::new(&[
        ("go.mod", "module example.com/mixed\ngo 1.22\n"),
        (
            "extension/background.js",
            "export function handleTabCreated(tab) {\n    return tab.id;\n}\n",
        ),
    ]);
    let root = ws.root();
    ws.write(
        "extension/background.js",
        "export function handleTabCreated(tab) {\n    return tab.id + 1;\n}\n",
    );

    // Custom gateway that refuses handshake when preferred_engine is typescript
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(msg)) = framed.next().await {
                    match msg {
                        WireMessage::SyncProbeRequest(req) => {
                            let _ = framed.send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                                server_workspace_root: req.client_workspace_root,
                                seeded: false,
                                files_deleted: 0,
                                missing: Vec::new(),
                            })).await;
                        }
                        WireMessage::SyncRequest(req) => {
                            let _ = framed.send(WireMessage::SyncResponse(SyncResponse {
                                server_workspace_root: req.client_workspace_root,
                                files_updated: 0,
                                files_deleted: 0,
                                bytes_transferred: 0,
                                duration_ms: 0,
                                workspace_was_fresh: false,
                                stale_paths: Vec::new(),
                            })).await;
                        }
                        WireMessage::HandshakeRequest(req) => {
                            if req.preferred_engine.as_deref() == Some("typescript") {
                                let _ = framed.send(WireMessage::Disconnect {
                                    reason: "engine typescript is not served by this node".to_string(),
                                }).await;
                                return;
                            }
                            let _ = framed.send(WireMessage::HandshakeResponse(HandshakeResponse {
                                protocol_version: PROTOCOL_VERSION,
                                server_pid: std::process::id(),
                                session_id: 1,
                                server_workspace_root: req.client_workspace_root,
                                detected_engine: "go".to_string(),
                                stale_paths: Vec::new(),
                                engine_age_ms: None,
                                index_gated: false,
                                capabilities: None,
                            })).await;
                        }
                        _ => {}
                    }
                }
            });
        }
    });

    let report = impact::analyze(addr, &root, None, 2)
        .await
        .expect("analysis runs");

    assert_eq!(report.language, "go");
    assert!(
        report.unattributed_files.is_empty(),
        "unattributed files must be empty but was: {:?}",
        report.unattributed_files
    );
    assert_eq!(report.incomplete.len(), 1);
    match &report.incomplete[0] {
        Gap::Symbols { file, error } => {
            assert_eq!(file, "extension/background.js");
            assert!(
                error.contains("refused") || error.contains("engine typescript is not served") || error.contains("closed"),
                "expected engine error in gap: {error}"
            );
        }
        other => panic!("expected Gap::Symbols, got {other:?}"),
    }
    let reason = report.full_suite_reason().expect("reason exists");
    assert!(
        reason.contains("incomplete"),
        "expected incomplete analysis in reason: {reason}"
    );
    assert!(
        !reason.contains("lines changed outside any function"),
        "must NOT claim lines changed outside functions: {reason}"
    );
}
