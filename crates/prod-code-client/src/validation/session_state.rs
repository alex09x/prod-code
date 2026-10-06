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
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(serde::Deserialize, serde::Serialize, Default)]
pub struct CliStreamSessionState {
    pub version: u32,
    pub chunks: Vec<String>,
    pub borrow_check: bool,
    pub closed: bool,
    pub updated_at_secs: u64,
}

pub const CLI_STREAM_SESSION_TTL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);
pub const MAX_CLI_STREAM_SESSION_FILES: usize = 1024;

pub fn cli_stream_session_dir() -> Result<PathBuf> {
    let base = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(env::temp_dir);
    let dir = base.join("prod-code").join("stream-sessions");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot create CLI stream-session cache {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

pub fn cli_stream_session_path(
    dir: &Path,
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    session: &str,
) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    remote.hash(&mut hasher);
    root.hash(&mut hasher);
    file.hash(&mut hasher);
    session.hash(&mut hasher);
    dir.join(format!("{:016x}.json", hasher.finish()))
}

pub fn prune_expired_cli_stream_sessions(dir: &Path, active: &Path) -> Result<()> {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(CLI_STREAM_SESSION_TTL)
        .unwrap_or(std::time::UNIX_EPOCH);
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path == active || path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file()
            || metadata.modified().is_ok_and(|modified| modified >= cutoff)
        {
            continue;
        }
        let Ok(lock_file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
        else {
            continue;
        };
        if lock_file.try_lock().is_err() {
            continue;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let stale = dir.join(format!(".expired-{}-{stamp}", std::process::id()));
        if std::fs::rename(&path, &stale).is_ok() {
            drop(lock_file);
            let _ = std::fs::remove_file(stale);
        }
    }
    let count = std::fs::read_dir(dir)?
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .count();
    if count >= MAX_CLI_STREAM_SESSION_FILES && !active.exists() {
        anyhow::bail!(
            "maximum persisted CLI stream sessions limit ({MAX_CLI_STREAM_SESSION_FILES}) reached; close or expire an old session"
        );
    }
    Ok(())
}

pub fn load_cli_stream_session(
    file: &mut std::fs::File,
    reset: bool,
    borrow_check: bool,
) -> Result<CliStreamSessionState> {
    use std::io::{Read, Seek};
    if reset {
        return Ok(CliStreamSessionState {
            version: 1,
            borrow_check,
            ..Default::default()
        });
    }
    let metadata = file.metadata()?;
    if metadata.len() == 0 {
        anyhow::bail!("CLI stream session not found; start it with --reset");
    }
    if metadata.len()
        > (prod_code_mcp::diagnostics::MAX_STREAM_SESSION_BYTES as u64) * 6 + 64 * 1024
    {
        anyhow::bail!(
            "persisted CLI stream session exceeds its storage limit; start a new session with --reset"
        );
    }
    if metadata
        .modified()
        .ok()
        .and_then(|modified| std::time::SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > CLI_STREAM_SESSION_TTL)
    {
        anyhow::bail!("CLI stream session expired; start a new session with --reset");
    }
    file.seek(std::io::SeekFrom::Start(0))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)?;
    let state: CliStreamSessionState = serde_json::from_slice(&bytes)
        .context("persisted CLI stream session is invalid; start a new session with --reset")?;
    if state.version != 1 {
        anyhow::bail!(
            "persisted CLI stream session version is unsupported; start a new session with --reset"
        );
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if state.updated_at_secs > 0
        && now.saturating_sub(state.updated_at_secs) > CLI_STREAM_SESSION_TTL.as_secs()
    {
        anyhow::bail!("CLI stream session expired; start a new session with --reset");
    }
    if state.closed {
        anyhow::bail!("CLI stream session is already closed; start a new session with --reset");
    }
    if state.borrow_check != borrow_check {
        anyhow::bail!("borrow-check mode cannot change during a CLI stream session");
    }
    Ok(state)
}

pub fn write_cli_stream_session(file: &mut std::fs::File, state: &CliStreamSessionState) -> Result<()> {
    use std::io::{Seek, Write};
    let bytes = serde_json::to_vec(state)?;
    if bytes.len() > (prod_code_mcp::diagnostics::MAX_STREAM_SESSION_BYTES as usize) * 6 + 64 * 1024
    {
        anyhow::bail!("serialized CLI stream session exceeds its storage limit");
    }
    file.set_len(0)?;
    file.seek(std::io::SeekFrom::Start(0))?;
    file.write_all(&bytes)?;
    file.sync_data()?;
    Ok(())
}
