/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use prod_code_gateway::{ServerState, handle_client};
use prod_code_mcp::shadow::{self, HypothesisSpec, ShadowOutcome};
use prod_code_protocol::{ProdCodeCodec, ShadowRunRequest, WireMessage};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

pub struct Gateway {
    state: Arc<ServerState>,
    server: TcpListener,
    proxy: TcpListener,
    _storage: tempfile::TempDir,
}

impl Gateway {
    pub async fn new() -> Self {
        let storage = tempfile::tempdir().unwrap();
        let mut state = ServerState::new(storage.path().join("workspaces"));
        state.shadow_root = storage.path().join("shadows");
        Self {
            state: Arc::new(state),
            server: TcpListener::bind("127.0.0.1:0").await.unwrap(),
            proxy: TcpListener::bind("127.0.0.1:0").await.unwrap(),
            _storage: storage,
        }
    }

    pub async fn run(
        &self,
        root: &Path,
        specs: &[HypothesisSpec],
        command: &[&str],
    ) -> (ShadowOutcome, Vec<ShadowRunRequest>) {
        let server_addr = self.server.local_addr().unwrap();
        let proxy_addr = self.proxy.local_addr().unwrap();
        // No detached harness tasks: all three futures are owned and joined even on error.
        // The proxy's deadline closes both sockets; handle_client then cancels and joins
        // production shadow jobs. Each command also has the real gateway's 5s timeout.
        let serving = async {
            let (stream, addr) =
                tokio::time::timeout(Duration::from_secs(60), self.server.accept())
                    .await
                    .context("gateway accept timed out")??;
            handle_client(stream, addr, Arc::clone(&self.state)).await
        };
        let recording = async {
            tokio::time::timeout(Duration::from_secs(60), async {
                let (stream, _) = self.proxy.accept().await?;
                let mut client = Framed::new(stream, ProdCodeCodec::new());
                let stream = TcpStream::connect(server_addr).await?;
                let mut upstream = Framed::new(stream, ProdCodeCodec::new());
                let mut requests = Vec::new();
                loop {
                    tokio::select! {
                        message = client.next() => {
                            let Some(message) = message else { break };
                            let message = message?;
                            if let WireMessage::ShadowRunRequest(req) = &message {
                                requests.push(req.clone());
                            }
                            upstream.send(message).await?;
                        }
                        message = upstream.next() => {
                            let Some(message) = message else { break };
                            client.send(message?).await?;
                        }
                    }
                }
                Ok::<_, anyhow::Error>(requests)
            })
            .await
            .context("recording proxy timed out")?
        };
        let running = shadow::run_shadow(
            proxy_addr,
            root,
            None,
            specs,
            command.iter().map(|s| s.to_string()).collect(),
            Vec::new(),
            5,
            1,
            4096,
            false,
        );
        let (outcome, recorded, served) = tokio::join!(running, recording, serving);
        served.expect("production handle_client completes");
        let requests = recorded.expect("transparent proxy completes");
        let outcome = outcome.expect("production shadow round-trip completes");
        println!(
            "native backend={} overlay_unavailable={:?}",
            outcome.mode,
            prod_code_gateway::shadow::overlay_unavailable()
        );
        for result in &outcome.results {
            println!(
                "name={} exit={:?} timed_out={} error={:?} output={:?}",
                result.name, result.exit_code, result.timed_out, result.error, result.output
            );
        }
        (outcome, requests)
    }
}

pub struct Workspace {
    dir: tempfile::TempDir,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    pub fn root(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    pub fn write(&self, path: &str, bytes: &str, mode: u32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.root().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    pub async fn commit(&self) {
        for args in [
            vec!["init", "-q"],
            vec!["add", "-A"],
            vec![
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            let mut child = tokio::process::Command::new("git")
                .args(["-c", "core.hooksPath=/dev/null"])
                .args(args)
                .current_dir(self.root())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
                Ok(status) => assert!(status.unwrap().success(), "fixture git command failed"),
                Err(_) => {
                    child.kill().await.unwrap();
                    child.wait().await.unwrap();
                    panic!("fixture git command timed out");
                }
            }
        }
    }
}

pub fn snapshot(root: &Path) -> Vec<(PathBuf, u32, Vec<u8>)> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;
    fn visit(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, u32, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            // Sync owns this changing timestamp marker; it is not a fixture file.
            if matches!(
                path.file_name().unwrap().to_str(),
                Some(".git" | ".prod-code-last-used")
            ) {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let bytes = if meta.is_symlink() {
                std::fs::read_link(&path)
                    .unwrap()
                    .as_os_str()
                    .as_bytes()
                    .to_vec()
            } else if meta.is_dir() {
                visit(root, &path, out);
                Vec::new()
            } else {
                std::fs::read(&path).unwrap()
            };
            out.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                meta.permissions().mode(),
                bytes,
            ));
        }
    }
    let mut out = Vec::new();
    visit(root, root, &mut out);
    out.sort();
    out
}
