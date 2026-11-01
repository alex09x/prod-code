/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    PathTranslator, ProdCodeCodec, WireMessage, readiness::ReadySignal, transport::read_lsp_frame,
};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::BufReader;
use tokio::time::Instant;
use tokio_util::codec::Framed;

use crate::workspace::WatchedChange;

use super::command::{
    ServerCommand, frame, server_command, server_initialization_options, to_server,
    to_server_with_options,
};
use super::probe::{EditorProxyOptions, WRITE_BUDGET};
use super::proxy::run_with_options;
use super::registry::{EditorServers, PendingServerFrame};

#[cfg(unix)]
#[tokio::test]
async fn gateway_watchdog_pings_do_not_suppress_post_initialize_health_probes() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (client_socket, (server_socket, _)) =
        tokio::try_join!(tokio::net::TcpStream::connect(addr), listener.accept()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().to_path_buf();
    let root_text = root_path.to_string_lossy().into_owned();
    let probe_marker = root_path.join("health-probe-seen");
    let probe_marker_text = probe_marker.to_string_lossy().into_owned();
    let initialization_marker = root_path.join("initialization-options-seen");
    let initialization_marker_text = initialization_marker.to_string_lossy().into_owned();
    let stub_path = root_path.join("typings").to_string_lossy().into_owned();
    let translator = PathTranslator::new(&root_text, &root_text);
    let script = concat!(
        "import json, sys\n",
        "import os\n",
        "while True:\n",
        "    length = None\n",
        "    while True:\n",
        "        line = sys.stdin.buffer.readline()\n",
        "        if not line: sys.exit(0)\n",
        "        if line in (b'\\r\\n', b'\\n'): break\n",
        "        if line.lower().startswith(b'content-length:'): length = int(line.split(b':', 1)[1])\n",
        "    message = json.loads(sys.stdin.buffer.read(length))\n",
        "    if message.get('method') == 'prodCode/healthProbe': open(os.environ['PROBE_MARKER'], 'w').write('seen')\n",
        "    if message.get('method') == 'initialize': open(os.environ['INIT_MARKER'], 'w').write(json.dumps(message.get('params', {}).get('initializationOptions')))\n",
        "    result = {'capabilities': {}} if message.get('method') == 'initialize' else {}\n",
        "    body = json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'result': result}).encode()\n",
        "    sys.stdout.buffer.write(b'Content-Length: %d\\r\\n\\r\\n' % len(body) + body)\n",
        "    sys.stdout.buffer.flush()\n",
    );
    let command = ServerCommand {
        program: "python3".to_string(),
        args: vec!["-u".to_string(), "-c".to_string(), script.to_string()],
        env: vec![
            ("PROBE_MARKER".to_string(), probe_marker_text),
            ("INIT_MARKER".to_string(), initialization_marker_text),
        ],
        initialization_options: Some(serde_json::json!({
            "python": { "analysis": { "stubPath": stub_path.clone() } }
        })),
        ready: ReadySignal::Unknown,
    };
    let servers = EditorServers::default();
    let server_task = tokio::spawn(async move {
        let _root = root;
        run_with_options(
            Framed::new(server_socket, ProdCodeCodec::new()),
            translator,
            command,
            &root_path,
            &servers,
            1,
            EditorProxyOptions {
                health_probe_interval: Some(Duration::from_millis(60)),
                health_response_timeout: Duration::from_secs(1),
                ..EditorProxyOptions::default()
            },
        )
        .await
    });
    let mut client = Framed::new(client_socket, ProdCodeCodec::new());

    for _ in 0..4 {
        client.send(WireMessage::Ping).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(100), client.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            WireMessage::Pong
        ));
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    client
        .send(WireMessage::LspPayload(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#
                .to_string(),
        ))
        .await
        .unwrap();
    let init = tokio::time::timeout(Duration::from_secs(1), client.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(init, WireMessage::LspPayload(payload) if serde_json::from_str::<serde_json::Value>(&payload).unwrap()["id"] == 1)
    );
    let received_options: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&initialization_marker).expect("child recorded initialization options"),
    )
    .unwrap();
    assert_eq!(
        received_options["python"]["analysis"]["stubPath"].as_str(),
        Some(stub_path.as_str())
    );

    // Allow several probe intervals after initialization; the first interval may have
    // elapsed before the initialization response made probes eligible.
    for _ in 0..24 {
        client.send(WireMessage::Ping).await.unwrap();
        let response = tokio::time::timeout(Duration::from_millis(40), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(response, WireMessage::Pong));
        if probe_marker.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert!(
        probe_marker.exists(),
        "health probe should reach the LSP server despite gateway pings"
    );
    client
        .send(WireMessage::Disconnect {
            reason: "test complete".to_string(),
        })
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
}

#[tokio::test]
async fn frames_are_read_whatever_the_case_of_their_headers() {
    let input =
        b"Content-Length: 2\r\n\r\n{}content-length: 13\r\nContent-Type: x\r\n\r\n{\"id\":1}     "
            as &[u8];
    let mut reader = BufReader::new(input);
    assert_eq!(
        read_lsp_frame(&mut reader).await.unwrap().as_deref(),
        Some("{}")
    );
    assert_eq!(
        read_lsp_frame(&mut reader).await.unwrap().as_deref(),
        Some("{\"id\":1}     ")
    );
    assert_eq!(read_lsp_frame(&mut reader).await.unwrap(), None);
    assert_eq!(frame("{}"), b"Content-Length: 2\r\n\r\n{}".to_vec());
}

#[test]
fn initialize_reaches_the_server_on_its_paths_without_the_editors_process() {
    let translator = PathTranslator::new("/Users/dev/app", "/srv/workspaces/app");
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":4242,"rootUri":"file:///Users/dev/app","rootPath":"/Users/dev/app"}}"#;
    let sent: serde_json::Value = serde_json::from_str(&to_server(&translator, init)).unwrap();
    assert_eq!(sent["params"]["processId"], serde_json::Value::Null);
    assert_eq!(sent["params"]["rootUri"], "file:///srv/workspaces/app");
    assert_eq!(sent["params"]["rootPath"], "/srv/workspaces/app");
    // Other requests keep their JSON values while locations are translated.
    let hover = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///Users/dev/app/src/lib.rs"}}}"#;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&to_server(&translator, hover)).unwrap(),
        serde_json::from_str::<serde_json::Value>(
            &hover.replace("/Users/dev/app", "/srv/workspaces/app")
        )
        .unwrap()
    );
}

#[test]
fn server_initialization_options_merge_with_editor_options() {
    let translator = PathTranslator::new("/client", "/server");
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"initializationOptions":{"python":{"analysis":{"typeCheckingMode":"strict"}}}}}"#;
    let server_options = serde_json::json!({
        "python": { "analysis": { "stubPath": "/server/typings" } }
    });

    let sent: serde_json::Value = serde_json::from_str(&to_server_with_options(
        &translator,
        initialize,
        Some(&server_options),
    ))
    .unwrap();

    assert_eq!(
        sent["params"]["initializationOptions"]["python"]["analysis"]["stubPath"],
        "/server/typings"
    );
    assert_eq!(
        sent["params"]["initializationOptions"]["python"]["analysis"]["typeCheckingMode"],
        "strict"
    );
}

#[test]
fn editor_proxy_issue_1289_generic_defaults_keep_sdk_paths_without_editor_preferences() {
    let options = Some(serde_json::json!({
        "typescript": { "includeInlayParameterNameHints": "all" },
        "tsserver": { "path": "/server/typescript/lib", "maxTsServerMemory": 4096 },
        "java": { "autobuild": { "enabled": false } }
    }));

    assert_eq!(
        server_initialization_options("typescript", options.clone()),
        Some(serde_json::json!({"tsserver":{"path":"/server/typescript/lib"}}))
    );
    assert!(server_initialization_options("java", options.clone()).is_none());
    assert_eq!(
        server_initialization_options("python", options.clone()),
        options
    );
}

#[test]
fn a_server_the_node_lacks_is_not_offered() {
    assert!(server_command("cobol").is_none());
}

#[tokio::test]
async fn a_sync_reaches_the_servers_whose_root_holds_the_files() {
    let servers = EditorServers::default();
    let (app_tx, mut app_rx) = rapidfire::mpsc::bounded(8);
    let (other_tx, mut other_rx) = rapidfire::mpsc::bounded(8);
    let app = servers.register(PathBuf::from("/srv/workspaces/app"), app_tx, WRITE_BUDGET);
    let _other = servers.register(
        PathBuf::from("/srv/workspaces/other"),
        other_tx,
        WRITE_BUDGET,
    );
    assert_eq!(servers.count(), 2);
    servers
        .notify(&[(
            PathBuf::from("/srv/workspaces/app/src/lib.rs"),
            WatchedChange::Changed,
        )])
        .await;
    let note: serde_json::Value = serde_json::from_str(&app_rx.recv().await.unwrap().body).unwrap();
    assert_eq!(note["method"], "workspace/didChangeWatchedFiles");
    assert_eq!(
        note["params"]["changes"][0],
        serde_json::json!({ "uri": "file:///srv/workspaces/app/src/lib.rs", "type": 2 })
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), other_rx.recv())
            .await
            .is_err(),
        "the other workspace's server hears nothing"
    );
    drop(app);
    assert_eq!(servers.count(), 1);
}

#[tokio::test]
async fn a_full_registration_retires_without_delaying_a_healthy_target() {
    let servers = EditorServers::default();
    let root = PathBuf::from("/srv/workspaces/app");
    let (lagging_tx, _lagging_rx) = rapidfire::mpsc::bounded(1);
    let mut lagging = servers.register(root.clone(), lagging_tx.clone(), WRITE_BUDGET);
    lagging_tx
        .try_send(PendingServerFrame {
            body: "held".to_string(),
            deadline: Instant::now() + WRITE_BUDGET,
        })
        .unwrap();
    let (healthy_tx, mut healthy_rx) = rapidfire::mpsc::bounded(1);
    let _healthy = servers.register(root.clone(), healthy_tx, WRITE_BUDGET);

    servers
        .notify(&[(root.join("src/lib.rs"), WatchedChange::Changed)])
        .await;

    let healthy = tokio::time::timeout(Duration::from_millis(50), healthy_rx.recv())
        .await
        .expect("healthy registration is notified promptly")
        .expect("healthy registration remains open");
    let note: serde_json::Value = serde_json::from_str(&healthy.body).unwrap();
    assert_eq!(note["method"], "workspace/didChangeWatchedFiles");
    tokio::time::timeout(Duration::from_millis(50), lagging.retired())
        .await
        .expect("full registration is retired promptly");
    assert_eq!(servers.count(), 1);
}

#[test]
fn issue_1771_diagnostic_normalization_preservation_and_code_action_round_trip() {
    let translator = PathTranslator::new("/client", "/server");
    let (href559, href609) = (
        "https://doc.rust-lang.org/error_codes/E0559.html",
        "https://doc.rust-lang.org/error_codes/E0609.html",
    );

    // 1. publishDiagnostics & 3. enum-variant E0559 preservation
    let server_pub = serde_json::json!({
        "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": "file:///client/lib.rs",
            "diagnostics": [
                { "code": "E0559", "message": "no field `a` on type `A`", "codeDescription": { "href": href559 } },
                { "code": "E0559", "message": "no field `b` on type `B`", "data": { "k": 1 } },
                { "code": "E0559", "message": "no field `c` on type `C`", "data": null },
                { "code": "E0559", "message": "no field `d` on type `D`", "data": [2] },
                { "code": "E0559", "message": "variant `V::X` has no field named `f`" }
            ]
        }
    }).to_string();
    let norm_pub: serde_json::Value =
        serde_json::from_str(&super::normalize_server_diagnostics(&server_pub)).unwrap();
    let d = norm_pub["params"]["diagnostics"].as_array().unwrap();
    for (i, code) in ["E0609", "E0609", "E0609", "E0609", "E0559"]
        .iter()
        .enumerate()
    {
        assert_eq!(d[i]["code"], *code);
    }
    assert_eq!(d[0]["codeDescription"]["href"], href609);
    assert_eq!(d[1]["data"]["k"], 1);

    // 2. pull diagnostics: document and workspace
    let pull_doc = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": {
            "items": [{ "code": "E0559", "message": "no field `x` on type `X`" }],
            "relatedDocuments": { "file:///r.rs": { "items": [{ "code": "E0559", "message": "no field `y` on type `Y`" }] } }
        }
    }).to_string();
    let norm_doc: serde_json::Value =
        serde_json::from_str(&super::normalize_server_diagnostics(&pull_doc)).unwrap();
    assert_eq!(norm_doc["result"]["items"][0]["code"], "E0609");
    let rel_code = &norm_doc["result"]["relatedDocuments"]["file:///r.rs"]["items"][0]["code"];
    assert_eq!(rel_code, "E0609");

    let pull_ws = serde_json::json!({
        "jsonrpc": "2.0", "id": 2,
        "result": { "items": [{ "items": [{ "code": "E0559", "message": "no field `z` on type `Z`" }] }] }
    }).to_string();
    let norm_ws: serde_json::Value =
        serde_json::from_str(&super::normalize_server_diagnostics(&pull_ws)).unwrap();
    assert_eq!(norm_ws["result"]["items"][0]["items"][0]["code"], "E0609");

    // 4. codeAction round-trip restoration (with marker and fallback)
    let mut req_diags = d.clone();
    req_diags.push(serde_json::json!({
        "code": "E0609", "message": "no field `fb` on type `F`",
        "codeDescription": { "href": href609 }
    }));
    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "textDocument/codeAction",
        "params": { "context": { "diagnostics": req_diags } }
    })
    .to_string();
    let fwd: serde_json::Value = serde_json::from_str(&to_server(&translator, &req)).unwrap();
    let r = fwd["params"]["context"]["diagnostics"].as_array().unwrap();
    for diag in r {
        assert_eq!(diag["code"], "E0559");
    }
    assert_eq!(r[0]["codeDescription"]["href"], href559);
    assert!(r[0].get("data").is_none());
    assert_eq!(r[1]["data"], serde_json::json!({ "k": 1 }));
    assert_eq!(r[2]["data"], serde_json::Value::Null);
    assert_eq!(r[3]["data"], serde_json::json!([2]));
    assert_eq!(r[4]["message"], "variant `V::X` has no field named `f`");
    assert_eq!(r[5]["codeDescription"]["href"], href559);
}
