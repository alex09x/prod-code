//! Reading source files that live only on the gateway host: what a definition outside the
//! checkout (standard library, dependency caches, SDK headers) points at.

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, ReadFileRequest, WireMessage};
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio_util::codec::Framed;

/// Maximum bytes read from a local source file before truncating (matches gateway ReadFile limit).
pub const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

/// Reads `path` on the gateway `remote`. Returns the bytes and whether they were truncated.
pub async fn read_remote_file(
    remote: SocketAddr,
    path: &str,
    max_bytes: u64,
) -> Result<(Vec<u8>, bool)> {
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
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

/// Reads a source file. If `path_str` is relative or a local absolute path inside `root`,
/// it is read directly from disk in the local checkout (capped at 2 MiB).
/// If `path_str` is an external absolute path (e.g. stdlib, dependency cache, SDK headers),
/// it is fetched from the remote gateway via `read_remote_file`.
/// Relative paths that attempt to escape `root` are refused.
pub async fn read_source(
    remote: SocketAddr,
    root: &Path,
    path_str: &str,
) -> Result<(Vec<u8>, bool)> {
    let path = uri_to_path(path_str);
    let p = Path::new(&path);
    let root_canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());

    if p.is_absolute() {
        if let Some(local_path) = absolute_checkout_source_path(root, &root_canon, p)? {
            read_local_source_file(&local_path)
        } else {
            read_remote_file(remote, &path, 0).await
        }
    } else {
        let local_path = resolve_relative_checkout_path(root, &root_canon, p)?;
        read_local_source_file(&local_path)
    }
}

fn absolute_checkout_source_path(
    root: &Path,
    root_canon: &Path,
    path: &Path,
) -> Result<Option<PathBuf>> {
    let claims_checkout = path.starts_with(root) || path.starts_with(root_canon);
    if claims_checkout
        && path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        anyhow::bail!("{} is outside the workspace", path.display());
    }

    match std::fs::canonicalize(path) {
        Ok(canonical) if canonical.starts_with(root_canon) => Ok(Some(canonical)),
        Ok(_) if claims_checkout => {
            anyhow::bail!("{} is outside the workspace", path.display())
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && claims_checkout => {
            let ancestor = canonical_existing_ancestor(path)?;
            if !ancestor.starts_with(root_canon) {
                anyhow::bail!("{} is outside the workspace", path.display());
            }
            Err(error).with_context(|| format!("reading {}", path.display()))
        }
        Err(error) if claims_checkout => {
            Err(error).with_context(|| format!("resolving {}", path.display()))
        }
        Err(_) => Ok(None),
    }
}

fn canonical_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut ancestor = path;
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                return std::fs::canonicalize(ancestor)
                    .with_context(|| format!("resolving {}", ancestor.display()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = ancestor
                    .parent()
                    .context("source path has no existing ancestor")?;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("resolving {}", ancestor.display()));
            }
        }
    }
}

fn read_local_source_file(path: &Path) -> Result<(Vec<u8>, bool)> {
    let file = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    read_limited_source(file).with_context(|| format!("reading {}", path.display()))
}

fn read_limited_source(reader: impl Read) -> Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading source content")?;
    let truncated = bytes.len() as u64 > MAX_SOURCE_BYTES;
    bytes.truncate(MAX_SOURCE_BYTES as usize);
    Ok((bytes, truncated))
}

fn resolve_relative_checkout_path(root: &Path, root_canon: &Path, p: &Path) -> Result<PathBuf> {
    let mut norm = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !norm.pop() {
                    anyhow::bail!("{} is outside the workspace", p.display());
                }
            }
            std::path::Component::Normal(c) => norm.push(c),
            _ => anyhow::bail!("{} is outside the workspace", p.display()),
        }
    }

    let candidate = if let Ok(cwd) = std::env::current_dir() {
        let cwd_canon = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
        if cwd_canon.starts_with(root_canon) && cwd.join(&norm).exists() {
            cwd.join(&norm)
        } else {
            root.join(&norm)
        }
    } else {
        root.join(&norm)
    };

    let resolved = if candidate.exists() {
        std::fs::canonicalize(&candidate)
            .with_context(|| format!("reading {}", candidate.display()))?
    } else {
        let mut cur = candidate.as_path();
        while !cur.exists() {
            if let Some(parent) = cur.parent() {
                cur = parent;
            } else {
                break;
            }
        }
        if let Ok(cur_canon) = std::fs::canonicalize(cur) {
            if !cur_canon.starts_with(root) && !cur_canon.starts_with(root_canon) {
                anyhow::bail!("{} is outside the workspace", candidate.display());
            }
        }
        candidate.clone()
    };

    if !resolved.starts_with(root) && !resolved.starts_with(root_canon) {
        anyhow::bail!("{} is outside the workspace", candidate.display());
    }

    Ok(candidate)
}

/// Whether a location's file lies outside the checkout at `root` (a path the client cannot
/// open itself).
pub fn is_external(root: &Path, file_path: &str) -> bool {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path = Path::new(file_path);
    if !path.is_absolute() {
        return false;
    }
    let path_canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    !path.starts_with(&root) && !path_canon.starts_with(&root)
}

/// `file://` URI or plain path to a plain path. Only a URI is percent-decoded: a plain path is
/// already decoded, and a file named `100%41.rs` must not turn into `100A.rs`.
pub fn uri_to_path(uri: &str) -> String {
    prod_code_protocol::path::uri_or_path(uri)
        .to_string_lossy()
        .into_owned()
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
    fn uri_to_path_decodes_a_uri_once_and_a_plain_path_never() {
        // A literal `%41` in a file name is `%2541` in its URI, and stays `%41` in the path.
        assert_eq!(
            uri_to_path("file:///w/my%20app%20%231/100%2541%20%C3%BC.rs"),
            "/w/my app #1/100%41 ü.rs"
        );
        assert_eq!(uri_to_path("/w/100%41.rs"), "/w/100%41.rs");
        assert_eq!(uri_to_path("untitled:Untitled-1"), "untitled:Untitled-1");
        assert_eq!(uri_to_path("file://remote/tmp/%€"), "remote/tmp/%€");
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
        assert!(!is_external(&root, "src/lib.rs"));
    }

    #[tokio::test]
    async fn read_source_reads_local_file_and_refuses_escaping_relative_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let src = root.join("crates").join("my-crate");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("lib.rs"), "pub fn test_local() {}\n").unwrap();

        let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();

        let (bytes, truncated) = read_source(addr, root, "crates/my-crate/lib.rs").await.unwrap();
        assert!(!truncated);
        assert_eq!(String::from_utf8(bytes).unwrap(), "pub fn test_local() {}\n");

        let (bytes_abs, _) = read_source(addr, root, &src.join("lib.rs").to_string_lossy())
            .await
            .unwrap();
        assert_eq!(String::from_utf8(bytes_abs).unwrap(), "pub fn test_local() {}\n");

        let err = read_source(addr, root, "../../etc/passwd").await.unwrap_err();
        assert!(format!("{err:#}").contains("outside the workspace"));
    }

    #[tokio::test]
    async fn read_source_refuses_absolute_parent_and_symlink_escapes() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        let outside = parent.path().join("private.rs");
        std::fs::write(&outside, "private").unwrap();
        let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();

        let traversal = root.join("../private.rs");
        let err = read_source(addr, &root, &traversal.to_string_lossy())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("outside the workspace"),
            "{err:#}"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let external = parent.path().join("external");
            std::fs::create_dir(&external).unwrap();
            std::fs::write(external.join("private.py"), "private").unwrap();
            symlink(&external, root.join("linked")).unwrap();
            let linked = root.join("linked/private.py");
            let err = read_source(addr, &root, &linked.to_string_lossy())
                .await
                .unwrap_err();
            assert!(
                format!("{err:#}").contains("outside the workspace"),
                "{err:#}"
            );
        }
    }

    #[test]
    fn bounded_source_reader_stops_after_one_truncation_byte() {
        use std::cell::Cell;

        struct CountingReader<'a> {
            bytes: &'a [u8],
            read: &'a Cell<usize>,
        }

        impl std::io::Read for CountingReader<'_> {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                let count = output.len().min(self.bytes.len());
                output[..count].copy_from_slice(&self.bytes[..count]);
                self.bytes = &self.bytes[count..];
                self.read.set(self.read.get() + count);
                Ok(count)
            }
        }

        let data = vec![b'x'; MAX_SOURCE_BYTES as usize + 4096];
        let read = Cell::new(0);
        let reader = CountingReader {
            bytes: &data,
            read: &read,
        };
        let (bytes, truncated) = read_limited_source(reader).unwrap();
        assert!(truncated);
        assert_eq!(bytes.len(), MAX_SOURCE_BYTES as usize);
        assert_eq!(read.get() as u64, MAX_SOURCE_BYTES + 1);
    }
}
