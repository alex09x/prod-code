//! A persistent gateway session for batches of LSP queries: one connection, one pre-flight
//! sync, one handshake and one `initialize`, then any number of requests (documents are
//! opened once). Batch features (impact analysis, dead-code scans) use this instead of a
//! connection per query.

use crate::sync::{WorkspaceIdentity, engine_project, push_workspace_sync, workspace_identity};
use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use url::Url;

pub struct LspSession {
    framed: Framed<TcpStream, ProdCodeCodec>,
    root: PathBuf,
    opened: HashSet<String>,
    next_id: i64,
    /// The engine the gateway chose for this session.
    pub engine: String,
}

impl LspSession {
    /// Opens a session on `remote` for the checkout at `root`. `hint` selects a nested
    /// project (any path inside it); the root project otherwise.
    pub async fn open(remote: SocketAddr, root: &Path, hint: Option<&Path>) -> Result<Self> {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let root_str = root.to_string_lossy().to_string();
        let identity: WorkspaceIdentity = workspace_identity(&root);
        let stream = TcpStream::connect(remote)
            .await
            .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        push_workspace_sync(&mut framed, &root, &identity, None)
            .await
            .context("pre-flight workspace sync failed")?;
        let (engine_subpath, _) = engine_project(&root, hint.unwrap_or(&root));
        framed
            .send(WireMessage::HandshakeRequest(HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                client_name: "prod-code-batch".to_string(),
                client_pid: std::process::id(),
                auth_token: None,
                client_workspace_root: root_str.clone(),
                preferred_engine: None,
                base_workspace_name: Some(identity.name.clone()),
                engine_subpath,
            }))
            .await?;
        let handshake = match framed.next().await {
            Some(Ok(WireMessage::HandshakeResponse(resp))) => resp,
            other => anyhow::bail!("unexpected handshake response: {other:?}"),
        };
        let folder_name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_string();
        let mut session = Self {
            framed,
            root,
            opened: HashSet::new(),
            next_id: 1,
            engine: handshake.detected_engine,
        };
        let init = serde_json::json!({
            "processId": null,
            "rootUri": format!("file://{root_str}"),
            "workspaceFolders": [{ "name": folder_name, "uri": format!("file://{root_str}") }],
            "capabilities": {
                "workspace": { "workspaceFolders": true, "configuration": true },
                "textDocument": {
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "definition": { "linkSupport": true },
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "references": {}
                }
            }
        });
        session.request("initialize", init).await?;
        session.notify("initialized", serde_json::json!({})).await?;
        Ok(session)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    async fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<()> {
        let msg = serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.framed
            .send(WireMessage::LspPayload(msg.to_string()))
            .await?;
        Ok(())
    }

    async fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg =
            serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.framed
            .send(WireMessage::LspPayload(msg.to_string()))
            .await?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("timeout waiting for {method}");
            }
            match tokio::time::timeout(remaining, self.framed.next()).await {
                Ok(Some(Ok(WireMessage::LspPayload(json)))) => {
                    let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) else {
                        continue;
                    };
                    if val.get("id").and_then(|i| i.as_i64()) != Some(id) {
                        continue;
                    }
                    if let Some(err) = val.get("error") {
                        let message = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error");
                        anyhow::bail!("{method} failed: {message}");
                    }
                    return Ok(val
                        .get("result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null));
                }
                Ok(Some(Ok(WireMessage::Ping))) => {
                    let _ = self.framed.send(WireMessage::Pong).await;
                }
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(e))) => anyhow::bail!("frame decode error: {e}"),
                Ok(None) => anyhow::bail!("gateway closed the session"),
                Err(_) => anyhow::bail!("timeout waiting for {method}"),
            }
        }
    }

    /// Opens `file` (once) and runs `method` with `params` on it.
    pub async fn query(
        &mut self,
        file: &Path,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            self.root.join(file)
        };
        let uri = Url::from_file_path(&abs)
            .map_err(|_| anyhow!("invalid file path {}", abs.display()))?
            .to_string();
        if !self.opened.contains(&uri) {
            let text = tokio::fs::read_to_string(&abs)
                .await
                .with_context(|| format!("failed to read {}", abs.display()))?;
            self.notify(
                "textDocument/didOpen",
                serde_json::json!({ "textDocument": {
                    "uri": uri,
                    "languageId": crate::lang::language_id_for_path(&abs),
                    "version": 1,
                    "text": text,
                }}),
            )
            .await?;
            self.opened.insert(uri.clone());
        }
        self.request(method, params).await
    }

    /// Runs `method` on `file` with `text` as its content instead of what is on disk: the
    /// document is opened (or changed) with the proposed text, so the server analyses the
    /// edit without anything being written.
    pub async fn query_with_text(
        &mut self,
        file: &Path,
        text: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            self.root.join(file)
        };
        let uri = Url::from_file_path(&abs)
            .map_err(|_| anyhow!("invalid file path {}", abs.display()))?
            .to_string();
        if self.opened.contains(&uri) {
            self.notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": { "uri": uri, "version": self.next_id },
                    "contentChanges": [ { "text": text } ]
                }),
            )
            .await?;
        } else {
            self.notify(
                "textDocument/didOpen",
                serde_json::json!({ "textDocument": {
                    "uri": uri,
                    "languageId": crate::lang::language_id_for_path(&abs),
                    "version": 1,
                    "text": text,
                }}),
            )
            .await?;
            self.opened.insert(uri.clone());
        }
        self.request(method, params).await
    }

    /// The `file://` URI the session uses for `file`.
    pub fn uri_for(&self, file: &Path) -> Result<String> {
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            self.root.join(file)
        };
        Ok(Url::from_file_path(&abs)
            .map_err(|_| anyhow!("invalid file path {}", abs.display()))?
            .to_string())
    }

    /// Ends the session cleanly.
    pub async fn close(mut self) {
        for uri in std::mem::take(&mut self.opened) {
            let _ = self
                .notify(
                    "textDocument/didClose",
                    serde_json::json!({ "textDocument": { "uri": uri } }),
                )
                .await;
        }
        let _ = self
            .framed
            .send(WireMessage::Disconnect {
                reason: "batch finished".to_string(),
            })
            .await;
    }
}
