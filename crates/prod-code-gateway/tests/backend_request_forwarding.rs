use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage};
use std::{
    io::{BufRead, BufReader},
    net::SocketAddr,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_util::codec::Framed;

const WATCHDOG: Duration = Duration::from_secs(15);

struct Gateway {
    _process: OwnedProcess,
    addr: SocketAddr,
    _storage: tempfile::TempDir,
    _home: tempfile::TempDir,
}

struct OwnedProcess {
    child: Child,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        let group = -(self.child.id() as i32);
        // The gateway and every fake server share this private process group.
        // Reap descendants even when the gateway has already exited.
        unsafe {
            libc::kill(group, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        unsafe {
            libc::kill(group, libc::SIGKILL);
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Gateway {
    fn start(log: &Path) -> Self {
        let storage = tempfile::tempdir().expect("storage directory");
        let home = tempfile::tempdir().expect("fake gopls home");
        let bin = home.path().join(".cargo/bin");
        std::fs::create_dir_all(&bin).expect("fake bin directory");
        let fake = bin.join("gopls");
        std::fs::write(&fake, FAKE_GOPLS).expect("fake gopls");
        let mut permissions = std::fs::metadata(&fake)
            .expect("fake metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake, permissions).expect("fake is executable");

        let mut command = Command::new(env!("CARGO_BIN_EXE_prod-code-server"));
        command
            .process_group(0)
            .env("HOME", home.path())
            .env("FAKE_GOPLS_LOG", log)
            .env("PROD_CODE_BIND", "127.0.0.1:0")
            .env("PROD_CODE_STORAGE", storage.path())
            .env("PROD_CODE_PEERS", "")
            .env_remove("RUST_LOG")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut process = OwnedProcess {
            child: command.spawn().expect("gateway starts"),
            reader: None,
        };
        let addr = read_bound_address(&mut process);
        let gateway = Self {
            _process: process,
            addr,
            _storage: storage,
            _home: home,
        };
        gateway.wait_until_listening();
        gateway
    }

    fn wait_until_listening(&self) {
        let deadline = Instant::now() + WATCHDOG;
        while Instant::now() < deadline {
            if std::net::TcpStream::connect_timeout(&self.addr, Duration::from_millis(100)).is_ok()
            {
                return;
            }
        }
        panic!("gateway never listened on {}", self.addr);
    }
}

fn read_bound_address(process: &mut OwnedProcess) -> SocketAddr {
    let stdout = process.child.stdout.take().expect("gateway stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    process.reader = Some(std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if let Some(rest) = line.split("listening on ").nth(1)
                && let Ok(addr) = rest.trim().parse()
            {
                let _ = tx.send(addr);
                return;
            }
        }
    }));
    rx.recv_timeout(WATCHDOG)
        .expect("gateway reports its bound address")
}

async fn wait_for_log(log: &Path, predicate: impl Fn(&[serde_json::Value]) -> bool) {
    timeout(WATCHDOG, async {
        loop {
            let entries = std::fs::read_to_string(log)
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).expect("fake gopls log JSON"))
                .collect::<Vec<_>>();
            if predicate(&entries) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fake gopls received every expected reply");
}

fn has_reply(
    entries: &[serde_json::Value],
    id: serde_json::Value,
    result: serde_json::Value,
) -> bool {
    entries
        .iter()
        .filter(|entry| entry.get("id") == Some(&id) && entry.get("result") == Some(&result))
        .count()
        == 1
}

#[tokio::test(flavor = "current_thread")]
async fn fallback_auto_replies_do_not_cross_the_session_boundary() {
    let log_dir = tempfile::tempdir().expect("fake log directory");
    let log = log_dir.path().join("gopls.jsonl");
    let gateway = Gateway::start(&log);
    let client_root = tempfile::tempdir().expect("client workspace");
    std::fs::write(
        client_root.path().join("go.mod"),
        "module example.com/fallback\n\ngo 1.22\n",
    )
    .expect("go module");

    timeout(WATCHDOG, exchange(&gateway, &client_root, &log))
        .await
        .expect("fallback exchange finishes within its watchdog");
}

async fn exchange(gateway: &Gateway, client_root: &tempfile::TempDir, log: &Path) {
    let stream = TcpStream::connect(gateway.addr)
        .await
        .expect("gateway connection");
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: PROTOCOL_VERSION,
            supported_versions: None,
            client_name: "fallback-forwarding-test".to_string(),
            client_pid: std::process::id(),
            auth_token: None,
            client_workspace_root: client_root.path().display().to_string(),
            preferred_engine: Some("go".to_string()),
            base_workspace_name: None,
            engine_subpath: None,
            client_agent: None,
            client_host: None,
            purpose: None,
        }))
        .await
        .expect("handshake is sent");
    let Some(Ok(WireMessage::HandshakeResponse(response))) = framed.next().await else {
        panic!("fallback session was not initialized");
    };
    assert_eq!(response.detected_engine, "go");

    framed
        .send(WireMessage::LspPayload(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 700,
                "method": "test/trigger",
                "params": {}
            })
            .to_string(),
        ))
        .await
        .expect("trigger reaches fallback");

    let mut payloads = Vec::new();
    let mut saw_response = false;
    let mut saw_unknown = false;
    let mut saw_notification = false;
    let mut saw_missing_id = false;
    let mut saw_null_id = false;
    let mut saw_malformed = false;
    let delivered = timeout(WATCHDOG, async {
        while !(saw_response
            && saw_unknown
            && saw_notification
            && saw_missing_id
            && saw_null_id
            && saw_malformed)
        {
            let Some(Ok(WireMessage::LspPayload(payload))) = framed.next().await else {
                panic!("gateway closed while forwarding fallback frames");
            };
            saw_malformed |= payload == MALFORMED_NOTIFICATION;
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&payload) {
                saw_response |= value.get("id") == Some(&serde_json::json!(700))
                    && value
                        .pointer("/result/from")
                        .and_then(serde_json::Value::as_str)
                        == Some("fake");
                saw_unknown |= value.get("id") == Some(&serde_json::json!("unknown"))
                    && value.get("method").and_then(serde_json::Value::as_str)
                        == Some("workspace/unknown");
                saw_notification |= value.get("method").and_then(serde_json::Value::as_str)
                    == Some("window/logMessage");
                saw_missing_id |= value.get("method").and_then(serde_json::Value::as_str)
                    == Some("window/workDoneProgress/create")
                    && value.get("id").is_none();
                saw_null_id |= value.get("method").and_then(serde_json::Value::as_str)
                    == Some("client/registerCapability")
                    && value.get("id") == Some(&serde_json::Value::Null);
                payloads.push(value);
            }
        }
    })
    .await;
    assert!(
        delivered.is_ok(),
        "only deliverable fallback frames arrive before watchdog: {payloads:?}"
    );

    assert!(
        !payloads.iter().any(|value| {
            value.get("method").and_then(serde_json::Value::as_str)
                == Some("window/workDoneProgress/create")
                && value.get("id") == Some(&serde_json::json!(700))
        }),
        "the auto-answered request sharing the client request ID was forwarded"
    );
    assert!(
        !payloads.iter().any(|value| {
            value.get("method").and_then(serde_json::Value::as_str)
                == Some("client/registerCapability")
                && value.get("id") == Some(&serde_json::json!("capability"))
        }),
        "client/registerCapability was forwarded after the fallback answered it"
    );
    assert!(
        !payloads.iter().any(|value| {
            value.get("method").and_then(serde_json::Value::as_str)
                == Some("workspace/configuration")
                && value.get("id") == Some(&serde_json::json!(19))
        }),
        "workspace/configuration was forwarded after the fallback answered it"
    );

    framed
        .send(WireMessage::LspPayload(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "unknown",
                "result": {"accepted": true}
            })
            .to_string(),
        ))
        .await
        .expect("unknown request response reaches fallback");

    wait_for_log(log, |entries| {
        has_reply(entries, serde_json::json!(700), serde_json::Value::Null)
            && has_reply(
                entries,
                serde_json::json!("capability"),
                serde_json::Value::Null,
            )
            && has_reply(entries, serde_json::json!(19), serde_json::json!([{}]))
            && has_reply(entries, serde_json::Value::Null, serde_json::Value::Null)
            && entries.iter().any(|entry| {
                entry.get("id") == Some(&serde_json::json!("unknown"))
                    && entry.pointer("/result/accepted") == Some(&serde_json::Value::Bool(true))
            })
    })
    .await;

    framed
        .send(WireMessage::Disconnect {
            reason: "test complete".to_string(),
        })
        .await
        .expect("session disconnect");
}

const MALFORMED_NOTIFICATION: &str = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":"#;

const FAKE_GOPLS: &str = r#"#!/usr/bin/env python3
import json
import os
import sys

log = open(os.environ["FAKE_GOPLS_LOG"], "a", buffering=1)

def read_frame():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1].strip())
    return json.loads(sys.stdin.buffer.read(length))

def write_raw(body):
    encoded = body.encode()
    sys.stdout.buffer.write(b"Content-Length: " + str(len(encoded)).encode() + b"\r\n\r\n" + encoded)
    sys.stdout.buffer.flush()

def write(value):
    write_raw(json.dumps(value, separators=(",", ":")))

while True:
    message = read_frame()
    if message is None:
        break
    log.write(json.dumps(message, separators=(",", ":")) + "\n")
    if message.get("method") == "initialize":
        if message.get("params", {}).get("processId") is not None:
            write_raw("not json")
            break
        write({"jsonrpc":"2.0", "id":message["id"], "result":{"capabilities":{}}})
    elif message.get("method") == "test/trigger":
        write({"jsonrpc":"2.0", "id":700, "method":"window/workDoneProgress/create", "params":{}})
        write({"jsonrpc":"2.0", "id":"capability", "method":"client/registerCapability", "params":{}})
        write({"jsonrpc":"2.0", "id":19, "method":"workspace/configuration", "params":{"items":[{}]}})
        write({"jsonrpc":"2.0", "method":"window/logMessage", "params":{"type":3, "message":"notice"}})
        write_raw('{"jsonrpc":"2.0","method":"window/logMessage","params":')
        write({"jsonrpc":"2.0", "method":"window/workDoneProgress/create", "params":{}})
        write({"jsonrpc":"2.0", "id":None, "method":"client/registerCapability", "params":{}})
        write({"jsonrpc":"2.0", "id":"unknown", "method":"workspace/unknown", "params":{}})
        write({"jsonrpc":"2.0", "id":700, "result":{"from":"fake"}})
"#;
