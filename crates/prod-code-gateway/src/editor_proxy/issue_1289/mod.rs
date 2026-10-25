/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod backpressure;
mod initialization;

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{PathTranslator, ProdCodeCodec, WireMessage, readiness::ReadySignal};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{Instant, timeout};
use tokio_util::codec::Framed;

use super::EditorServers;
use crate::editor_proxy::{EditorProxyOptions, ProbeState, ServerCommand, run_with_options};

const WAIT: Duration = Duration::from_secs(5);
const TEARDOWN: Duration = Duration::from_millis(250);

// The child publishes its PID before waiting on a file barrier. No stdin is read until
// the test releases it. Each received frame is echoed as a result for ordering/capture.
const CHILD: &str = r#"
import json, os, sys, time
with open('pid.tmp', 'w') as marker:
    marker.write(str(os.getpid()))
os.replace('pid.tmp', 'pid')
while not os.path.exists('release'):
    if os.path.exists('exit'): sys.exit(0)
    time.sleep(0.002)
while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line in (b'\r\n', b'\n'): break
        if line.lower().startswith(b'content-length:'): length = int(line.split(b':', 1)[1])
    message = json.loads(sys.stdin.buffer.read(length))
    if message.get('method') == 'initialize':
        open('initialize.json', 'w').write(json.dumps(message))
    # Avoid echoing the large pipe-blocking padding back to the editor.
    message.get('params', {}).pop('padding', None)
    body = json.dumps({'jsonrpc': '2.0', 'id': message.get('id'), 'result': message}).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()
"#;

struct Session {
    editor: Framed<TcpStream, ProdCodeCodec>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    root: tempfile::TempDir,
    servers: Arc<EditorServers>,
    state: Arc<Mutex<ProbeState>>,
    pid: i32,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Session {
    async fn start(options: Option<Value>, budget: Duration) -> Self {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, (server, _)) = tokio::try_join!(
            TcpStream::connect(listener.local_addr().unwrap()),
            listener.accept()
        )
        .unwrap();
        let servers = Arc::new(EditorServers::default());
        let state = Arc::new(Mutex::new(ProbeState::default()));
        let command = ServerCommand {
            program: "python3".into(),
            args: vec!["-u".into(), "-c".into(), CHILD.into()],
            env: Vec::new(),
            initialization_options: options,
            ready: ReadySignal::Unknown,
        };
        let proxy_root = root.path().to_path_buf();
        let proxy_servers = Arc::clone(&servers);
        let proxy_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            run_with_options(
                Framed::new(server, ProdCodeCodec::new()),
                PathTranslator::new("/client", &proxy_root.to_string_lossy()),
                command,
                &proxy_root,
                &proxy_servers,
                1289,
                EditorProxyOptions {
                    write_budget: budget,
                    teardown_budget: TEARDOWN,
                    health_probe_interval: None,
                    probe_state: Some(proxy_state),
                    ..Default::default()
                },
            )
            .await
        });
        let mut session = Self {
            editor: Framed::new(client, ProdCodeCodec::new()),
            task,
            root,
            servers,
            state,
            pid: 0,
        };
        session
            .wait_until(|| session.root.path().join("pid").exists())
            .await;
        session.pid = std::fs::read_to_string(session.root.path().join("pid"))
            .unwrap()
            .parse()
            .unwrap();
        session
    }

    async fn wait_until(&self, condition: impl Fn() -> bool) {
        timeout(WAIT, async {
            while !condition() {
                assert!(
                    !self.task.is_finished(),
                    "proxy ended before the controlled boundary"
                );
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("controlled boundary reached before timeout");
    }

    async fn send(&mut self, message: Value) {
        timeout(
            WAIT,
            self.editor
                .send(WireMessage::LspPayload(message.to_string())),
        )
        .await
        .unwrap()
        .unwrap();
    }

    async fn receive(&mut self) -> Value {
        let message = timeout(WAIT, self.editor.next())
            .await
            .unwrap()
            .expect("session survives temporary pressure")
            .unwrap();
        let WireMessage::LspPayload(raw) = message else {
            panic!("unexpected {message:?}")
        };
        serde_json::from_str(&raw).unwrap()
    }

    fn release(&self) {
        std::fs::write(self.root.path().join("release"), "go").unwrap();
    }

    async fn finish(&mut self) {
        timeout(WAIT, &mut self.task)
            .await
            .expect("bounded proxy teardown")
            .unwrap()
            .unwrap();
        assert_eq!(self.servers.count(), 0);
        assert_eq!(
            unsafe { libc::kill(self.pid, 0) },
            -1,
            "owned child was reaped"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    async fn disconnect(&mut self) {
        self.editor
            .send(WireMessage::Disconnect {
                reason: "test complete".into(),
            })
            .await
            .unwrap();
        self.finish().await;
    }
}
