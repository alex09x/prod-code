/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_engine_rust::RustEngine;
use prod_code_gateway::ServerState;
use prod_code_gateway::session::{SharedOutputFrame, SharedOutputSender, on_client_message};
use prod_code_gateway::state::SessionMeta;
use prod_code_gateway::workspace::{SessionView, SharedWorkspace, WorkspaceManager};
use prod_code_protocol::{PathTranslator, WireMessage};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

#[path = "native_rust/bridge.rs"]
pub mod bridge;

pub const SOURCE: &str = concat!(
    "macro_rules! choose { ($x:expr) => { $x + 10 }; }\n",
    "pub fn first() -> i32 { choose!(1) }\n",
    "pub fn second() -> i32 {\n",
    "    macro_rules! choose { ($x:expr) => { $x * 20 }; }\n",
    "    choose!(2)\n",
    "}\n",
);

// Each fixture loads a real crate graph, including the sysroot. Keep this target's memory
// bounded even when cargo invokes its tests concurrently on a many-core build node.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub manager: Arc<WorkspaceManager>,
    pub workspace: Arc<SharedWorkspace>,
    pub state: ServerState,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    pub async fn new(source: &str, edition: &str) -> Self {
        let serial = SERIAL.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("fixture");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), format!(
            "[package]\nname = \"native_extensions\"\nversion = \"0.0.0\"\nedition = \"{edition}\"\n"
        )).unwrap();
        std::fs::write(root.join("src/lib.rs"), source).unwrap();
        std::fs::write(
            root.join("prod-code.toml"),
            "[rust]\nproc_macro_srv = \"disabled\"\nbuild_scripts = false\n",
        )
        .unwrap();
        let load_root = root.clone();
        let engine = tokio::task::spawn_blocking(move || RustEngine::load(&load_root))
            .await
            .unwrap()
            .unwrap();
        let workspace = Arc::new(SharedWorkspace::new(
            root,
            "rust".into(),
            Some(Arc::new(tokio::sync::Mutex::new(engine))),
            None,
            None,
            None,
        ));
        let manager = Arc::new(WorkspaceManager::new());
        manager.insert_ready_for_test(Arc::clone(&workspace)).await;
        let state = ServerState::new(temp.path().join("storage"));
        Self {
            temp,
            manager,
            workspace,
            state,
            _serial: serial,
        }
    }

    pub async fn session(&self, id: u64) -> Session {
        self.session_in(id, &self.workspace).await
    }

    pub async fn session_in(&self, id: u64, workspace: &Arc<SharedWorkspace>) -> Session {
        let root = &workspace.root;
        let lease = self.manager.get_or_load(root, "rust").await.unwrap();
        let view = self
            .manager
            .register_session_view(id, root.clone(), lease)
            .await;
        assert!(view.workspace.rust_engine.is_some());
        assert!(view.workspace.backend.is_none());
        let meta = Arc::new(SessionMeta {
            session_id: id,
            client_name: "native-test".into(),
            agent: "test".into(),
            host: "test".into(),
            client_addr: "test".into(),
            workspace: "fixture".into(),
            engine: "rust".into(),
            engine_root: root.clone(),
            storage_root: root.clone(),
            metrics: Arc::clone(&self.state.metrics),
            editor: false,
            edits: Arc::default(),
        });
        let (tx, rx) = rapidfire::mpsc::bounded(32);
        Session {
            view,
            meta,
            output: SharedOutputSender::new(tx, Duration::from_secs(1)),
            rx,
            translator: PathTranslator::new("/client", &root.to_string_lossy()),
        }
    }

    pub async fn finish(&self, sessions: Vec<Session>) {
        for session in sessions {
            self.manager.unregister_session_view(session.view).await;
        }
    }
}

pub struct Session {
    pub view: SessionView,
    meta: Arc<SessionMeta>,
    output: SharedOutputSender,
    rx: rapidfire::mpsc::Receiver<SharedOutputFrame>,
    translator: PathTranslator,
}

impl Session {
    pub async fn send(&self, request: Value) {
        println!("REQUEST session={} {}", self.view.session_id, request);
        let pending = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        on_client_message(
            Some(Ok(WireMessage::LspPayload(request.to_string()))),
            &self.output,
            &self.translator,
            &self.view,
            &self.meta,
            &pending,
        )
        .await;
    }

    pub async fn receive(&mut self) -> Option<Value> {
        let started = std::time::Instant::now();
        match tokio::time::timeout(Duration::from_secs(65), self.rx.recv()).await {
            Ok(Ok(frame)) => {
                let WireMessage::LspPayload(payload) = frame.message else {
                    panic!("expected LSP")
                };
                let value: Value = serde_json::from_str(&payload).unwrap();
                println!(
                    "RESPONSE session={} elapsed_ms={} {}",
                    self.view.session_id,
                    started.elapsed().as_millis(),
                    value
                );
                Some(value)
            }
            _ => {
                println!(
                    "NO RESPONSE session={} (65 second deadline or closed output)",
                    self.view.session_id
                );
                None
            }
        }
    }

    pub async fn request(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
            .await;
        let response = self
            .receive()
            .await
            .expect("native request must answer within 65 seconds");
        assert_eq!(response["id"], id);
        assert_eq!(response["jsonrpc"], "2.0");
        response
    }

    pub async fn open(&self, source: &str) {
        self.send(json!({"jsonrpc":"2.0", "method":"textDocument/didOpen", "params":{
            "textDocument":{"uri":"file:///client/src/lib.rs", "languageId":"rust", "version":1, "text":source}
        }})).await;
    }

    pub async fn tree(&mut self, id: Value) -> Value {
        let reply = self
            .request(id, "rust-analyzer/viewSyntaxTree", document())
            .await;
        serde_json::from_str(reply["result"].as_str().expect("JSON string tree")).unwrap()
    }

    pub async fn expand(&mut self, id: Value, source: &str, needle: &str) -> Value {
        let mut params = document();
        let byte = source.find(needle).unwrap();
        let prefix = &source[..byte];
        params["position"] = json!({"line":prefix.bytes().filter(|&b| b == b'\n').count(),
            "character":prefix.rsplit('\n').next().unwrap().encode_utf16().count()});
        self.request(id, "rust-analyzer/expandMacro", params).await
    }
}

pub fn document() -> Value {
    json!({"textDocument":{"uri":"file:///client/src/lib.rs"}})
}
