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

use super::command::{ServerCommand, frame, server_command, to_server};
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
        "    result = {'capabilities': {}} if message.get('method') == 'initialize' else {}\n",
        "    body = json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'result': result}).encode()\n",
        "    sys.stdout.buffer.write(b'Content-Length: %d\\r\\n\\r\\n' % len(body) + body)\n",
        "    sys.stdout.buffer.flush()\n",
    );
    let command = ServerCommand {
        program: "python3".to_string(),
        args: vec!["-u".to_string(), "-c".to_string(), script.to_string()],
        env: vec![("PROBE_MARKER".to_string(), probe_marker_text)],
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
