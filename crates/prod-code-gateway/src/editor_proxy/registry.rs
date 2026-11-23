/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::workspace::WatchedChange;
use prod_code_protocol::WireMessage;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::Instant;

#[cfg(all(test, unix))]
#[path = "issue_1289/mod.rs"]
mod issue_1289;

/// The editors' language servers running on this node, with the roots they were started in,
/// so that a sync can tell each which of its files changed on disk.
#[derive(Default)]
pub struct EditorServers {
    next: AtomicU64,
    servers: std::sync::Mutex<Vec<ServerRegistration>>,
}

pub(crate) struct ServerRegistration {
    pub(crate) id: u64,
    pub(crate) root: PathBuf,
    pub(crate) input: rapidfire::mpsc::Sender<PendingServerFrame>,
    pub(crate) retire: tokio::sync::watch::Sender<bool>,
    pub(crate) write_budget: Duration,
}

pub(crate) struct PendingServerFrame {
    pub(crate) body: String,
    pub(crate) deadline: Instant,
}

pub(crate) struct PendingEditorMessage {
    pub(crate) message: WireMessage,
    pub(crate) deadline: Instant,
}

/// A server's place in [`EditorServers`], given up when the session ends.
pub struct Registration<'a> {
    servers: &'a EditorServers,
    id: u64,
    retired: tokio::sync::watch::Receiver<bool>,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.servers.remove(self.id, false);
    }
}

impl Registration<'_> {
    pub(crate) async fn retired(&mut self) {
        if !*self.retired.borrow() {
            let _ = self.retired.changed().await;
        }
    }
}

impl EditorServers {
    /// Adds a server started in `root` that takes LSP message bodies on `input`.
    pub(crate) fn register(
        &self,
        root: PathBuf,
        input: rapidfire::mpsc::Sender<PendingServerFrame>,
        write_budget: Duration,
    ) -> Registration<'_> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (retire, retired) = tokio::sync::watch::channel(false);
        self.servers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(ServerRegistration {
                id,
                root,
                input,
                retire,
                write_budget,
            });
        Registration {
            servers: self,
            id,
            retired,
        }
    }

    fn remove(&self, id: u64, retire: bool) {
        let mut servers = self.servers.lock().unwrap_or_else(|e| e.into_inner());
        servers.retain(|server| {
            if server.id != id {
                return true;
            }
            server.input.close();
            if retire {
                server.retire.send_replace(true);
            }
            false
        });
    }

    /// How many editor servers are running.
    pub fn count(&self) -> usize {
        self.servers.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Sends `workspace/didChangeWatchedFiles` for the `changes` under each server's root.
    pub async fn notify(&self, changes: &[(PathBuf, WatchedChange)]) {
        let targets: Vec<_> = self
            .servers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|server| {
                (
                    server.id,
                    server.root.clone(),
                    server.input.clone(),
                    server.write_budget,
                )
            })
            .collect();
        let mut retire = Vec::new();
        // A prod-code.toml that changed at or above a Rust server's root may now forbid build
        // scripts there; a server of its own runs them, so it is retired and the editor's next
        // session goes to the in-process engine.
        let policy_dirs: Vec<&std::path::Path> = changes
            .iter()
            .filter(|(path, _)| {
                path.file_name()
                    .is_some_and(|n| n == prod_code_engine_rust::ProdCodeConfig::FILE_NAME)
            })
            .filter_map(|(path, _)| path.parent())
            .collect();
        for (id, root, input, write_budget) in targets {
            if policy_dirs.iter().any(|dir| root.starts_with(dir))
                && root.join("Cargo.toml").is_file()
                && !prod_code_engine_rust::ProdCodeConfig::load(&root)
                    .rust
                    .build_scripts
            {
                retire.push(id);
                continue;
            }
            let events = crate::workspace::watched_events(&root, changes);
            if events.is_empty() {
                continue;
            }
            let note = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "workspace/didChangeWatchedFiles",
                "params": { "changes": events }
            });
            let pending = PendingServerFrame {
                body: note.to_string(),
                deadline: Instant::now() + write_budget,
            };
            if input.try_send(pending).is_err() {
                retire.push(id);
            }
        }
        for id in retire {
            // A watched-file notification is mandatory. A full or closed input means that
            // this transport has lost part of its stream and must never be reused.
            self.remove(id, true);
        }
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    /// A Rust editor server whose root a synced prod-code.toml now forbids build scripts in (at
    /// its root or above it) is retired; one under an unrelated change keeps running.
    #[tokio::test]
    async fn a_rust_editor_server_is_retired_when_its_policy_forbids_build_scripts() {
        let checkout = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(checkout.path().join(".git")).unwrap();
        let root = checkout.path().join("crates/inner");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"inner\"\n").unwrap();
        let servers = EditorServers::default();
        let (input, _output) = rapidfire::mpsc::bounded(8);
        let mut registration = servers.register(root.clone(), input, Duration::from_secs(1));

        let unrelated = vec![(root.join("src/lib.rs"), WatchedChange::Changed)];
        servers.notify(&unrelated).await;
        assert_eq!(servers.count(), 1, "an ordinary change keeps the server");

        let config = checkout.path().join("prod-code.toml");
        std::fs::write(&config, "[rust]\nbuild_scripts = false\n").unwrap();
        servers.notify(&[(config, WatchedChange::Created)]).await;
        assert_eq!(servers.count(), 0, "the server is retired");
        tokio::time::timeout(Duration::from_secs(1), registration.retired())
            .await
            .expect("its session is told to end");
    }
}
