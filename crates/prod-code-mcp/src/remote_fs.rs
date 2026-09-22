//! Reading source files that live only on the gateway host: what a definition outside the
//! checkout (standard library, dependency caches, SDK headers) points at.

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, ReadFileRequest, WireMessage};
use std::net::SocketAddr;
use std::path::Path;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

/// Reads `path` on the gateway `remote`. Returns the bytes and whether they were truncated.
pub async fn read_remote_file(
    remote: SocketAddr,
    path: &str,
    max_bytes: u64,
) -> Result<(Vec<u8>, bool)> {
    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let _ = stream.set_nodelay(true);
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::ReadFileRequest(ReadFileRequest {
            path: path.to_string(),
            max_bytes,
        }))
        .await?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(15), framed.next())
        .await
        .map_err(|_| anyhow!("timed out reading {path} from {remote}"))?;
    match reply {
        Some(Ok(WireMessage::ReadFileResponse(resp))) => match (resp.content, resp.error) {
            (Some(bytes), _) => Ok((bytes, resp.truncated)),
            (None, Some(err)) => Err(anyhow!(err)),
            (None, None) => Err(anyhow!("empty reply for {path}")),
        },
        Some(Ok(other)) => Err(anyhow!("unexpected reply: {other:?}")),
        Some(Err(e)) => Err(anyhow!("decode error: {e}")),
        None => Err(anyhow!("gateway closed the connection")),
    }
}

/// Whether a location's file lies outside the checkout at `root` (a path the client cannot
/// open itself).
pub fn is_external(root: &Path, file_path: &str) -> bool {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    !Path::new(file_path).starts_with(&root)
}

/// `file://` URI or plain path to a plain path.
pub fn uri_to_path(uri: &str) -> String {
    let raw = uri.strip_prefix("file://").unwrap_or(uri);
    percent_decode(raw)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Lines `line - context ..= line + context` of `text` (1-based `line`), numbered, with the
/// target line marked.
pub fn snippet(text: &str, line: u32, context: u32) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let target = (line.max(1) as usize).min(lines.len());
    let from = target.saturating_sub(context as usize).max(1);
    let to = (target + context as usize).min(lines.len());
    let width = to.to_string().len();
    let mut out = String::new();
    for (idx, text) in lines.iter().enumerate().take(to).skip(from - 1) {
        let n = idx + 1;
        let marker = if n == target { ">" } else { " " };
        out.push_str(&format!("{marker}{n:>width$} | {text}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_marks_the_target_line() {
        let text = "a\nb\nc\nd\ne\n";
        let s = snippet(text, 3, 1);
        assert_eq!(s, " 2 | b\n>3 | c\n 4 | d\n");
        assert_eq!(snippet(text, 99, 1), " 4 | d\n>5 | e\n");
        assert_eq!(
            uri_to_path("file:///usr/include/c%2B%2B/13/stdlib.h"),
            "/usr/include/c++/13/stdlib.h"
        );
        assert!(is_external(Path::new("/tmp"), "/usr/include/x.h"));
    }

    #[test]
    fn snippet_of_an_empty_file_is_empty() {
        assert_eq!(snippet("", 1, 2), "");
    }

    #[test]
    fn uri_to_path_accepts_a_plain_path_without_the_file_prefix() {
        assert_eq!(uri_to_path("/already/a/path.rs"), "/already/a/path.rs");
    }

    #[test]
    fn percent_decode_leaves_an_invalid_escape_untouched() {
        // Not valid hex after `%`: kept as literal characters rather than decoded.
        assert_eq!(uri_to_path("file:///tmp/100%zz"), "/tmp/100%zz");
        // A `%` too close to the end to have two hex digits after it is also left alone.
        assert_eq!(uri_to_path("file:///tmp/x%2"), "/tmp/x%2");
    }

    #[test]
    fn is_external_is_false_for_a_path_inside_the_root() {
        // Canonicalized first: `is_external` canonicalizes the root itself, and on macOS a
        // temporary directory is reached through a symlink, so an uncanonicalized join here
        // would not share a prefix with it.
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let inside = root.join("src").join("lib.rs");
        assert!(!is_external(&root, &inside.to_string_lossy()));
    }
}
