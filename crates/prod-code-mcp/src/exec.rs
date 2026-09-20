//! Remote command execution: sync the checkout, then run a command inside its server copy and
//! stream the output back. Shared by the CLI (`prod-code exec`) and the MCP tool `code_exec`.

use crate::sync::{
    WorkspaceIdentity, apply_pulled_files_for, push_workspace_sync, workspace_identity,
};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ExecExit, ExecRequest, ProdCodeCodec, WireMessage};
use std::net::SocketAddr;
use std::path::Path;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

/// Result of a remote command: its exit record plus the checkout files the command changed on
/// the server and that were written back locally.
#[derive(Debug, Clone)]
pub struct RemoteOutcome {
    pub exit: ExecExit,
    pub pulled_files: Vec<String>,
}

/// The `/`-separated path of `dir` inside `root`, or `None` when `dir` is the root itself
/// or lies outside it: the directory a command runs in on the server.
pub fn subdir_of(root: &Path, dir: &Path) -> Option<String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let rel = dir.strip_prefix(&root).ok()?;
    let rel = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    (!rel.is_empty()).then_some(rel)
}

/// Runs `command` in the server copy of `root`, calling `on_output(is_stderr, bytes)` for
/// every chunk as it arrives. With `pull_changes`, files the command created, changed or
/// deleted on the server are written back into the checkout and recorded in the watermark.
#[allow(clippy::too_many_arguments)]
pub async fn run_remote(
    remote: SocketAddr,
    root: &Path,
    subdir: Option<&str>,
    command: Vec<String>,
    env: Vec<(String, String)>,
    timeout_secs: u64,
    pull_changes: bool,
    mut on_output: impl FnMut(bool, &[u8]),
) -> Result<RemoteOutcome> {
    anyhow::ensure!(!command.is_empty(), "empty command");
    let identity: WorkspaceIdentity = workspace_identity(root);
    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    push_workspace_sync(&mut framed, root, &identity, None)
        .await
        .context("pre-flight workspace sync failed")?;

    framed
        .send(WireMessage::ExecRequest(ExecRequest {
            client_workspace_root: root.to_string_lossy().to_string(),
            base_workspace_name: Some(identity.name.clone()),
            command,
            env,
            timeout_secs,
            pull_changes,
            subdir: subdir.map(str::to_string),
        }))
        .await?;

    let mut pulled_files = Vec::new();
    loop {
        match framed.next().await {
            Some(Ok(WireMessage::ExecChunk(chunk))) => {
                if let Some(data) = chunk.data.as_deref() {
                    on_output(chunk.stderr, data);
                }
            }
            Some(Ok(WireMessage::ExecChanges(changes))) => {
                pulled_files.extend(apply_pulled_files_for(
                    root,
                    &remote.to_string(),
                    &changes.files,
                )?);
            }
            Some(Ok(WireMessage::ExecExit(exit))) => {
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: "exec finished".to_string(),
                    })
                    .await;
                return Ok(RemoteOutcome { exit, pulled_files });
            }
            Some(Ok(WireMessage::Pong)) | Some(Ok(WireMessage::LspPayload(_))) => {}
            Some(Ok(other)) => anyhow::bail!("unexpected message during exec: {other:?}"),
            Some(Err(e)) => anyhow::bail!("frame decode error during exec: {e}"),
            None => anyhow::bail!("gateway closed the connection during exec"),
        }
    }
}

/// Keeps the last `limit` bytes of combined output for a compact tool result.
pub struct TailBuffer {
    limit: usize,
    buf: Vec<u8>,
    pub total: usize,
}

impl TailBuffer {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            buf: Vec::new(),
            total: 0,
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.total += data.len();
        self.buf.extend_from_slice(data);
        if self.buf.len() > self.limit {
            let cut = self.buf.len() - self.limit;
            self.buf.drain(..cut);
        }
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_buffer_keeps_only_the_end() {
        let mut tail = TailBuffer::new(8);
        tail.push(b"0123456789");
        tail.push(b"ab");
        assert_eq!(tail.text(), "456789ab");
        assert_eq!(tail.total, 12);
    }
}
