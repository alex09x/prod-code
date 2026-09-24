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

/// What in a file makes its meaning depend on the platform: conditional compilation on the
/// target, and the C library, whose signatures differ between Linux and Apple (#140).
const PLATFORM_MARKS: &[&str] = &[
    "cfg(target_",
    "cfg(unix)",
    "cfg(windows)",
    "libc::",
    "#ifdef __APPLE__",
    "#if defined(__APPLE__)",
    "#ifdef __linux__",
    "#if os(",
    "//go:build ",
];

/// A warning for files a command rewrote on a node of another OS than this machine's, when
/// they hold code whose meaning depends on the platform, or the checkout is an Apple project: a
/// lint or a fix computed there can be wrong here, and it was written back as a success.
/// `None` when the platforms match, nothing was written, or nothing looks platform-specific.
pub fn platform_warning(root: &Path, node: Option<&str>, pulled: &[String]) -> Option<String> {
    let node = node?;
    let here = prod_code_protocol::platform();
    let os = |p: &str| p.split(' ').next().unwrap_or("").to_string();
    if pulled.is_empty() || os(node) == os(&here) {
        return None;
    }
    let marked: Vec<&String> = pulled
        .iter()
        .filter(|rel| {
            std::fs::read_to_string(root.join(rel))
                .is_ok_and(|text| PLATFORM_MARKS.iter().any(|m| text.contains(m)))
        })
        .collect();
    let apple = root.join("Package.swift").is_file()
        || std::fs::read_dir(root).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x == "xcodeproj" || x == "xcworkspace")
            })
        });
    if marked.is_empty() && !apple {
        return None;
    }
    let what = if marked.is_empty() {
        "this checkout is also an Apple project".to_string()
    } else {
        format!(
            "{} of them hold code that depends on the platform ({})",
            marked.len(),
            marked
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Some(format!(
        "warning: the command ran on {node} and this machine is {here}; {what}. A fix or a lint computed there can be wrong here (a `cfg` it never compiled, a libc signature that differs), so build the checkout here before trusting it."
    ))
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
    let _ = stream.set_nodelay(true);
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
            client_agent: Some(prod_code_protocol::detect_client_agent()),
            client_host: Some(prod_code_protocol::client_host()),
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
    fn files_rewritten_on_another_platform_are_named_when_they_depend_on_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/pty.rs"),
            "unsafe { libc::openpty(m, s, p, t, &mut w) };\n",
        )
        .unwrap();
        std::fs::write(root.join("src/plain.rs"), "pub fn add(a: u8) -> u8 { a }\n").unwrap();
        let here = prod_code_protocol::platform();
        let other = if cfg!(target_os = "macos") {
            "linux x86_64"
        } else {
            "macos aarch64"
        };
        let both = vec!["src/pty.rs".to_string(), "src/plain.rs".to_string()];
        let plain = vec!["src/plain.rs".to_string()];
        assert_eq!(platform_warning(root, Some(&here), &both), None);
        assert_eq!(platform_warning(root, None, &both), None);
        assert_eq!(platform_warning(root, Some(other), &[]), None);
        assert_eq!(platform_warning(root, Some(other), &plain), None);
        let warning = platform_warning(root, Some(other), &both).expect("a libc call is named");
        assert!(
            warning.contains(&format!("ran on {other} and this machine is {here}")),
            "{warning}"
        );
        assert!(
            warning.contains("1 of them hold code that depends on the platform (src/pty.rs)"),
            "{warning}"
        );
        std::fs::write(root.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();
        let apple = platform_warning(root, Some(other), &plain).expect("an Apple project is named");
        assert!(apple.contains("also an Apple project"), "{apple}");
    }

    #[test]
    fn tail_buffer_keeps_only_the_end() {
        let mut tail = TailBuffer::new(8);
        tail.push(b"0123456789");
        tail.push(b"ab");
        assert_eq!(tail.text(), "456789ab");
        assert_eq!(tail.total, 12);
    }

    #[test]
    fn tail_buffer_keeps_everything_under_the_limit() {
        let mut tail = TailBuffer::new(8);
        tail.push(b"ab");
        assert_eq!(tail.text(), "ab");
        assert_eq!(tail.total, 2);
    }

    #[test]
    fn subdir_of_is_none_for_the_root_itself() {
        let root = std::env::temp_dir();
        assert_eq!(subdir_of(&root, &root), None);
    }

    #[test]
    fn subdir_of_is_the_relative_slash_separated_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(subdir_of(&root, &nested).as_deref(), Some("a/b"));
    }

    #[test]
    fn subdir_of_is_none_outside_the_root() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(a.path()).unwrap();
        let outside = std::fs::canonicalize(b.path()).unwrap();
        assert_eq!(subdir_of(&root, &outside), None);
    }
}
