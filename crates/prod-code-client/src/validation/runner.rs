/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::session_state::{
    cli_stream_session_dir, cli_stream_session_path, load_cli_stream_session,
    prune_expired_cli_stream_sessions, write_cli_stream_session,
};
use crate::commands::run_tool;
use crate::workspace::find_workspace_root;
use anyhow::{Context, Result};
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// `validate --compile`: the proposed files through `code_validate_edits` with `compile: true`,
/// so the project's check command runs on them in a shadow copy on the node (#376).
pub async fn run_validate_compiled(
    remote: SocketAddr,
    file: &Path,
    text: String,
    with: &[String],
    borrow_check: bool,
) -> Result<()> {
    let abs = |p: &Path| {
        std::fs::canonicalize(p).unwrap_or_else(|_| {
            env::current_dir()
                .map(|cwd| cwd.join(p))
                .unwrap_or_else(|_| p.to_path_buf())
        })
    };
    let mut edits = vec![serde_json::json!({
        "path": abs(file).to_string_lossy(),
        "new_text": text,
    })];
    for pair in with {
        let (target, from) = pair
            .split_once('=')
            .with_context(|| format!("--with takes FILE=NEW, got `{pair}`"))?;
        let new_text =
            std::fs::read_to_string(from).with_context(|| format!("failed to read {from}"))?;
        edits.push(serde_json::json!({
            "path": abs(Path::new(target)).to_string_lossy(),
            "new_text": new_text,
        }));
    }
    if borrow_check {
        eprintln!(
            "Running the remote compiler check for the proposed changes (with borrow-checker proof)..."
        );
    } else {
        eprintln!("Running the remote compiler check for the proposed changes...");
    }
    run_tool(
        remote,
        "code_validate_edits",
        serde_json::json!({
            "edits": edits,
            "compile": true,
            "borrow_check": borrow_check,
        }),
    )
    .await
}

/// Validate an individual chunk fed into a stateful streaming session (Roadmap 7.7).
pub async fn run_validate_chunk(
    remote: SocketAddr,
    file: &Path,
    session: &str,
    chunk: &str,
    close: bool,
    reset: bool,
    borrow_check: bool,
    json: bool,
) -> Result<()> {
    if chunk.len() > prod_code_mcp::diagnostics::MAX_STREAM_CHUNK_BYTES {
        anyhow::bail!("CLI stream chunk exceeds the 1 MiB limit");
    }
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let dir = cli_stream_session_dir()?;
    let state_path = cli_stream_session_path(&dir, remote, &root, &abs_path, session);
    prune_expired_cli_stream_sessions(&dir, &state_path)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut state_file = options.open(&state_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        state_file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    match state_file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            anyhow::bail!(
                "CLI stream session is already being updated by another process; retry this chunk"
            );
        }
        Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
    }
    let mut state = load_cli_stream_session(&mut state_file, reset, borrow_check)?;
    let previous_chunks = state.chunks.clone();
    let previous_bytes: usize = previous_chunks.iter().map(String::len).sum();
    if previous_bytes.saturating_add(chunk.len())
        > prod_code_mcp::diagnostics::MAX_STREAM_SESSION_BYTES
    {
        anyhow::bail!("accumulated CLI stream session exceeds the 16 MiB limit");
    }
    let manager = prod_code_mcp::diagnostics::stream_manager();
    for (index, previous) in previous_chunks.iter().enumerate() {
        let replay = manager
            .feed_chunk(
                remote,
                &root,
                &abs_path,
                session,
                previous,
                false,
                index == 0,
                borrow_check,
            )
            .await?;
        if replay.intercepted {
            state.closed = true;
            state.chunks.clear();
            state.updated_at_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            write_cli_stream_session(&mut state_file, &state)?;
            drop(state_file);
            anyhow::bail!(
                "persisted CLI stream session is intercepted; start a new session with --reset"
            );
        }
    }
    let res = manager
        .feed_chunk(
            remote,
            &root,
            &abs_path,
            session,
            chunk,
            close,
            previous_chunks.is_empty(),
            borrow_check,
        )
        .await?;
    state.updated_at_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    state.closed = close || res.intercepted;
    if state.closed {
        state.chunks.clear();
    } else {
        state.chunks.push(chunk.to_string());
    }
    write_cli_stream_session(&mut state_file, &state)?;
    drop(state_file);
    if json {
        println!("{}", serde_json::to_string_pretty(&res)?);
    } else {
        print!("{}", res.render());
    }
    if res.intercepted {
        std::process::exit(1);
    }
    Ok(())
}

/// Validate code as streamed line-by-line or chunk-by-chunk from stdin (or a file),
/// intercepting hallucinated methods and type errors on the fly before turn completion (Roadmap 7.7).
pub async fn run_validate_stream(
    remote: SocketAddr,
    file: &Path,
    from: Option<PathBuf>,
    session_id: Option<String>,
    borrow_check: bool,
    json: bool,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let session = session_id.unwrap_or_else(|| {
        let counter = prod_code_mcp::diagnostics::next_batch_counter();
        format!("stream-{}-{}", std::process::id(), counter)
    });
    let mgr = prod_code_mcp::diagnostics::stream_manager();
    mgr.reset_session(remote, &root, &abs_path, &session);

    let started = std::time::Instant::now();
    let mut chunk_idx = 0;

    if let Some(from_path) = from {
        let content = std::fs::read_to_string(&from_path)
            .with_context(|| format!("failed to read {}", from_path.display()))?;
        let lines: Vec<&str> = content.split_inclusive('\n').collect();
        let total = lines.len();
        for (i, line) in lines.iter().enumerate() {
            chunk_idx += 1;
            let is_last = i + 1 == total;
            let is_first = i == 0;
            let res = mgr
                .feed_chunk(
                    remote,
                    &root,
                    &abs_path,
                    &session,
                    line,
                    is_last,
                    is_first,
                    borrow_check,
                )
                .await?;
            if res.intercepted {
                if json {
                    println!("{}", serde_json::to_string_pretty(&res)?);
                } else {
                    print!("{}", res.render());
                    eprintln!(
                        "[prod-code stream] INTERCEPTED on-the-fly at line/chunk {} ({:.2}s)",
                        chunk_idx,
                        started.elapsed().as_secs_f64()
                    );
                }
                std::process::exit(1);
            }
            if is_last {
                if json {
                    println!("{}", serde_json::to_string_pretty(&res)?);
                } else {
                    print!("{}", res.render());
                    eprintln!(
                        "[prod-code stream] {} chunk(s) validated clean in {:.2}s",
                        chunk_idx,
                        started.elapsed().as_secs_f64()
                    );
                }
            }
        }
    } else {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        let mut reader = std::io::BufReader::new(stdin.lock());
        let mut line = String::new();
        while reader.read_line(&mut line)? > 0 {
            chunk_idx += 1;
            let is_first = chunk_idx == 1;
            let res = mgr
                .feed_chunk(
                    remote,
                    &root,
                    &abs_path,
                    &session,
                    &line,
                    false,
                    is_first,
                    borrow_check,
                )
                .await?;
            line.clear();
            if res.intercepted {
                if json {
                    println!("{}", serde_json::to_string_pretty(&res)?);
                } else {
                    print!("{}", res.render());
                    eprintln!(
                        "[prod-code stream] INTERCEPTED on-the-fly at chunk {} after {:.2}s - terminating stream",
                        chunk_idx,
                        started.elapsed().as_secs_f64()
                    );
                }
                std::process::exit(1);
            }
        }
        // End of stream from stdin: close the session and run final validation & borrow checking
        let is_first = chunk_idx == 0;
        let final_res = mgr
            .feed_chunk(
                remote,
                &root,
                &abs_path,
                &session,
                "",
                true,
                is_first,
                borrow_check,
            )
            .await?;
        if json {
            println!("{}", serde_json::to_string_pretty(&final_res)?);
        } else {
            print!("{}", final_res.render());
            eprintln!(
                "[prod-code stream] {} chunk(s) streamed and validated in {:.2}s",
                chunk_idx,
                started.elapsed().as_secs_f64()
            );
        }
        if final_res.intercepted {
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Several proposed files checked together in one overlay, so a change to one is judged against
/// the proposed state of the others (a constant added in one file and imported in another).
pub async fn run_validate_together(
    remote: SocketAddr,
    file: &Path,
    text: String,
    with: &[String],
    json: bool,
) -> Result<()> {
    let abs = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let first = abs(file);
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&first).unwrap_or(cwd);
    let mut edits = vec![(first, text)];
    for pair in with {
        let (target, from) = pair
            .split_once('=')
            .with_context(|| format!("--with takes FILE=NEW, got `{pair}`"))?;
        let new_text =
            std::fs::read_to_string(from).with_context(|| format!("failed to read {from}"))?;
        edits.push((abs(Path::new(target)), new_text));
    }
    let started = std::time::Instant::now();
    let reports = prod_code_mcp::diagnostics::validate_texts(remote, &root, &edits, &[]).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        for report in &reports {
            print!("{}", report.render());
        }
        eprintln!(
            "[prod-code] {} file(s) analysed together in {:.2}s",
            edits.len(),
            started.elapsed().as_secs_f64()
        );
    }
    if reports.iter().any(|r| !r.ok()) {
        std::process::exit(1);
    }
    Ok(())
}
