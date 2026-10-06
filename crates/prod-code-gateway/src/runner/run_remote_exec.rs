/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

pub async fn run_remote_exec(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: RemoteExecRequest,
) -> Result<()> {
    run_remote_exec_with_ram(
        storage_root,
        metrics,
        workspace_manager,
        framed,
        req,
        false,
        None,
    )
    .await
}

pub async fn run_remote_exec_with_ram(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: RemoteExecRequest,
    build_cache_ram: bool,
    build_cache_dir: Option<&std::path::Path>,
) -> Result<()> {
    let start = Instant::now();
    let workspace = workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let workspace_str = workspace.to_string_lossy().to_string();
    let fail = |error: String| RemoteExecResult {
        exit_code: None,
        duration_ms: 0,
        server_workspace_root: workspace_str.clone(),
        timed_out: false,
        error: Some(error),
        usage: None,
        platform: Some(prod_code_protocol::platform()),
        diagnostics: Vec::new(),
        tests_passed: 0,
        tests_failed: 0,
        tests_skipped: 0,
        test_failures: Vec::new(),
        benches: Vec::new(),
    };

    if !workspace.is_dir() {
        framed
            .send(WireMessage::RemoteExecResult(fail(format!(
                "workspace {workspace_str} is not synced to this gateway"
            ))))
            .await?;
        return Ok(());
    }
    workspace::touch_last_used(&workspace);

    let argv = req.to_argv();
    let Some((program, args)) = argv.split_first() else {
        framed
            .send(WireMessage::RemoteExecResult(fail(
                "empty command".to_string(),
            )))
            .await?;
        return Ok(());
    };

    let timeout_secs = if req.timeout_secs == 0 {
        EXEC_DEFAULT_TIMEOUT_SECS
    } else {
        req.timeout_secs
    };
    if timeout_secs > MAX_REMOTE_EXEC_TIMEOUT_SECS {
        framed
            .send(WireMessage::RemoteExecResult(fail(format!(
                "timeout_secs ({timeout_secs}) exceeds maximum allowed ({MAX_REMOTE_EXEC_TIMEOUT_SECS})"
            ))))
            .await?;
        return Ok(());
    }
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let deadline = match tokio::time::Instant::now().checked_add(timeout) {
        Some(d) => d,
        None => {
            framed
                .send(WireMessage::RemoteExecResult(fail(
                    "timeout_secs overflowed deadline calculation".to_string(),
                )))
                .await?;
            return Ok(());
        }
    };

    match check_node_disk_headroom(&workspace, storage_root) {
        DiskCheckOutcome::Refuse(err) => {
            framed
                .send(WireMessage::RemoteExecResult(fail(err)))
                .await?;
            return Ok(());
        }
        DiskCheckOutcome::Warn(warn) => {
            let _ = framed
                .send(WireMessage::RemoteExecStream(RemoteExecStream::Chunk(
                    ExecChunk {
                        stderr: true,
                        data: Some(warn.into_bytes()),
                    },
                )))
                .await;
        }
        DiskCheckOutcome::Ok => {}
    }

    let snapshot_started = Instant::now();
    let before = Arc::new(if req.pull_changes {
        let root = workspace.clone();
        tokio::task::spawn_blocking(move || snapshot_tree(&root))
            .await
            .unwrap_or_default()
    } else {
        TreeSnapshot::default()
    });

    let run_dir = resolve_run_dir(&workspace, req.subdir.as_deref());

    let ram_target = resolve_ram_build_cache(&workspace, build_cache_ram, build_cache_dir);
    let _ram_lease = ram_target.as_deref().map(RamBuildLease::acquire);
    let (mut cmd, report) = exec_shim::command(program);
    cmd.args(args)
        .current_dir(&run_dir)
        .envs(polyglot_compiler_cache_env(
            &workspace,
            on_path("ccache"),
            ram_target.as_deref(),
        ))
        .envs(req.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    cmd.scrub_cluster_secrets();
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            framed
                .send(WireMessage::RemoteExecResult(fail(format!(
                    "failed to start {program}: {e}"
                ))))
                .await?;
            return Ok(());
        }
    };

    tracing::info!(
        workspace = %workspace_str,
        command = %argv.join(" "),
        language = ?req.language,
        "🛠️ [REMOTE_EXEC] started"
    );
    let _running = RunningEntry::start(&workspace, &argv);

    let (tx, mut rx) = rapidfire::mpsc::bounded::<ExecChunk>(256);
    let readers = spawn_pipe_readers(&mut child, tx.clone());
    drop(tx);

    let pid = child.id();
    let exited = Arc::new(std::sync::Mutex::new(false));
    let (exit_tx, mut exit_rx) = tokio::sync::oneshot::channel();
    {
        let exited = exited.clone();
        tokio::task::spawn_blocking(move || {
            let _ = exit_tx.send(wait_with_usage(pid as i32, &exited));
            drop(child);
        });
    }

    let mut timed_out = false;
    let mut status = None;
    let mut chunks_open = true;
    let mut client_left = false;

    let mut accumulator = JsonStreamAccumulator::new();

    loop {
        tokio::select! {
            chunk = rx.recv(), if chunks_open => match chunk {
                Ok(chunk) => {
                    let is_stderr = chunk.stderr;
                    let chunk_data = chunk.data.clone();
                    if framed.send(WireMessage::RemoteExecStream(RemoteExecStream::Chunk(chunk))).await.is_err() {
                        client_left = true;
                    } else if (req.format == RemoteExecFormat::Json || matches!(req.command, RemoteExecCommand::Test | RemoteExecCommand::Bench)) && !is_stderr
                        && let Some(ref bytes) = chunk_data {
                            if !accumulator.push_chunk(bytes, req.language, framed).await? {
                                client_left = true;
                            }
                        }
                }
                Err(_) => chunks_open = false,
            },
            exit = &mut exit_rx, if status.is_none() => {
                status = Some(exit.ok().flatten());
            }
            _ = tokio::time::sleep_until(deadline), if !timed_out && status.is_none() => {
                timed_out = true;
                kill_exec_group(pid, &exited);
            }
            incoming = framed.next(), if status.is_none() => match incoming {
                Some(Ok(WireMessage::Ping)) => {
                    client_left = framed.send(WireMessage::Pong).await.is_err();
                }
                Some(Ok(WireMessage::Disconnect { .. })) | Some(Err(_)) | None => client_left = true,
                _ => {}
            }
        }
        if client_left || (!chunks_open && status.is_some()) {
            break;
        }
    }

    if client_left {
        kill_exec_group(pid, &exited);
        drop(rx);
        if status.is_none() {
            let _ = exit_rx.await;
        }
        if req.pull_changes {
            let restored =
                restore_after_lost_client(workspace_manager, &workspace, before, snapshot_started)
                    .await;
            tracing::info!(
                workspace = %workspace_str,
                "🛠️ [REMOTE_EXEC] client left; command killed; {restored} file(s) it changed restored"
            );
        } else {
            tracing::info!(workspace = %workspace_str, "🛠️ [REMOTE_EXEC] client left; command killed");
        }
        return Ok(());
    }

    for reader in readers {
        let _ = reader.await;
    }

    if req.format == RemoteExecFormat::Json
        || matches!(
            req.command,
            RemoteExecCommand::Test | RemoteExecCommand::Bench
        )
    {
        let _ = accumulator.flush(req.language, framed).await;
    }

    let report_status = report.as_ref().and_then(exec_shim::ReportFile::read);
    let shim_raw_status = status.flatten();
    let had_report = report.is_some();
    drop(report);
    let (exit_code, usage, exec_err) = if let Some((raw, usage)) = report_status {
        use std::os::unix::process::ExitStatusExt;
        (
            std::process::ExitStatus::from_raw(raw).code(),
            Some(usage),
            None,
        )
    } else if let Some((raw, usage)) = shim_raw_status {
        use std::os::unix::process::ExitStatusExt;
        let exit_status = std::process::ExitStatus::from_raw(raw);
        if exit_status.code() == Some(74) && had_report && !timed_out {
            (
                Some(74),
                Some(usage),
                Some(
                    "exec shim failed to write process report (disk full or write error)"
                        .to_string(),
                ),
            )
        } else {
            (exit_status.code(), Some(usage), None)
        }
    } else {
        (None, None, None)
    };

    if exit_code == Some(254) {
        tokio::task::spawn_blocking(crate::shadow::ensure_sccache_server)
            .await
            .ok();
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    tracing::info!(
        workspace = %workspace_str,
        command = %argv.join(" "),
        exit_code = ?exit_code,
        timed_out,
        duration_ms,
        "🛠️ [REMOTE_EXEC] finished"
    );

    {
        let mut ev = metrics::Event::blank("remote_exec");
        ev.agent = req
            .client_agent
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        ev.host = req
            .client_host
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        ev.workspace = workspace
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ev.command = argv.join(" ");
        ev.method = metrics::command_method(&ev.command);
        ev.engine = req.language.as_str().to_string();
        if metrics::is_compilation_command(&ev.command) {
            ev.compiler = metrics.resolve_compiler(&ev.command);
        }
        ev.duration_ms = duration_ms;
        ev.exit_code = exit_code;
        ev.ok = exit_code == Some(0) && exec_err.is_none();
        if !ev.ok {
            let detail = exec_err
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| ev.command.clone());
            ev.error_class = Some(metrics::classify_error(&detail, exit_code).to_string());
        }
        metrics.record(ev);
    }

    if req.pull_changes {
        let files = send_exec_changes_and_recover(
            workspace_manager,
            &workspace,
            &workspace_str,
            Arc::clone(&before),
            snapshot_started,
            framed,
        )
        .await?;
        if !files.is_empty() {
            refresh_engines(workspace_manager, &workspace, &files).await;
        }
    }

    framed
        .send(WireMessage::RemoteExecResult(RemoteExecResult {
            exit_code,
            duration_ms,
            server_workspace_root: workspace_str,
            timed_out,
            error: exec_err,
            usage,
            platform: Some(prod_code_protocol::platform()),
            diagnostics: accumulator.diagnostics,
            tests_passed: accumulator.tests_passed,
            tests_failed: accumulator.tests_failed,
            tests_skipped: accumulator.tests_skipped,
            test_failures: accumulator.test_failures,
            benches: accumulator.benches,
        }))
        .await?;

    Ok(())
}
