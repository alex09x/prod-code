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

pub async fn run_exec(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: ExecRequest,
) -> Result<()> {
    run_exec_with_ram(
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

pub async fn run_exec_with_ram(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: ExecRequest,
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
    let fail = |error: String| ExecExit {
        exit_code: None,
        duration_ms: 0,
        server_workspace_root: workspace_str.clone(),
        timed_out: false,
        error: Some(error),
        usage: None,
        platform: Some(prod_code_protocol::platform()),
    };
    if !workspace.is_dir() {
        framed
            .send(WireMessage::ExecExit(fail(format!(
                "workspace {workspace_str} is not synced to this gateway"
            ))))
            .await?;
        return Ok(());
    }
    workspace::touch_last_used(&workspace);
    let Some((program, args)) = req.command.split_first() else {
        framed
            .send(WireMessage::ExecExit(fail("empty command".to_string())))
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
            .send(WireMessage::ExecExit(fail(format!(
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
                .send(WireMessage::ExecExit(fail(
                    "timeout_secs overflowed deadline calculation".to_string(),
                )))
                .await?;
            return Ok(());
        }
    };
    match check_node_disk_headroom(&workspace, storage_root) {
        DiskCheckOutcome::Refuse(err) => {
            framed.send(WireMessage::ExecExit(fail(err))).await?;
            return Ok(());
        }
        DiskCheckOutcome::Warn(warn) => {
            let _ = framed
                .send(WireMessage::ExecChunk(ExecChunk {
                    stderr: true,
                    data: Some(warn.into_bytes()),
                }))
                .await;
        }
        DiskCheckOutcome::Ok => {}
    }
    // Syncs that land after this are the client's newer text, which a restore leaves alone.
    let snapshot_started = Instant::now();
    let before = Arc::new(if req.pull_changes {
        let root = workspace.clone();
        tokio::task::spawn_blocking(move || snapshot_tree(&root))
            .await
            .unwrap_or_default()
    } else {
        TreeSnapshot::default()
    });

    let run_dir = match req.subdir.as_deref() {
        Some(sub)
            if !sub.is_empty() && !sub.starts_with('/') && !sub.split('/').any(|c| c == "..") =>
        {
            let target = workspace.join(sub);
            if !target.is_dir() {
                framed
                    .send(WireMessage::ExecExit(fail(format!(
                        "working directory '{sub}' does not exist in workspace {workspace_str}"
                    ))))
                    .await?;
                return Ok(());
            }
            target
        }
        Some(sub) if !sub.is_empty() => {
            framed
                .send(WireMessage::ExecExit(fail(format!(
                    "invalid working directory '{sub}'"
                ))))
                .await?;
            return Ok(());
        }
        _ => workspace.clone(),
    };
    // A std child, reaped here with `wait4` so its resource use comes back with the exit
    // status (#180); tokio only gets the pipes. The gateway binary starts it through its exec
    // shim, so that the peak memory reported is the command's and not the gateway's (#255).
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
    // The cluster's secret credentials (token and TLS material) are never given to executed commands (#402, Phase 5.6).
    // Scrubbed AFTER request env is applied so that client requests cannot inject or read cluster secrets.
    cmd.scrub_cluster_secrets();
    // Own process group, so a timeout or client disconnect can take down the whole tree
    // (cargo -> test binary -> its helpers), not just the direct child.
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            framed
                .send(WireMessage::ExecExit(fail(format!(
                    "failed to start {program}: {e}"
                ))))
                .await?;
            return Ok(());
        }
    };
    tracing::info!(
        workspace = %workspace_str,
        command = %req.command.join(" "),
        "🛠️ [EXEC] started"
    );
    let _running = RunningEntry::start(&workspace, &req.command);

    // rapidfire (lock-free MPSC): stdout and stderr readers fan in, the session task drains.
    let (tx, mut rx) = rapidfire::mpsc::bounded::<ExecChunk>(256);
    let readers = spawn_pipe_readers(&mut child, tx.clone());
    drop(tx);
    // Reaped on a blocking thread; the pid stays ours to kill until then.
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
    loop {
        tokio::select! {
            chunk = rx.recv(), if chunks_open => match chunk {
                // A client that can no longer be written to is gone as surely as one that
                // hung up, and the command must not outlive it either way.
                Ok(chunk) => client_left = framed.send(WireMessage::ExecChunk(chunk)).await.is_err(),
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
                // A connection that fails is as gone as one that closed: nothing the command
                // changes can reach the client anymore.
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
        // Readers blocked on a full channel see it close and let go of the pipes.
        drop(rx);
        // The command has to be gone before its changes are undone, or it could write again
        // after the restore.
        if status.is_none() {
            let _ = exit_rx.await;
        }
        if req.pull_changes {
            let restored =
                restore_after_lost_client(workspace_manager, &workspace, before, snapshot_started)
                    .await;
            tracing::info!(
                workspace = %workspace_str,
                "🛠️ [EXEC] client left; command killed; {restored} file(s) it changed restored"
            );
        } else {
            tracing::info!(workspace = %workspace_str, "🛠️ [EXEC] client left; command killed");
        }
        return Ok(());
    }
    for reader in readers {
        let _ = reader.await;
    }
    // The shim's report describes the command itself. There is none when the group was killed
    // on a timeout, and then what `wait4` said about the shim stands in for it.
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
        tracing::warn!(
            "exec command exited with 254; ensuring sccache server is running cleanly on host"
        );
        tokio::task::spawn_blocking(crate::shadow::ensure_sccache_server)
            .await
            .ok();
    }
    let duration_ms = start.elapsed().as_millis() as u64;
    tracing::info!(
        workspace = %workspace_str,
        command = %req.command.join(" "),
        exit_code = ?exit_code,
        timed_out,
        duration_ms,
        "🛠️ [EXEC] finished"
    );
    {
        let mut ev = metrics::Event::blank("exec");
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
        ev.command = req.command.join(" ");
        ev.method = metrics::command_method(&ev.command);
        ev.duration_ms = start.elapsed().as_millis() as u64;
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
        .send(WireMessage::ExecExit(ExecExit {
            exit_code,
            duration_ms,
            server_workspace_root: workspace_str,
            timed_out,
            error: exec_err,
            usage,
            platform: Some(prod_code_protocol::platform()),
        }))
        .await?;
    Ok(())
}
