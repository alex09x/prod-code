//! The editor's own language server on the node (#332).
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
use prod_code_protocol::{
    AnyStream, PathTranslator, ProdCodeCodec, WireMessage, transport::read_lsp_frame,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::{Instant, timeout_at};
use tokio_util::codec::Framed;

const CHANNEL_CAPACITY: usize = 1024;
const WRITE_BUDGET: Duration = Duration::from_secs(30);
const TEARDOWN_BUDGET: Duration = Duration::from_secs(5);

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
        "java" => from(GenericLspConfig::for_java()),
        "kotlin" => from(GenericLspConfig::for_kotlin()),
        "csharp" => from(GenericLspConfig::for_csharp()),
        "php" => from(GenericLspConfig::for_php()),
        "ruby" => from(GenericLspConfig::for_ruby()),
        "dart" => from(GenericLspConfig::for_dart()),
        "zig" => from(GenericLspConfig::for_zig()),
        "elixir" => from(GenericLspConfig::for_elixir()),
        "scala" => from(GenericLspConfig::for_scala()),
        "lua" => from(GenericLspConfig::for_lua()),
        "haskell" => from(GenericLspConfig::for_haskell()),
        "ocaml" => from(GenericLspConfig::for_ocaml()),
        "clojure" => from(GenericLspConfig::for_clojure()),
        "julia" => from(GenericLspConfig::for_julia()),
        "shell" => from(GenericLspConfig::for_shell()),
        "r" => from(GenericLspConfig::for_r()),
        "erlang" => from(GenericLspConfig::for_erlang()),
        "fsharp" => from(GenericLspConfig::for_fsharp()),
        "perl" => from(GenericLspConfig::for_perl()),
        "solidity" => from(GenericLspConfig::for_solidity()),
        "nim" => from(GenericLspConfig::for_nim()),
        "d" => from(GenericLspConfig::for_d()),
        "fortran" => from(GenericLspConfig::for_fortran()),
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
    servers: std::sync::Mutex<Vec<ServerRegistration>>,
}

struct ServerRegistration {
    id: u64,
    root: PathBuf,
    input: rapidfire::mpsc::Sender<PendingServerFrame>,
    retire: tokio::sync::watch::Sender<bool>,
    write_budget: Duration,
}

struct PendingServerFrame {
    body: String,
    deadline: Instant,
}

struct PendingEditorMessage {
    message: WireMessage,
    deadline: Instant,
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
    async fn retired(&mut self) {
        if !*self.retired.borrow() {
            let _ = self.retired.changed().await;
        }
    }
}

impl EditorServers {
    /// Adds a server started in `root` that takes LSP message bodies on `input`.
    fn register(
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
        for (id, root, input, write_budget) in targets {
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

struct TaskAbortGuard(AbortHandle);

impl TaskAbortGuard {
    fn new<T>(task: &JoinHandle<T>) -> Self {
        Self(task.abort_handle())
    }
}

impl Drop for TaskAbortGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct OwnedChild {
    child: tokio::process::Child,
    process_group: Option<i32>,
    leader_reaped: bool,
    group_retired: bool,
}

impl OwnedChild {
    fn new(child: tokio::process::Child) -> Self {
        #[cfg(unix)]
        let process_group = child.id().and_then(|pid| i32::try_from(pid).ok());
        #[cfg(not(unix))]
        let process_group = None;
        Self {
            child,
            process_group,
            leader_reaped: false,
            group_retired: false,
        }
    }

    fn retire_group(&mut self) {
        if self.group_retired {
            return;
        }
        self.group_retired = true;
        #[cfg(unix)]
        if let Some(group) = self.process_group {
            // The command was put in its own process group before spawn. A negative PID
            // targets only that owned group, including descendants which ignore shutdown.
            let _ = unsafe { libc::kill(-group, libc::SIGKILL) };
        }
        if !self.leader_reaped {
            let _ = self.child.start_kill();
        }
    }

    async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let result = self.child.wait().await;
        if result.is_ok() {
            self.leader_reaped = true;
        }
        result
    }

    async fn retire(&mut self, deadline: Instant) {
        // Reaping the direct child says nothing about descendants which still belong to the
        // exact process group created at spawn. Retire that group once on every exit path.
        self.retire_group();
        if self.leader_reaped {
            return;
        }
        if matches!(timeout_at(deadline, self.child.wait()).await, Ok(Ok(_))) {
            self.leader_reaped = true;
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.retire_group();
        if self.leader_reaped {
            return;
        }
        // Cancellation cannot await. Give the exact child a short synchronous reap window;
        // kill_on_drop remains the final fallback if the platform has not reported it yet.
        for _ in 0..50 {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.leader_reaped = true;
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(2)),
                Err(_) => break,
            }
        }
    }
}

async fn write_server_frames(
    mut stdin: tokio::process::ChildStdin,
    mut input: rapidfire::mpsc::Receiver<PendingServerFrame>,
) -> Result<()> {
    while let Ok(pending) = input.recv().await {
        let bytes = frame(&pending.body);
        timeout_at(pending.deadline, async {
            stdin.write_all(&bytes).await?;
            stdin.flush().await
        })
        .await
        .context("editor server input write exceeded its deadline")?
        .context("writing an editor server input frame")?;
    }
    Ok(())
}

async fn write_editor_messages(
    mut socket: futures_util::stream::SplitSink<Framed<AnyStream, ProdCodeCodec>, WireMessage>,
    mut input: rapidfire::mpsc::Receiver<PendingEditorMessage>,
) -> Result<()> {
    while let Ok(pending) = input.recv().await {
        timeout_at(pending.deadline, socket.send(pending.message))
            .await
            .context("editor socket write exceeded its deadline")?
            .context("writing a message to the editor")?;
    }
    Ok(())
}

async fn finish_task(
    task: &mut JoinHandle<Result<()>>,
    already_finished: bool,
    deadline: Instant,
    session_id: u64,
    task_name: &'static str,
) {
    if already_finished {
        return;
    }
    match timeout_at(deadline, &mut *task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => tracing::warn!(
            session_id,
            task = task_name,
            error = %error,
            "editor task failed during teardown"
        ),
        Ok(Err(error)) => tracing::warn!(
            session_id,
            task = task_name,
            %error,
            "editor task join failed during teardown"
        ),
        Err(_) => {
            tracing::warn!(
                session_id,
                task = task_name,
                "editor task exceeded the teardown deadline; aborting it"
            );
            task.abort();
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(
                    session_id,
                    task = task_name,
                    error = %error,
                    "editor task failed while being aborted during teardown"
                ),
                Err(error) => tracing::warn!(
                    session_id,
                    task = task_name,
                    %error,
                    "editor task join failed after teardown abort"
                ),
            }
        }
    }
}

/// Runs an editor's session: starts `command` in `root` and carries the protocol between the
/// editor on `framed` and the server until either ends.
pub async fn run<S>(
    framed: Framed<S, ProdCodeCodec>,
    translator: PathTranslator,
    command: ServerCommand,
    root: &Path,
    servers: &EditorServers,
    session_id: u64,
) -> Result<()>
where
    S: Into<AnyStream>,
{
    run_with_budgets(
        framed,
        translator,
        command,
        root,
        servers,
        session_id,
        WRITE_BUDGET,
        TEARDOWN_BUDGET,
    )
    .await
}

/// Test injection point for exercising deadlines without changing the product CLI.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub async fn run_with_budgets<S>(
    framed: Framed<S, ProdCodeCodec>,
    translator: PathTranslator,
    command: ServerCommand,
    root: &Path,
    servers: &EditorServers,
    session_id: u64,
    write_budget: Duration,
    teardown_budget: Duration,
) -> Result<()>
where
    S: Into<AnyStream>,
{
    let parts = framed.into_parts();
    let stream: AnyStream = parts.io.into();
    let mut new_parts = tokio_util::codec::FramedParts::new(stream, parts.codec);
    new_parts.read_buf = parts.read_buf;
    new_parts.write_buf = parts.write_buf;
    let framed = Framed::from_parts(new_parts);

    let mut process = tokio::process::Command::new(&command.program);
    process
        .args(&command.args)
        .envs(command.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        process.process_group(0);
    }
    let child = process
        .spawn()
        .with_context(|| format!("starting {} for an editor", command.program))?;
    let mut child = OwnedChild::new(child);
    let stdin = child
        .child
        .stdin
        .take()
        .context("the server has no stdin")?;
    let stdout = child
        .child
        .stdout
        .take()
        .context("the server has no stdout")?;
    let stderr = child
        .child
        .stderr
        .take()
        .context("the server has no stderr")?;
    tracing::info!(session_id, program = %command.program, root = %root.display(), "✏️ [EDITOR] language server started");

    let mut stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .context("reading editor server stderr")?
        {
            tracing::debug!(session_id, "editor server: {line}");
        }
        Ok(())
    });
    let _stderr_guard = TaskAbortGuard::new(&stderr_task);

    let (to_server_tx, to_server_rx) = rapidfire::mpsc::bounded(CHANNEL_CAPACITY);
    let mut registration = servers.register(root.to_path_buf(), to_server_tx.clone(), write_budget);
    let mut writer_task = tokio::spawn(write_server_frames(stdin, to_server_rx));
    let _writer_guard = TaskAbortGuard::new(&writer_task);

    let (socket_tx, mut socket_rx) = framed.split();
    let (to_editor_tx, to_editor_rx) = rapidfire::mpsc::bounded(CHANNEL_CAPACITY);
    let mut socket_writer_task = tokio::spawn(write_editor_messages(socket_tx, to_editor_rx));
    let _socket_writer_guard = TaskAbortGuard::new(&socket_writer_task);
    let reader_translator = translator.clone();
    let reader_tx = to_editor_tx.clone();
    let mut reader_task = tokio::spawn(async move {
        let mut stdout = BufReader::new(stdout);
        while let Some(body) = read_lsp_frame(&mut stdout)
            .await
            .context("reading an editor server output frame")?
        {
            let editor = reader_translator.translate_lsp_to_client(&body);
            let deadline = Instant::now() + write_budget;
            timeout_at(
                deadline,
                reader_tx.send(PendingEditorMessage {
                    message: WireMessage::LspPayload(editor),
                    deadline,
                }),
            )
            .await
            .context("editor output queue exceeded its deadline")?
            .map_err(|_| anyhow::anyhow!("editor output queue closed"))?;
        }
        Ok(())
    });
    let _reader_guard = TaskAbortGuard::new(&reader_task);

    let mut reader_finished = false;
    let mut writer_finished = false;
    let mut socket_writer_finished = false;
    let mut stderr_finished = false;
    loop {
        tokio::select! {
            message = socket_rx.next() => match message {
                Some(Ok(WireMessage::LspPayload(raw))) => {
                    let deadline = Instant::now() + write_budget;
                    if to_server_tx.try_send(PendingServerFrame {
                        body: to_server(&translator, &raw),
                        deadline,
                    }).is_err() {
                        break;
                    }
                }
                Some(Ok(WireMessage::Ping)) => {
                    let deadline = Instant::now() + write_budget;
                    if to_editor_tx.try_send(PendingEditorMessage {
                        message: WireMessage::Pong,
                        deadline,
                    }).is_err() {
                        break;
                    }
                }
                Some(Ok(WireMessage::Disconnect { .. })) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            // The server exited or closed its output: the session is over.
            result = &mut reader_task => {
                reader_finished = true;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => tracing::warn!(session_id, error = %error, "editor server stdout ended with an error"),
                    Err(error) => tracing::warn!(session_id, %error, "editor server stdout task failed"),
                }
                break;
            },
            result = &mut writer_task => {
                writer_finished = true;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => tracing::warn!(session_id, error = %error, "editor server stdin ended with an error"),
                    Err(error) => tracing::warn!(session_id, %error, "editor server stdin task failed"),
                }
                break;
            },
            result = &mut socket_writer_task => {
                socket_writer_finished = true;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => tracing::warn!(session_id, error = %error, "editor socket writer ended with an error"),
                    Err(error) => tracing::warn!(session_id, %error, "editor socket writer task failed"),
                }
                break;
            },
            result = &mut stderr_task, if !stderr_finished => {
                stderr_finished = true;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::warn!(session_id, error = %error, "editor server stderr ended with an error");
                        break;
                    }
                    Err(error) => {
                        tracing::warn!(session_id, %error, "editor server stderr task failed");
                        break;
                    }
                }
            },
            status = child.wait() => {
                if let Err(error) = status {
                    tracing::warn!(session_id, %error, "waiting for editor server failed");
                }
                break;
            },
            _ = registration.retired() => break,
        }
    }
    drop(registration);
    drop(to_server_tx);
    let cleanup_deadline = Instant::now() + teardown_budget;
    // Retire the process tree before draining editor output: a non-reading editor can no
    // longer postpone ownership cleanup, while already queued final messages may still drain.
    child.retire(cleanup_deadline).await;
    finish_task(
        &mut writer_task,
        writer_finished,
        cleanup_deadline,
        session_id,
        "server stdin writer",
    )
    .await;
    finish_task(
        &mut reader_task,
        reader_finished,
        cleanup_deadline,
        session_id,
        "server stdout reader",
    )
    .await;
    finish_task(
        &mut stderr_task,
        stderr_finished,
        cleanup_deadline,
        session_id,
        "server stderr reader",
    )
    .await;
    drop(to_editor_tx);
    // What the server said last still reaches a reading editor, within the same teardown budget.
    finish_task(
        &mut socket_writer_task,
        socket_writer_finished,
        cleanup_deadline,
        session_id,
        "editor socket writer",
    )
    .await;
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
        let note: serde_json::Value =
            serde_json::from_str(&app_rx.recv().await.unwrap().body).unwrap();
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
}
