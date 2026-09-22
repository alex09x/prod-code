//! The orchestration that needs a real gateway round trip: `slice` walking dependency edges
//! across files, `fixture` resolving and building nested types, `run_shadow`/`run_verify`
//! talking to a remote exec, and the cluster's node-selection network calls.
//!
//! `slice`, `fixture` and the diagnostics they use speak the LSP wire protocol, so
//! [`prod_code_testkit::ScriptedGateway`] (a real TCP server that answers each method from a
//! closure) is enough. `shadow::run_shadow`, `verify::run_verify` and the `cluster` probes
//! speak other messages of the same wire protocol (`ShadowRunRequest`, `ExecRequest`,
//! `ClusterRequest`, ...) that the testkit's gateway does not answer, so those tests drive a
//! small hand-rolled server that speaks exactly the messages each call sends.

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::{cluster, fixture, shadow, slice, verify};
use prod_code_protocol::{
    ClusterResponse, ExecChunk, ExecExit, ExecRequest, MetricsResponse, PeerInfo, PlaceResponse,
    ProdCodeCodec, ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse, StatusResponse,
    SyncProbeResponse, WireMessage,
};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

fn sample_status() -> StatusResponse {
    StatusResponse {
        server_pid: 4242,
        uptime_seconds: 10,
        active_sessions: 1,
        loaded_workspaces: 1,
        detected_engines: vec!["rust (ra_ap_ide)".to_string()],
        memory_rss_bytes: Some(1024),
        total_queries: 5,
        active_queries: 0,
        load_average_millis: Some(800),
        cpu_count: Some(8),
    }
}

// ===== slice: the dependency walk across files (needs documentSymbol + definition) =====

/// The whole point of the slicer: a dependency in another file is pulled in and grouped under
/// its own file, a name the body references but never defines locally is dropped, a name that
/// resolves back into the item itself is a local rather than a new item, and a name outside the
/// workspace is reported rather than followed.
#[tokio::test]
async fn slice_follows_dependencies_across_files_marks_locals_and_reports_external_names() {
    let source = "pub fn run(cfg: &Config) -> u32 {\n    log(cfg.count);\n    helper(cfg.count)\n}\n\nfn helper(count: u32) -> u32 {\n    count + 1\n}\n";
    let ws = Workspace::new(&[
        ("src/lib.rs", source),
        (
            "src/config.rs",
            "pub struct Config {\n    pub count: u32,\n}\n",
        ),
    ]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");
    let config = root.join("src/config.rs");

    let (lib_uri, config_uri) = (lib.clone(), config.clone());
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => {
            let uri = params
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            if uri.ends_with("lib.rs") {
                serde_json::json!([
                    answers::document_symbol("run", 12, 1, 4, 8),
                    answers::document_symbol("helper", 12, 6, 8, 4),
                ])
            } else {
                serde_json::json!([answers::document_symbol("Config", 23, 1, 3, 12)])
            }
        }
        "textDocument/definition" => {
            let uri = params
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let line = params.pointer("/position/line").and_then(|l| l.as_u64());
            let ch = params
                .pointer("/position/character")
                .and_then(|c| c.as_u64());
            if uri.ends_with("lib.rs") && line == Some(0) && ch == Some(17) {
                // `Config` on the signature line.
                serde_json::json!([{
                    "uri": format!("file://{}", config_uri.display()),
                    "range": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 17 } }
                }])
            } else if uri.ends_with("lib.rs") && line == Some(1) && ch == Some(4) {
                // `log`: outside the workspace entirely.
                serde_json::json!([{
                    "uri": "file:///usr/lib/rust/std/log.rs",
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }
                }])
            } else if uri.ends_with("lib.rs") && line == Some(2) && ch == Some(4) {
                // `helper`, declared lower in the same file.
                serde_json::json!([{
                    "uri": format!("file://{}", lib_uri.display()),
                    "range": { "start": { "line": 5, "character": 3 }, "end": { "line": 5, "character": 9 } }
                }])
            } else if uri.ends_with("config.rs") && line == Some(0) && ch == Some(11) {
                // `Config` inside its own declaration: resolves to itself.
                serde_json::json!([{
                    "uri": format!("file://{}", config_uri.display()),
                    "range": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 17 } }
                }])
            } else {
                serde_json::Value::Null
            }
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(
        gateway.addr(),
        &root,
        &lib,
        1,
        8,
        2,
        slice::DEFAULT_MAX_BYTES,
    )
    .await
    .expect("the slice runs");

    assert_eq!(report.seed, "run");
    assert_eq!(report.items.len(), 3, "{:?}", report.items);
    let config_item = report
        .items
        .iter()
        .find(|i| i.name == "Config")
        .expect("Config was pulled in");
    assert_eq!(config_item.file, "src/config.rs");
    assert_eq!(config_item.depth, 1);
    assert_eq!(config_item.because.as_deref(), Some("run"));
    let helper_item = report
        .items
        .iter()
        .find(|i| i.name == "helper")
        .expect("helper was pulled in");
    assert_eq!(helper_item.file, "src/lib.rs");
    assert_eq!(helper_item.depth, 1);
    assert_eq!(
        report.external,
        vec!["log".to_string()],
        "log resolved outside the workspace and was not followed"
    );
    assert_eq!(report.truncated, 0);

    let text = report.render();
    assert!(text.contains("=== src/lib.rs"), "{text}");
    assert!(text.contains("=== src/config.rs"), "{text}");
    assert!(text.contains("(the seed)"), "{text}");
    assert!(text.contains("used by run"), "{text}");
    assert!(
        text.contains("outside the workspace, not followed: log"),
        "{text}"
    );
}

/// At depth 0 the seed is returned alone, and the slicer never asks the analyzer what its body
/// depends on: only the one `documentSymbol` call that finds the seed's own declaration.
#[tokio::test]
async fn slice_at_depth_zero_returns_only_the_seed_without_asking_for_its_dependencies() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn run() -> u32 {\n    1\n}\n")]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");

    let gateway = ScriptedGateway::start(|method, _params| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("run", 12, 1, 3, 8)])
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(
        gateway.addr(),
        &root,
        &lib,
        1,
        8,
        0,
        slice::DEFAULT_MAX_BYTES,
    )
    .await
    .expect("the slice runs");

    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].because, None);
    assert_eq!(
        gateway.calls(),
        1,
        "no definition query was made at depth 0"
    );
}

/// A budget too small for the second item stops the walk and counts what was left out, without
/// dropping what already fit.
#[tokio::test]
async fn slice_stops_at_the_byte_budget_and_counts_what_it_left_out() {
    let seed = "pub fn run(cfg: &Config) -> u32 {\n    helper(cfg.count)\n}";
    let source = format!("{seed}\n\npub struct Config {{\n    pub count: u32,\n}}\n");
    let ws = Workspace::new(&[("src/lib.rs", source.as_str())]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");

    let lib_for_mock = lib.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("run", 12, 1, 3, 8),
            answers::document_symbol("Config", 23, 5, 7, 12),
        ]),
        "textDocument/definition" => {
            let line = params.pointer("/position/line").and_then(|l| l.as_u64());
            let ch = params
                .pointer("/position/character")
                .and_then(|c| c.as_u64());
            if line == Some(0) && ch == Some(17) {
                serde_json::json!([{
                    "uri": format!("file://{}", lib_for_mock.display()),
                    "range": { "start": { "line": 4, "character": 11 }, "end": { "line": 4, "character": 17 } }
                }])
            } else {
                serde_json::Value::Null
            }
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &lib, 1, 8, 2, seed.len())
        .await
        .expect("the slice runs");

    assert_eq!(
        report.items.len(),
        1,
        "the seed fits the budget, the dependency does not"
    );
    assert_eq!(report.truncated, 1);
    assert!(
        report.render().contains("further item(s) omitted"),
        "{}",
        report.render()
    );
}

/// A position that lands outside every declaration is reported rather than silently sliced as
/// an empty result.
#[tokio::test]
async fn slice_reports_when_there_is_no_declaration_at_the_seed_position() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn run() {}\n")]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");

    let gateway = ScriptedGateway::start(|method, _params| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("run", 12, 1, 1, 8)])
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let err = slice::slice(
        gateway.addr(),
        &root,
        &lib,
        50,
        1,
        2,
        slice::DEFAULT_MAX_BYTES,
    )
    .await
    .expect_err("line 50 has no declaration");
    assert!(format!("{err:#}").contains("no declaration at"));
}

// ===== fixture: resolving a type and building its value (needs workspace/symbol + documentSymbol) =====

/// The generator recurses into a nested workspace type, unwraps `Box<T>`, uses an enum's first
/// variant, builds a tuple struct positionally and falls back to `Default::default()` for a
/// type it cannot find — while a unit struct at the top level is just its own name.
#[tokio::test]
async fn build_literal_recurses_into_nested_types_wrappers_and_enums_and_falls_back_when_unresolvable()
 {
    let source = "pub struct Config {\n    pub name: String,\n    pub engine: Engine,\n    pub status: Status,\n    pub point: Point,\n    pub marker: Marker,\n    pub boxed: Box<Engine>,\n    pub other: Other,\n}\n\npub struct Engine {\n    pub threads: u32,\n}\n\npub enum Status {\n    Idle,\n    Running,\n}\n\npub struct Point(pub u32, pub u32);\n\npub struct Marker;\n";
    let ws = Workspace::new(&[("src/lib.rs", source)]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");

    let path = lib.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => {
            let query = params.get("query").and_then(|q| q.as_str()).unwrap_or("");
            match query {
                "Config" => serde_json::json!([answers::symbol("Config", 23, &path, 1, 12)]),
                "Engine" => serde_json::json!([answers::symbol("Engine", 23, &path, 11, 12)]),
                "Status" => serde_json::json!([answers::symbol("Status", 10, &path, 15, 10)]),
                "Point" => serde_json::json!([answers::symbol("Point", 23, &path, 20, 12)]),
                "Marker" => serde_json::json!([answers::symbol("Marker", 23, &path, 22, 12)]),
                _ => serde_json::json!([]),
            }
        }
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("Config", 23, 1, 9, 12),
            answers::document_symbol("Engine", 23, 11, 13, 12),
            answers::document_symbol("Status", 10, 15, 18, 10),
            answers::document_symbol("Point", 23, 20, 20, 12),
            answers::document_symbol("Marker", 23, 22, 22, 12),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;

    let fixture = fixture::generate(gateway.addr(), &root, "Config", 2, false, None)
        .await
        .expect("the fixture is built");

    assert!(
        fixture.value.contains("name: String::new(),"),
        "{}",
        fixture.value
    );
    assert!(
        fixture.value.contains("engine: Engine {"),
        "{}",
        fixture.value
    );
    assert!(fixture.value.contains("threads: 0,"), "{}", fixture.value);
    assert!(
        fixture.value.contains("status: Status::Idle,"),
        "{}",
        fixture.value
    );
    assert!(
        fixture.value.contains("point: Point(0, 0),"),
        "{}",
        fixture.value
    );
    assert!(
        fixture.value.contains("marker: Marker,"),
        "{}",
        fixture.value
    );
    assert!(
        fixture.value.contains("boxed: Box::new(Engine {"),
        "{}",
        fixture.value
    );
    assert!(
        fixture.value.contains("other: Default::default(),"),
        "{}",
        fixture.value
    );
    assert_eq!(fixture.fallbacks, vec!["Other".to_string()]);
    assert_eq!(fixture.file, "src/lib.rs");
    assert!(!fixture.verified);

    let marker = fixture::generate(gateway.addr(), &root, "Marker", 1, false, None)
        .await
        .expect("a unit struct is generated directly");
    assert_eq!(marker.value, "Marker");
}

/// A hint picks the declaration in that file; without one, two types of the same name are
/// reported together rather than one being picked arbitrarily.
#[tokio::test]
async fn resolve_type_prefers_the_hint_and_lists_every_file_without_it() {
    let ws = Workspace::new(&[
        ("a.rs", "pub struct Point {\n    pub x: i32,\n}\n"),
        ("b.rs", "pub struct Point {\n    pub y: i32,\n}\n"),
    ]);
    let root = ws.root();
    let a = root.join("a.rs");
    let b = root.join("b.rs");

    let (a_uri, b_uri) = (a.clone(), b.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("Point", 23, &a_uri, 1, 12),
            answers::symbol("Point", 23, &b_uri, 1, 12),
        ]),
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("Point", 23, 1, 3, 12)])
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let fixture_b = fixture::generate(gateway.addr(), &root, "Point", 1, false, Some(b.as_path()))
        .await
        .expect("the hint disambiguates");
    assert_eq!(fixture_b.file, "b.rs");
    assert!(fixture_b.value.contains("y: 0"), "{}", fixture_b.value);

    let err = fixture::generate(gateway.addr(), &root, "Point", 1, false, None)
        .await
        .expect_err("without a hint the two declarations are ambiguous");
    let text = format!("{err:#}");
    assert!(text.contains("declared in more than one file"), "{text}");
    assert!(text.contains("a.rs") && text.contains("b.rs"), "{text}");
}

/// `verify: false` never asks the analyzer, and `verify: true` reports exactly what it rejected
/// rather than pretending the fixture is usable.
#[tokio::test]
async fn generate_is_unverified_without_verify_and_reports_the_analyzers_rejection_with_it() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "pub struct Config {\n    pub retries: u32,\n}\n",
    )]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");

    let path = lib.clone();
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => serde_json::json!([answers::symbol("Config", 23, &path, 1, 12)]),
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("Config", 23, 1, 3, 12)])
        }
        "textDocument/diagnostic" => {
            answers::error_at(6, 9, "E0425", "cannot find value `q` in this scope")
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let unverified = fixture::generate(gateway.addr(), &root, "Config", 1, false, None)
        .await
        .expect("generation without verification still succeeds");
    assert!(!unverified.verified);
    assert!(unverified.render().contains("not verified"));

    let rejected = fixture::generate(gateway.addr(), &root, "Config", 1, true, None)
        .await
        .expect("generation runs even when the analyzer rejects the result");
    assert!(rejected.verified);
    assert_eq!(rejected.diagnostics.len(), 1, "{:?}", rejected.diagnostics);
    assert!(rejected.diagnostics[0].contains("E0425"));
    assert!(rejected.render().contains("the analyzer rejects it"));
}

// ===== shadow: run_shadow talks ShadowRunRequest/ShadowRunResponse, not LSP =====

/// Answers exactly one `ShadowRunRequest` after the pre-flight sync probe, like the gateway
/// does; the probe always claims the workspace is already held so no upload follows, matching
/// `ScriptedGateway`'s own behaviour.
async fn shadow_gateway(
    respond: impl Fn(&ShadowRunRequest) -> WireMessage + Send + 'static,
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        let Some(Ok(WireMessage::SyncProbeRequest(probe))) = framed.next().await else {
            return;
        };
        let _ = framed
            .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                server_workspace_root: probe.client_workspace_root,
                seeded: false,
                files_deleted: 0,
                missing: Vec::new(),
            }))
            .await;
        let Some(Ok(WireMessage::ShadowRunRequest(req))) = framed.next().await else {
            return;
        };
        let _ = framed.send(respond(&req)).await;
        let _ = framed.next().await;
    });
    addr
}

/// Sends whatever messages the test gives it after the `ShadowRunRequest`, then closes: for the
/// error paths `run_shadow` takes when the gateway misbehaves.
async fn shadow_gateway_send(messages: Vec<WireMessage>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        let Some(Ok(WireMessage::SyncProbeRequest(probe))) = framed.next().await else {
            return;
        };
        let _ = framed
            .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                server_workspace_root: probe.client_workspace_root,
                seeded: false,
                files_deleted: 0,
                missing: Vec::new(),
            }))
            .await;
        let Some(Ok(WireMessage::ShadowRunRequest(_))) = framed.next().await else {
            return;
        };
        for message in messages {
            let _ = framed.send(message).await;
        }
    });
    addr
}

fn hypothesis(name: &str, path: &str, text: &str) -> shadow::HypothesisSpec {
    shadow::HypothesisSpec {
        name: name.to_string(),
        edits: vec![shadow::HypothesisEdit {
            relative_path: path.to_string(),
            text: Some(text.to_string()),
        }],
    }
}

/// A real round trip: two hypotheses go out, the gateway's per-hypothesis results come back,
/// and `run_shadow` ranks them, computes each one's diff against the checkout and names a
/// winner — the same report the CLI and the MCP tool print.
#[tokio::test]
async fn run_shadow_ranks_hypotheses_diffs_them_against_the_checkout_and_names_the_winner() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() -> u32 { 1 }\n")]);
    let root = ws.root();

    let addr = shadow_gateway(|req| {
        assert_eq!(req.hypotheses.len(), 2);
        WireMessage::ShadowRunResponse(ShadowRunResponse {
            server_workspace_root: "/srv/ws/demo".to_string(),
            mode: "overlay".to_string(),
            error: None,
            results: vec![
                ShadowHypothesisResult {
                    name: "ok".to_string(),
                    exit_code: Some(0),
                    duration_ms: 120,
                    timed_out: false,
                    error: None,
                    output_tail: Some(
                        b"test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
                            .to_vec(),
                    ),
                    output_len: 70,
                },
                ShadowHypothesisResult {
                    name: "broken".to_string(),
                    exit_code: Some(101),
                    duration_ms: 90,
                    timed_out: false,
                    error: None,
                    output_tail: Some(
                        b"test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n"
                            .to_vec(),
                    ),
                    output_len: 80,
                },
            ],
        })
    })
    .await;

    let specs = vec![
        hypothesis("ok", "src/lib.rs", "pub fn a() -> u32 { 2 }\n"),
        hypothesis("broken", "src/lib.rs", "pub fn a() -> u32 { 3 }\n"),
    ];
    let command = vec!["cargo".to_string(), "test".to_string()];
    let outcome = shadow::run_shadow(
        addr,
        &root,
        None,
        &specs,
        command.clone(),
        Vec::new(),
        30,
        2,
        4096,
    )
    .await
    .expect("the run completes");

    assert_eq!(outcome.mode, "overlay");
    assert_eq!(outcome.server_workspace_root, "/srv/ws/demo");
    assert_eq!(outcome.results.len(), 2);
    let winner = &outcome.results[outcome.winner.expect("ok passed")];
    assert_eq!(winner.name, "ok");
    assert_eq!(winner.tests, Some((3, 0)));
    assert!(
        winner.diff.contains("-pub fn a() -> u32 { 1 }"),
        "{}",
        winner.diff
    );

    let report = shadow::render_report(&outcome, &command, None, 200);
    assert!(report.contains("<- winner"), "{report}");
    assert!(report.contains("winner: ok"), "{report}");
}

/// The gateway can refuse a run outright (workspace not synced, empty command on its side); the
/// refusal reaches the caller as an error, not as an empty result set.
#[tokio::test]
async fn run_shadow_surfaces_a_refusal_from_the_gateway() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let addr = shadow_gateway(|_req| {
        WireMessage::ShadowRunResponse(ShadowRunResponse {
            server_workspace_root: String::new(),
            mode: String::new(),
            error: Some("workspace not synced".to_string()),
            results: Vec::new(),
        })
    })
    .await;

    let specs = vec![hypothesis("h", "src/lib.rs", "pub fn a() { 1 }\n")];
    let err = shadow::run_shadow(
        addr,
        &root,
        None,
        &specs,
        vec!["true".to_string()],
        Vec::new(),
        10,
        1,
        100,
    )
    .await
    .expect_err("a refusal is an error");
    assert!(format!("{err:#}").contains("workspace not synced"));
}

/// A message of a shape `run_shadow` never expects is reported by name rather than silently
/// misinterpreted.
#[tokio::test]
async fn run_shadow_rejects_a_message_it_does_not_expect() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let addr = shadow_gateway_send(vec![WireMessage::StatusResponse(sample_status())]).await;

    let specs = vec![hypothesis("h", "src/lib.rs", "pub fn a() { 1 }\n")];
    let err = shadow::run_shadow(
        addr,
        &root,
        None,
        &specs,
        vec!["true".to_string()],
        Vec::new(),
        10,
        1,
        100,
    )
    .await
    .expect_err("a status response is not a shadow run response");
    assert!(format!("{err:#}").contains("unexpected message"));
}

/// A `Pong` in between is ignored (the keepalive both sides may send), but the connection
/// closing before an answer arrives is a clear error, not a hang.
#[tokio::test]
async fn run_shadow_errors_when_the_gateway_closes_without_answering() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let addr = shadow_gateway_send(vec![WireMessage::Pong]).await;

    let specs = vec![hypothesis("h", "src/lib.rs", "pub fn a() { 1 }\n")];
    let err = shadow::run_shadow(
        addr,
        &root,
        None,
        &specs,
        vec!["true".to_string()],
        Vec::new(),
        10,
        1,
        100,
    )
    .await
    .expect_err("no response ever arrives");
    assert!(format!("{err:#}").contains("closed the connection"));
}

// ===== verify: run_verify talks ExecRequest/ExecChunk/ExecExit =====

/// Answers one `ExecRequest` after the pre-flight sync probe with the given stdout, stderr and
/// exit code; `fake_root`, when given, stands in for the gateway's own workspace path (what
/// diagnostics from an absolute-path tool get relativized against).
async fn exec_gateway(
    fake_root: Option<&'static str>,
    handler: impl Fn(&ExecRequest) -> (Vec<u8>, Vec<u8>, Option<i32>) + Send + 'static,
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        let Some(Ok(WireMessage::SyncProbeRequest(probe))) = framed.next().await else {
            return;
        };
        let _ = framed
            .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                server_workspace_root: probe.client_workspace_root.clone(),
                seeded: false,
                files_deleted: 0,
                missing: Vec::new(),
            }))
            .await;
        let Some(Ok(WireMessage::ExecRequest(req))) = framed.next().await else {
            return;
        };
        let (stdout, stderr, exit_code) = handler(&req);
        if !stdout.is_empty() {
            let _ = framed
                .send(WireMessage::ExecChunk(ExecChunk {
                    stderr: false,
                    data: Some(stdout),
                }))
                .await;
        }
        if !stderr.is_empty() {
            let _ = framed
                .send(WireMessage::ExecChunk(ExecChunk {
                    stderr: true,
                    data: Some(stderr),
                }))
                .await;
        }
        let server_workspace_root = fake_root
            .map(str::to_string)
            .unwrap_or(probe.client_workspace_root);
        let _ = framed
            .send(WireMessage::ExecExit(ExecExit {
                exit_code,
                duration_ms: 7,
                server_workspace_root,
                timed_out: false,
                error: None,
            }))
            .await;
        let _ = framed.next().await;
    });
    addr
}

/// `cargo test` output is parsed into pass/fail counts and the failing test's own output, a
/// warning on stderr becomes a diagnostic, and a hint inside a workspace member narrows the
/// command from `--workspace` to `-p <crate>`.
#[tokio::test]
async fn run_verify_runs_cargo_test_and_narrows_to_the_crate() {
    let ws = Workspace::new(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/demo\"]\n"),
        (
            "crates/demo/Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "crates/demo/src/lib.rs",
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        ),
    ]);
    let root = ws.root();
    let lib = root.join("crates/demo/src/lib.rs");

    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let stdout = b"running 3 tests\ntest a::ok1 ... ok\ntest a::ok2 ... ok\ntest a::bad ... FAILED\n\nfailures:\n\n---- a::bad stdout ----\nthread 'a::bad' panicked at src/lib.rs:5:9:\nassertion failed\n\nfailures:\n    a::bad\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n".to_vec();
    let stderr = b"warning: unused variable: `x`\n --> src/main.rs:4:9\n".to_vec();
    let addr = exec_gateway(None, move |req| {
        *seen2.lock().unwrap() = req.command.clone();
        (stdout.clone(), stderr.clone(), Some(101))
    })
    .await;

    let report = verify::run_verify(
        addr,
        &root,
        Some(lib.as_path()),
        verify::VerifyKind::Test,
        None,
        30,
    )
    .await
    .expect("the verify runs");

    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            "cargo".to_string(),
            "test".to_string(),
            "-p".to_string(),
            "demo".to_string()
        ]
    );
    assert_eq!(report.language, "rust");
    assert_eq!((report.tests_passed, report.tests_failed), (2, 1));
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].name, "a::bad");
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].level, "warning");
    assert!(!report.ok());
    assert!(
        report.summary().contains("2 passed, 1 failed"),
        "{}",
        report.summary()
    );
}

/// `cargo check --message-format=json` diagnostics are parsed straight off stdout.
#[tokio::test]
async fn run_verify_runs_cargo_check_and_parses_json_diagnostics() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub fn a() -> i32 { x }\n"),
    ]);
    let root = ws.root();
    let stdout = br#"{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0425"},"message":"cannot find value `x` in this scope","spans":[{"file_name":"src/lib.rs","line_start":1,"column_start":21,"is_primary":true}]}}
{"reason":"build-finished","success":false}
"#
    .to_vec();
    let addr = exec_gateway(None, move |_req| (stdout.clone(), Vec::new(), Some(101))).await;

    let report = verify::run_verify(addr, &root, None, verify::VerifyKind::Check, None, 30)
        .await
        .expect("the verify runs");
    assert_eq!(report.language, "rust");
    assert_eq!(report.errors(), 1);
    assert_eq!(report.diagnostics[0].code.as_deref(), Some("E0425"));
    assert!(!report.ok());
}

/// `go test -json` events are parsed into pass/fail counts and per-test output.
#[tokio::test]
async fn run_verify_runs_go_test_and_reports_pass_fail_counts() {
    let ws = Workspace::new(&[
        ("go.mod", "module example.com/demo\n\ngo 1.22\n"),
        ("main.go", "package main\n\nfunc main() {}\n"),
    ]);
    let root = ws.root();
    let stdout = b"{\"Action\":\"run\",\"Package\":\"p\",\"Test\":\"TestA\"}\n{\"Action\":\"output\",\"Package\":\"p\",\"Test\":\"TestA\",\"Output\":\"    a_test.go:7: boom\\n\"}\n{\"Action\":\"fail\",\"Package\":\"p\",\"Test\":\"TestA\"}\n{\"Action\":\"pass\",\"Package\":\"p\",\"Test\":\"TestB\"}\n".to_vec();
    let addr = exec_gateway(None, move |_req| (stdout.clone(), Vec::new(), Some(1))).await;

    let report = verify::run_verify(addr, &root, None, verify::VerifyKind::Test, None, 30)
        .await
        .expect("the verify runs");
    assert_eq!(report.language, "go");
    assert_eq!((report.tests_passed, report.tests_failed), (1, 1));
    assert_eq!(report.failures[0].name, "p.TestA");
}

/// `basedpyright --outputjson` diagnostics are parsed off stdout.
#[tokio::test]
async fn run_verify_runs_python_check_with_pyright_json() {
    let ws = Workspace::new(&[
        ("pyproject.toml", "[tool.pytest.ini_options]\n"),
        ("app.py", "x = 1\n"),
    ]);
    let root = ws.root();
    let stdout = br#"{"generalDiagnostics":[{"file":"/w/app.py","severity":"error","message":"boom","range":{"start":{"line":0,"character":0}},"rule":"reportGeneralTypeIssues"}],"summary":{}}"#
        .to_vec();
    let addr = exec_gateway(None, move |_req| (stdout.clone(), Vec::new(), Some(1))).await;

    let report = verify::run_verify(addr, &root, None, verify::VerifyKind::Check, None, 30)
        .await
        .expect("the verify runs");
    assert_eq!(report.language, "python");
    assert_eq!(report.errors(), 1);
}

/// A configured vitest runner's summary line is parsed into pass/fail counts.
#[tokio::test]
async fn run_verify_runs_typescript_vitest_tests() {
    let ws = Workspace::new(&[
        (
            "package.json",
            r#"{"name":"demo","devDependencies":{"vitest":"1.0.0"}}"#,
        ),
        ("src/index.ts", "export const a = 1;\n"),
    ]);
    let root = ws.root();
    let stdout =
        b" Test Files  1 failed | 1 passed (2)\n      Tests  1 failed | 1 passed (2)\n".to_vec();
    let addr = exec_gateway(None, move |_req| (stdout.clone(), Vec::new(), Some(1))).await;

    let report = verify::run_verify(
        addr,
        &root,
        None,
        verify::VerifyKind::Test,
        Some("adds"),
        30,
    )
    .await
    .expect("the verify runs");
    assert_eq!(report.language, "typescript");
    assert_eq!((report.tests_passed, report.tests_failed), (1, 1));
}

/// A lint kind not matched by any language-specific parser still gets diagnostics, through the
/// `path:line:col:` fallback that also covers eslint and clang-style tools.
#[tokio::test]
async fn run_verify_falls_back_to_colon_diagnostics_for_typescript_lint() {
    let ws = Workspace::new(&[
        ("package.json", r#"{"name":"demo"}"#),
        ("eslint.config.js", "module.exports = [];\n"),
        ("src/index.ts", "export const a = 1\n"),
    ]);
    let root = ws.root();
    let stderr = b"src/index.ts:1:20: error: Missing semicolon\n".to_vec();
    let addr = exec_gateway(None, move |_req| (Vec::new(), stderr.clone(), Some(1))).await;

    let report = verify::run_verify(addr, &root, None, verify::VerifyKind::Lint, None, 30)
        .await
        .expect("the verify runs");
    assert_eq!(report.language, "typescript");
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].file.as_deref(), Some("src/index.ts"));
}

/// `ctest --output-on-failure` output is parsed into pass/fail counts and the failing test's
/// captured output.
#[tokio::test]
async fn run_verify_runs_cpp_ctest() {
    let ws = Workspace::new(&[
        (
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)\nproject(demo)\n",
        ),
        ("src/main.cpp", "int main() { return 0; }\n"),
    ]);
    let root = ws.root();
    let stdout = b"Test project /build/demo\n    Start 1: adds\n1/2 Test #1: adds .............................   Passed    0.01 sec\n    Start 2: fails\n2/2 Test #2: fails ............................***Failed    0.02 sec\nexpected 42, got 43\n\n50% tests passed, 1 tests failed out of 2\n".to_vec();
    let addr = exec_gateway(Some("/build/demo"), move |_req| {
        (stdout.clone(), Vec::new(), Some(1))
    })
    .await;

    let report = verify::run_verify(addr, &root, None, verify::VerifyKind::Test, None, 30)
        .await
        .expect("the verify runs");
    assert_eq!(report.language, "cpp");
    assert_eq!((report.tests_passed, report.tests_failed), (1, 1));
    assert_eq!(report.failures[0].name, "fails");
}

/// A diagnostic printed with the gateway's own absolute workspace path is rewritten relative to
/// the checkout, the same way it is for every other language.
#[tokio::test]
async fn run_verify_relativizes_diagnostic_paths_from_the_server_workspace_root() {
    let ws = Workspace::new(&[
        (
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)\nproject(demo)\n",
        ),
        ("src/main.cpp", "int main() { return oops; }\n"),
    ]);
    let root = ws.root();
    let stderr =
        b"/build/demo/src/main.cpp:1:22: error: use of undeclared identifier 'oops'\n".to_vec();
    let addr = exec_gateway(Some("/build/demo"), move |_req| {
        (Vec::new(), stderr.clone(), Some(1))
    })
    .await;

    let report = verify::run_verify(addr, &root, None, verify::VerifyKind::Check, None, 30)
        .await
        .expect("the verify runs");
    assert_eq!(report.language, "cpp");
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].file.as_deref(), Some("src/main.cpp"));
}

// ===== cluster: the node-selection probes speak ClusterRequest/PlaceRequest/... =====

/// One gateway answering all four probes with a canned reply each.
async fn cluster_probe_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                match framed.next().await {
                    Some(Ok(WireMessage::ClusterRequest)) => {
                        let _ = framed
                            .send(WireMessage::ClusterResponse(ClusterResponse {
                                this_node: addr.to_string(),
                                nodes: vec![PeerInfo {
                                    addr: addr.to_string(),
                                    status: sample_status(),
                                    workspaces: Vec::new(),
                                    last_seen_secs: 0,
                                    alive: true,
                                }],
                            }))
                            .await;
                    }
                    Some(Ok(WireMessage::PlaceRequest(req))) => {
                        let _ = framed
                            .send(WireMessage::PlaceResponse(PlaceResponse {
                                node: Some(addr.to_string()),
                                reason: format!("chosen for {}", req.workspace_name),
                            }))
                            .await;
                    }
                    Some(Ok(WireMessage::MetricsRequest(req))) => {
                        let _ = framed
                            .send(WireMessage::MetricsResponse(MetricsResponse {
                                node: addr.to_string(),
                                since_secs: req.since_secs,
                                events_in_memory: 3,
                                queries: Vec::new(),
                                execs: Vec::new(),
                                sync_rounds: 1,
                                sync_files: 2,
                                sync_bytes: 100,
                            }))
                            .await;
                    }
                    Some(Ok(WireMessage::StatusRequest)) => {
                        let _ = framed
                            .send(WireMessage::StatusResponse(sample_status()))
                            .await;
                    }
                    _ => {}
                }
            });
        }
    });
    addr
}

/// Each of the four one-shot probes gets back exactly the shape it asked for.
#[tokio::test]
async fn cluster_probes_answer_view_placement_metrics_and_status() {
    let addr = cluster_probe_server().await;

    let view = cluster::cluster_view(addr).await.unwrap();
    assert_eq!(view.this_node, addr.to_string());
    assert_eq!(view.nodes.len(), 1);
    assert!(view.nodes[0].alive);

    let place = cluster::ask_placement(addr, "ws-x", Some("rust"))
        .await
        .unwrap();
    assert_eq!(place.node.as_deref(), Some(addr.to_string().as_str()));
    assert!(place.reason.contains("ws-x"));

    let metrics = cluster::node_metrics(addr, 60).await.unwrap();
    assert_eq!(metrics.since_secs, 60);
    assert_eq!(metrics.sync_files, 2);

    let status = cluster::node_status(addr).await.unwrap();
    assert_eq!(status.server_pid, 4242);
    assert!(cluster::supports_engine(&status, "rust"));
}

/// A gateway that only ever offers "python" for placement.
async fn placement_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                match framed.next().await {
                    Some(Ok(WireMessage::PlaceRequest(req))) => {
                        let node = if req.engine.as_deref() == Some("rust") {
                            None
                        } else {
                            Some(addr.to_string())
                        };
                        let _ = framed
                            .send(WireMessage::PlaceResponse(PlaceResponse {
                                node,
                                reason: "gossip".to_string(),
                            }))
                            .await;
                    }
                    Some(Ok(WireMessage::StatusRequest)) => {
                        let _ = framed
                            .send(WireMessage::StatusResponse(StatusResponse {
                                server_pid: 7,
                                uptime_seconds: 1,
                                active_sessions: 0,
                                loaded_workspaces: 0,
                                detected_engines: vec!["python".to_string()],
                                memory_rss_bytes: None,
                                total_queries: 0,
                                active_queries: 0,
                                load_average_millis: Some(500),
                                cpu_count: Some(4),
                            }))
                            .await;
                    }
                    _ => {}
                }
            });
        }
    });
    addr
}

/// With no engine required, the one reachable node is picked and remembered; asking again for
/// an engine it does not serve reaches the same node through the remembered-placement check,
/// finds it unfit, asks the cluster again, and reports that no reachable node serves it.
#[tokio::test]
async fn pick_node_asks_the_cluster_then_reports_when_nothing_serves_the_engine() {
    let home = placement_server().await;
    let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let nodes = vec![dead, home];
    let temp = tempfile::tempdir().unwrap();
    let placement = temp.path().join("placement.json");

    let picked = cluster::pick_node_with(&nodes, "ws-a", None, Some(&placement))
        .await
        .expect("home is reachable");
    assert_eq!(picked, home);

    let err = cluster::pick_node_with(&nodes, "ws-a", Some("rust"), Some(&placement))
        .await
        .expect_err("home only serves python");
    let text = format!("{err:#}");
    assert!(text.contains("no reachable gateway serves rust"), "{text}");
    assert!(text.contains(&home.to_string()), "{text}");
}
