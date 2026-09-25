//! The editor's own language server on the node (#331).
//!
//! An editor that runs `prod-code lsp` wants the language server it would run locally
//! (rust-analyzer, gopls, clangd) with everything that comes with it: the server's own
//! capabilities for the editor's, the editor's settings (`initializationOptions`,
//! `workspace/configuration`), the server's requests to the editor (`client/registerCapability`,
//! `workspace/applyEdit`, progress), check-on-save, and every extension of the protocol it
//! speaks. The shared engines cannot give that: the gateway initialises them once for itself,
//! answers their requests itself, and speaks only what the agents' tools need. So an editor's
//! session gets a server process of its own, started in the node's copy of the checkout; the
//! gateway only carries the protocol between the two and translates paths. The process ends
//! with the session.

use crate::workspace::WatchedChange;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{PathTranslator, ProdCodeCodec, WireMessage};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

/// How to start a language server for an editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Whether editors get servers of their own: `PROD_CODE_EDITOR_SERVERS=off` serves them from
/// the shared engines instead.
pub fn enabled() -> bool {
    std::env::var("PROD_CODE_EDITOR_SERVERS").as_deref() != Ok("off")
}

/// Whether `program --version` runs: a rustup proxy exists for rust-analyzer even where the
/// component is not installed, and then fails.
fn runs(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The language server an editor gets for `engine` on this node, or `None` when the node has
/// none; the session is then served by the shared engines.
pub fn server_command(engine: &str) -> Option<ServerCommand> {
    use prod_code_engine_generic::GenericLspConfig;
    let from = |config: GenericLspConfig| ServerCommand {
        program: config.command,
        args: config.args,
        env: config.env.into_iter().collect(),
    };
    let command = match engine {
        "rust" => ServerCommand {
            program: "rust-analyzer".to_string(),
            args: Vec::new(),
            env: Vec::new(),
        },
        "go" => ServerCommand {
            program: prod_code_engine_go::find_gopls_binary(None)?
                .to_string_lossy()
                .into_owned(),
            args: Vec::new(),
            env: Vec::new(),
        },
        "cpp" => from(GenericLspConfig::for_cpp()),
        "python" => from(GenericLspConfig::for_python()),
        "typescript" => from(GenericLspConfig::for_typescript()),
        "swift" => from(GenericLspConfig::for_swift()),
        _ => return None,
    };
    let installed = if engine == "rust" {
        runs(&command.program)
    } else {
        Path::new(&command.program).is_file()
            || prod_code_engine_generic::which_bin(&command.program).is_ok()
    };
    installed.then_some(command)
}

/// One LSP frame from `reader`: its body, or `None` at the end of the stream. Header names are
/// matched without regard to case, and headers other than the length are skipped.
pub async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let header = line.trim_end();
        if header.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    reader.read_exact(&mut body).await?;
    Ok(Some(String::from_utf8_lossy(&body).into_owned()))
}

/// `body` as one LSP frame.
fn frame(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

/// The editor's message as the server gets it: paths translated, and the editor's process id
/// dropped from `initialize`. That id names a process on the editor's machine; a server that
/// watches its parent would find it missing here, or find someone else's, and exit.
pub fn to_server(translator: &PathTranslator, raw: &str) -> String {
    let translated = translator.translate_lsp_to_server(raw);
    if !translated.contains("\"processId\"") {
        return translated;
    }
    match serde_json::from_str::<serde_json::Value>(&translated) {
        Ok(mut value) if value.get("method").and_then(|m| m.as_str()) == Some("initialize") => {
            if let Some(params) = value.get_mut("params").and_then(|p| p.as_object_mut()) {
                params.insert("processId".to_string(), serde_json::Value::Null);
            }
            value.to_string()
        }
        _ => translated,
    }
}

/// The editors' language servers running on this node, with the roots they were started in,
/// so that a sync can tell each which of its files changed on disk.
#[derive(Default)]
pub struct EditorServers {
    next: AtomicU64,
    servers: std::sync::Mutex<Vec<(u64, PathBuf, rapidfire::mpsc::Sender<String>)>>,
}

/// A server's place in [`EditorServers`], given up when the session ends.
pub struct Registration<'a> {
    servers: &'a EditorServers,
    id: u64,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        let mut servers = self
            .servers
            .servers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        servers.retain(|(id, _, _)| *id != self.id);
    }
}

impl EditorServers {
    /// Adds a server started in `root` that takes LSP message bodies on `input`.
    pub fn register(
        &self,
        root: PathBuf,
        input: rapidfire::mpsc::Sender<String>,
    ) -> Registration<'_> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.servers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((id, root, input));
        Registration { servers: self, id }
    }

    /// How many editor servers are running.
    pub fn count(&self) -> usize {
        self.servers.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Sends `workspace/didChangeWatchedFiles` for the `changes` under each server's root.
    pub async fn notify(&self, changes: &[(PathBuf, WatchedChange)]) {
        let targets: Vec<(PathBuf, rapidfire::mpsc::Sender<String>)> = self
            .servers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(_, root, input)| (root.clone(), input.clone()))
            .collect();
        for (root, input) in targets {
            let events = crate::workspace::watched_events(&root, changes);
            if events.is_empty() {
                continue;
            }
            let note = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "workspace/didChangeWatchedFiles",
                "params": { "changes": events }
            });
            let _ = input.send(note.to_string()).await;
        }
    }
}

/// Runs an editor's session: starts `command` in `root` and carries the protocol between the
/// editor on `framed` and the server until either ends.
pub async fn run(
    framed: Framed<TcpStream, ProdCodeCodec>,
    translator: PathTranslator,
    command: ServerCommand,
    root: &Path,
    servers: &EditorServers,
    session_id: u64,
) -> Result<()> {
    let mut child = tokio::process::Command::new(&command.program)
        .args(&command.args)
        .envs(command.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("starting {} for an editor", command.program))?;
    let mut stdin = child.stdin.take().context("the server has no stdin")?;
    let stdout = child.stdout.take().context("the server has no stdout")?;
    let stderr = child.stderr.take().context("the server has no stderr")?;
    tracing::info!(session_id, program = %command.program, root = %root.display(), "✏️ [EDITOR] language server started");

    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(session_id, "editor server: {line}");
        }
    });

    let (to_server_tx, mut to_server_rx) = rapidfire::mpsc::bounded::<String>(1024);
    let registration = servers.register(root.to_path_buf(), to_server_tx.clone());
    let writer = tokio::spawn(async move {
        while let Ok(body) = to_server_rx.recv().await {
            if stdin.write_all(&frame(&body)).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
    });

    let (mut socket_tx, mut socket_rx) = framed.split();
    let (to_editor_tx, mut to_editor_rx) = rapidfire::mpsc::bounded::<WireMessage>(1024);
    let socket_writer = tokio::spawn(async move {
        while let Ok(message) = to_editor_rx.recv().await {
            if socket_tx.send(message).await.is_err() {
                break;
            }
        }
    });
    let reader_translator = translator.clone();
    let reader_tx = to_editor_tx.clone();
    let mut reader = tokio::spawn(async move {
        let mut stdout = BufReader::new(stdout);
        while let Ok(Some(body)) = read_frame(&mut stdout).await {
            let editor = reader_translator.translate_lsp_to_client(&body);
            if reader_tx
                .send(WireMessage::LspPayload(editor))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            message = socket_rx.next() => match message {
                Some(Ok(WireMessage::LspPayload(raw))) => {
                    if to_server_tx.send(to_server(&translator, &raw)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(WireMessage::Ping)) => {
                    let _ = to_editor_tx.send(WireMessage::Pong).await;
                }
                Some(Ok(WireMessage::Disconnect { .. })) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            // The server exited or closed its output: the session is over.
            _ = &mut reader => break,
        }
    }
    drop(registration);
    reader.abort();
    writer.abort();
    drop(to_editor_tx);
    // What the server said last still reaches the editor.
    let _ = socket_writer.await;
    let _ = child.kill().await;
    tracing::info!(session_id, "✏️ [EDITOR] language server stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_are_read_whatever_the_case_of_their_headers() {
        let input = b"Content-Length: 2\r\n\r\n{}content-length: 13\r\nContent-Type: x\r\n\r\n{\"id\":1}     " as &[u8];
        let mut reader = BufReader::new(input);
        assert_eq!(
            read_frame(&mut reader).await.unwrap().as_deref(),
            Some("{}")
        );
        assert_eq!(
            read_frame(&mut reader).await.unwrap().as_deref(),
            Some("{\"id\":1}     ")
        );
        assert_eq!(read_frame(&mut reader).await.unwrap(), None);
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
        // Anything else passes as it came, translated.
        let hover = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///Users/dev/app/src/lib.rs"}}}"#;
        assert_eq!(
            to_server(&translator, hover),
            hover.replace("/Users/dev/app", "/srv/workspaces/app")
        );
    }

    #[test]
    fn a_server_the_node_lacks_is_not_offered() {
        assert!(server_command("cobol").is_none());
    }

    #[tokio::test]
    async fn a_sync_reaches_the_servers_whose_root_holds_the_files() {
        let servers = EditorServers::default();
        let (app_tx, mut app_rx) = rapidfire::mpsc::bounded::<String>(8);
        let (other_tx, mut other_rx) = rapidfire::mpsc::bounded::<String>(8);
        let app = servers.register(PathBuf::from("/srv/workspaces/app"), app_tx);
        let _other = servers.register(PathBuf::from("/srv/workspaces/other"), other_tx);
        assert_eq!(servers.count(), 2);
        servers
            .notify(&[(
                PathBuf::from("/srv/workspaces/app/src/lib.rs"),
                WatchedChange::Changed,
            )])
            .await;
        let note: serde_json::Value = serde_json::from_str(&app_rx.recv().await.unwrap()).unwrap();
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
}
