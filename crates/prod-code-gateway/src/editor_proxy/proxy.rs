/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use futures_util::StreamExt;
use prod_code_protocol::{
    AnyStream, PathTranslator, ProdCodeCodec, WireMessage, readiness::Readiness,
};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::{Instant, timeout_at};
use tokio_util::codec::Framed;

use super::child::{
    OwnedChild, TaskAbortGuard, finish_task, write_editor_messages, write_server_frames,
};
use super::command::{ServerCommand, to_server_with_options};
use super::probe::{
    CHANNEL_CAPACITY, EditorProxyOptions, HEALTH_PROBE_ID_PREFIX, HealthProbePending,
    NEXT_HEALTH_PROBE_NAMESPACE, ProbeState, record_liveness,
};
use super::probe_task::spawn_probe_task;
use super::reader::spawn_stdout_reader_task;
use super::registry::{EditorServers, PendingEditorMessage, PendingServerFrame};

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
    run_with_options(
        framed,
        translator,
        command,
        root,
        servers,
        session_id,
        EditorProxyOptions::default(),
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
    run_with_options(
        framed,
        translator,
        command,
        root,
        servers,
        session_id,
        EditorProxyOptions {
            write_budget,
            teardown_budget,
            ..Default::default()
        },
    )
    .await
}

/// Runs an editor's session with specific [`EditorProxyOptions`].
pub async fn run_with_options<S>(
    framed: Framed<S, ProdCodeCodec>,
    translator: PathTranslator,
    command: ServerCommand,
    root: &Path,
    servers: &EditorServers,
    session_id: u64,
    options: EditorProxyOptions,
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
    let mut registration = servers.register(
        root.to_path_buf(),
        to_server_tx.clone(),
        options.write_budget,
    );
    let mut writer_task = tokio::spawn(write_server_frames(stdin, to_server_rx));
    let _writer_guard = TaskAbortGuard::new(&writer_task);

    let (socket_tx, mut socket_rx) = framed.split();
    let (to_editor_tx, to_editor_rx) = rapidfire::mpsc::bounded(CHANNEL_CAPACITY);
    let mut socket_writer_task = tokio::spawn(write_editor_messages(socket_tx, to_editor_rx));
    let _socket_writer_guard = TaskAbortGuard::new(&socket_writer_task);

    let health_probe_id_prefix: Arc<str> = Arc::from(format!(
        "{HEALTH_PROBE_ID_PREFIX}{}:",
        NEXT_HEALTH_PROBE_NAMESPACE.fetch_add(1, Ordering::Relaxed)
    ));
    let probe_state = options
        .probe_state
        .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(ProbeState::default())));
    let health_pending: Arc<std::sync::Mutex<Option<HealthProbePending>>> =
        Arc::new(std::sync::Mutex::new(None));
    let next_probe_id = Arc::new(AtomicU64::new(1));
    let last_activity = Arc::new(std::sync::Mutex::new(Instant::now()));
    let ordinary_epoch = Arc::new(AtomicU64::new(0));
    let lsp_initialized = Arc::new(AtomicBool::new(false));
    let initialize_request_id: Arc<std::sync::Mutex<Option<serde_json::Value>>> =
        Arc::new(std::sync::Mutex::new(None));
    let readiness = Arc::new(Readiness::new(command.ready));
    let in_flight_requests = Arc::new(AtomicUsize::new(0));
    let (retire_tx, mut retire_rx) = tokio::sync::watch::channel(false);

    let probe_task = options.health_probe_interval.map(|interval| {
        spawn_probe_task(
            interval,
            options.health_response_timeout,
            session_id,
            to_server_tx.clone(),
            Arc::clone(&health_pending),
            Arc::clone(&probe_state),
            Arc::clone(&next_probe_id),
            Arc::clone(&health_probe_id_prefix),
            Arc::clone(&last_activity),
            Arc::clone(&ordinary_epoch),
            Arc::clone(&lsp_initialized),
            Arc::clone(&readiness),
            Arc::clone(&in_flight_requests),
            retire_tx.clone(),
        )
    });
    let _probe_guard = probe_task.as_ref().map(TaskAbortGuard::new);

    let mut reader_task = spawn_stdout_reader_task(
        stdout,
        translator.clone(),
        to_editor_tx.clone(),
        Arc::clone(&health_probe_id_prefix),
        Arc::clone(&health_pending),
        Arc::clone(&probe_state),
        Arc::clone(&last_activity),
        Arc::clone(&ordinary_epoch),
        Arc::clone(&lsp_initialized),
        Arc::clone(&initialize_request_id),
        Arc::clone(&readiness),
        Arc::clone(&in_flight_requests),
        options.write_budget,
    );
    let _reader_guard = TaskAbortGuard::new(&reader_task);

    let mut reader_finished = false;
    let mut writer_finished = false;
    let mut socket_writer_finished = false;
    let mut stderr_finished = false;
    loop {
        tokio::select! {
            message = socket_rx.next() => match message {
                Some(Ok(WireMessage::LspPayload(raw))) => {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                        if val.get("method").and_then(|m| m.as_str()) == Some("initialize")
                            && let Some(id) = val.get("id")
                        {
                            *initialize_request_id.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(id.clone());
                        }
                        if val.get("method").is_some()
                            && let Some(id) = val.get("id")
                        {
                            // Reject client requests attempting to use reserved probe prefix
                            if id.as_str().is_some_and(|s| s.starts_with(HEALTH_PROBE_ID_PREFIX)) {
                                let error_reply = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "error": {
                                        "code": -32600,
                                        "message": "Invalid request ID: reserved for health probe"
                                    }
                                });
                                let deadline = Instant::now() + options.write_budget;
                                let _ = to_editor_tx.try_send(PendingEditorMessage {
                                    message: WireMessage::LspPayload(error_reply.to_string()),
                                    deadline,
                                });
                                continue;
                            }
                            if val.get("method").is_some() {
                                in_flight_requests.fetch_add(1, Ordering::AcqRel);
                            }
                        }
                    }
                    record_liveness(&ordinary_epoch, &last_activity, &probe_state);
                    let deadline = Instant::now() + options.write_budget;
                    if to_server_tx.try_send(PendingServerFrame {
                        body: to_server_with_options(
                            &translator,
                            &raw,
                            command.initialization_options.as_ref(),
                        ),
                        deadline,
                    }).is_err() {
                        break;
                    }
                }
                Some(Ok(WireMessage::Ping)) => {
                    let deadline = Instant::now() + options.write_budget;
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
            _ = retire_rx.changed() => {
                if *retire_rx.borrow() {
                    tracing::warn!(session_id, "editor session retired due to health probe failure");
                    break;
                }
            }
        }
    }
    drop(registration);
    drop(to_server_tx);
    let cleanup_deadline = Instant::now() + options.teardown_budget;
    if let Some(mut task) = probe_task {
        task.abort();
        let _ = timeout_at(cleanup_deadline, &mut task).await;
    }
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
