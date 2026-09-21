//! Hot reload of the stdio MCP server.
//!
//! An agent session starts `prod-code mcp` once and keeps it for hours, so a newly installed
//! client binary (new tools, schema fixes) stayed invisible until the session was restarted.
//! The server now watches its own executable; when the file on disk changes and no request is
//! in flight, it announces `notifications/tools/list_changed`, flushes, and re-executes the new
//! binary with the same standard streams and arguments. The agent keeps its server, the
//! re-executed process serves the next request with the new code. Nothing is lost: the swap
//! happens only when the input buffer holds no partial or queued request.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use tokio::sync::Notify;

/// Set in the environment of a re-executed server so it knows the session is already
/// initialised and announces its (possibly changed) tool list once more.
pub const RESUMED_ENV: &str = "PROD_CODE_MCP_RESUMED";

/// How often the executable's metadata is compared.
pub const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// A file identity cheap enough to poll: size, modification time and inode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryStamp {
    pub len: u64,
    pub mtime: Option<SystemTime>,
    pub ino: u64,
}

/// The current stamp of `path`, or `None` when it cannot be read (mid-install, deleted).
pub fn stamp(path: &Path) -> Option<BinaryStamp> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    #[cfg(not(unix))]
    let ino = 0;
    Some(BinaryStamp {
        len: meta.len(),
        mtime: meta.modified().ok(),
        ino,
    })
}

/// Whether `path` now differs from `initial` and has stopped changing (a second reading after
/// `settle` matches the first), so a half-written install is never picked up.
pub async fn changed_and_settled(path: &Path, initial: &BinaryStamp, settle: Duration) -> bool {
    let Some(first) = stamp(path) else {
        return false;
    };
    if first == *initial {
        return false;
    }
    tokio::time::sleep(settle).await;
    stamp(path).is_some_and(|second| second == first)
}

/// Polls the executable and, once it has been replaced, raises `flag` and wakes `notify`.
pub fn spawn_watch(exe: PathBuf, initial: BinaryStamp, flag: Arc<AtomicBool>, notify: Arc<Notify>) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if changed_and_settled(&exe, &initial, Duration::from_millis(750)).await {
                tracing::info!(exe = %exe.display(), "installed binary changed; reloading between requests");
                flag.store(true, Ordering::Release);
                notify.notify_one();
                return;
            }
        }
    });
}

/// Replaces this process with `exe`, same arguments and standard streams, marked as resumed.
/// Only returns on failure.
pub fn reexec(exe: &Path) -> std::io::Error {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .env(RESUMED_ENV, "1")
            .exec()
    }
    #[cfg(not(unix))]
    {
        std::io::Error::new(std::io::ErrorKind::Unsupported, "hot reload needs exec(2)")
    }
}

/// The JSON-RPC notification that tells the client to fetch the tool list again.
pub fn tools_list_changed() -> String {
    "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n".to_string()
}

/// Takes the first complete line out of `pending` (without its newline), if any.
pub fn take_line(pending: &mut Vec<u8>) -> Option<String> {
    let pos = pending.iter().position(|b| *b == b'\n')?;
    let line: Vec<u8> = pending.drain(..=pos).collect();
    Some(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_line_returns_complete_lines_and_keeps_the_rest() {
        let mut pending = b"{\"a\":1}\n{\"b\":2}\n{\"c\"".to_vec();
        assert_eq!(take_line(&mut pending).as_deref(), Some("{\"a\":1}"));
        assert_eq!(take_line(&mut pending).as_deref(), Some("{\"b\":2}"));
        assert_eq!(take_line(&mut pending), None);
        assert_eq!(pending, b"{\"c\"".to_vec());
        pending.extend_from_slice(b":3}\n");
        assert_eq!(take_line(&mut pending).as_deref(), Some("{\"c\":3}"));
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn a_replaced_binary_is_detected_only_once_it_settled() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("prod-code");
        std::fs::write(&exe, b"v1").unwrap();
        let initial = stamp(&exe).unwrap();
        assert!(!changed_and_settled(&exe, &initial, Duration::from_millis(0)).await);

        // Atomic install: a new file moved over the old path (new inode, new size).
        let staged = dir.path().join("prod-code.new");
        std::fs::write(&staged, b"version two").unwrap();
        std::fs::rename(&staged, &exe).unwrap();
        assert!(changed_and_settled(&exe, &initial, Duration::from_millis(0)).await);
        assert_ne!(stamp(&exe).unwrap(), initial);

        // A missing file (mid-install) is never a change.
        std::fs::remove_file(&exe).unwrap();
        assert!(!changed_and_settled(&exe, &initial, Duration::from_millis(0)).await);
    }

    #[test]
    fn list_changed_notification_is_one_json_line() {
        let text = tools_list_changed();
        assert!(text.ends_with('\n'));
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(value["method"], "notifications/tools/list_changed");
    }
}
