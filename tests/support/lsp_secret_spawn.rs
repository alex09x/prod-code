/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::{tls::TLS_ENV_VARS, transport::AUTH_TOKEN_VARS};
use std::process::Stdio;
use std::time::Duration;

// These sentinels are noncredential fixture data, never used for authentication.
const SENTINEL: &str = "lsp-spawn-noncredential-fixture";
const MODE: &str = "LSP_SECRET_SPAWN_MODE";
const INHERITED_CONTROL: &str = "LSP_SPAWN_INHERITED_CONTROL";
const CONFIGURED_CONTROL: &str = "LSP_SPAWN_CONFIGURED_CONTROL";

fn reserved_names() -> Vec<&'static str> {
    AUTH_TOKEN_VARS.into_iter().chain(TLS_ENV_VARS).collect()
}

pub async fn run_driver(test: &str, mode: &str) {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            test.split_once("::").unwrap().1,
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env(MODE, mode)
        .env(INHERITED_CONTROL, SENTINEL)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    if mode == "inherited" {
        for name in reserved_names() {
            command.env(name, SENTINEL);
        }
    }
    let mut driver = command.spawn().unwrap();
    let status = match tokio::time::timeout(Duration::from_secs(30), driver.wait()).await {
        Ok(result) => result.unwrap(),
        Err(_) => {
            driver.kill().await.unwrap();
            panic!("isolated spawn driver timed out");
        }
    };
    assert!(status.success(), "{mode} spawn driver failed: {status}");
}

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Fixture {
    pub fn new() -> Self {
        let mode = std::env::var(MODE).expect("run only through the isolated fixture driver");
        assert!(matches!(mode.as_str(), "inherited" | "configured"));
        for name in reserved_names() {
            assert_eq!(std::env::var_os(name).is_some(), mode == "inherited");
        }
        let root = tempfile::tempdir().unwrap();
        let mut env = vec![(CONFIGURED_CONTROL.to_string(), SENTINEL.to_string())];
        if mode == "configured" {
            env.extend(
                reserved_names()
                    .into_iter()
                    .map(|name| (name.to_string(), SENTINEL.to_string())),
            );
        }
        let args = vec![
            "-u".to_string(),
            "-c".to_string(),
            SERVER.to_string(),
            root.path()
                .join("presence.json")
                .to_string_lossy()
                .into_owned(),
            serde_json::to_string(&reserved_names()).unwrap(),
        ];
        Self { root, args, env }
    }

    pub fn assert_scrubbed(&self) {
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(self.root.path().join("presence.json")).unwrap())
                .unwrap();
        assert_eq!(report["inherited_control"], true);
        assert_eq!(report["configured_control"], true);
        assert_eq!(
            report["present"],
            serde_json::json!([]),
            "reserved environment names reached the language server"
        );
    }
}

const SERVER: &str = r#"import json, os, sys
report = {
    'present': [name for name in json.loads(sys.argv[2]) if name in os.environ],
    'inherited_control': os.environ.get('LSP_SPAWN_INHERITED_CONTROL') == 'lsp-spawn-noncredential-fixture',
    'configured_control': os.environ.get('LSP_SPAWN_CONFIGURED_CONTROL') == 'lsp-spawn-noncredential-fixture',
}
with open(sys.argv[1], 'w') as output:
    json.dump(report, output)
while True:
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if not line.strip(): break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':', 1)[1])
    request = json.loads(sys.stdin.buffer.read(length))
    if request.get('method') == 'exit': sys.exit(0)
    if request.get('method') == 'initialize':
        body = json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': {'capabilities': {}}}).encode()
        sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
        sys.stdout.buffer.flush()
"#;
