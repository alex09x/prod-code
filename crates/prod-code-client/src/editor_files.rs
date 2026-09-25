//! Files an editor is pointed at that exist only on the node (#333).
//!
//! A language server on the node resolves definitions into the standard library, the dependency
//! caches and the files a build generates in the node's copy of the checkout. Those are paths
//! of another machine, which the editor cannot open. The `lsp` bridge copies each such file
//! into a local mirror when a message names it, and names the copy instead; a message from the
//! editor about a copy names the node's path again, so the server knows the file.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// One LSP frame from `reader`: its body, or `None` at the end of the stream. Header names are
/// matched without regard to case, and headers other than the length are skipped.
pub async fn read_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let header = line.trim_end();
        if header.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length.unwrap_or(0)];
    reader.read_exact(&mut body).await?;
    Ok(Some(String::from_utf8_lossy(&body).into_owned()))
}

/// The method of an LSP message, read without parsing the whole of it: the first
/// `"method":"…"` in the text.
pub fn method_of(raw: &str) -> Option<&str> {
    let at = raw.find("\"method\"")?;
    let rest = raw[at + 8..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    rest.find('"').map(|end| &rest[..end])
}

/// The language `prod-code lsp --language` names, as the engine that serves it.
pub fn engine_for_language(language: &str) -> Option<&'static str> {
    Some(match language.to_ascii_lowercase().as_str() {
        "rust" => "rust",
        "go" => "go",
        "c" | "cpp" | "c++" | "objc" | "objective-c" => "cpp",
        "python" => "python",
        "typescript" | "javascript" | "tsx" | "jsx" => "typescript",
        "swift" => "swift",
        _ => return None,
    })
}

/// What the editor is told when `prod-code lsp` cannot start a session: the reason and, on
/// macOS, for a node this process was not let through to, where to allow it. A process an app
/// starts reaches the local network only when that app may; connect() fails with
/// `EHOSTUNREACH` otherwise, while the same binary works from a terminal (#338).
pub fn startup_error_message(err: &anyhow::Error) -> String {
    let reason = format!("{err:#}");
    let blocked = reason.contains("os error 65") || reason.contains("No route to host");
    let hint = if cfg!(target_os = "macos") && blocked {
        ". macOS keeps the app that started prod-code off the local network: allow it under \
         System Settings > Privacy & Security > Local Network, then restart the language server"
    } else {
        ""
    };
    format!("prod-code lsp could not start: {reason}{hint}")
}

/// Answers every request the editor sends with `message` as an error, starting with its
/// `initialize`, until it closes the stream or sends `exit`.
pub async fn refuse_session<R, W>(
    reader: &mut R,
    writer: &mut W,
    message: &str,
) -> std::io::Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    while let Some(frame) = read_frame(reader).await? {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&frame) else {
            continue;
        };
        if value.get("method").and_then(|m| m.as_str()) == Some("exit") {
            break;
        }
        let Some(id) = value.get("id").filter(|_| value.get("method").is_some()) else {
            continue;
        };
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32603, "message": message }
        })
        .to_string();
        write_frame(writer, &body).await?;
    }
    Ok(())
}

/// Writes one LSP message to the editor.
pub async fn write_frame<W>(writer: &mut W, body: &str) -> std::io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
        .await?;
    writer.flush().await
}

/// The warning the editor is shown when the checkout could not be pushed before a save: the
/// server still hears of the save, but the check it starts reads the node's previous copy
/// (#350).
pub fn push_failed_warning(err: &anyhow::Error) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "window/showMessage",
        "params": {
            "type": 2,
            "message": format!(
                "prod-code: the checkout could not be pushed to the node ({err:#}); the check \
                 that follows sees the node's previous copy. Save again once the node is reachable."
            ),
        },
    })
    .to_string()
}

/// Node paths whose content never changes once there: a copy of one is never fetched again.
const IMMUTABLE: &[&str] = &[
    "/.cargo/registry/",
    "/.cargo/git/",
    "/.rustup/toolchains/",
    "/go/pkg/mod/",
];

/// The node's files for one editor session, mirrored under a local directory.
pub struct RemoteFiles {
    remote: SocketAddr,
    /// The node's absolute paths live under this directory.
    mirror: PathBuf,
    mirror_uri: String,
    client_root: PathBuf,
    server_root: PathBuf,
}

/// Where the node's files are mirrored by default: the user's cache directory.
pub fn default_cache() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/prod-code/remote")
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache"))
            .join("prod-code/remote")
    }
}

impl RemoteFiles {
    /// The files of the node at `remote` for a checkout at `client_root`, whose copy on the node
    /// is `server_root`, mirrored under `cache` (one directory per node).
    pub fn new(remote: SocketAddr, client_root: &Path, server_root: &Path, cache: &Path) -> Self {
        let mirror = cache.join(remote.to_string().replace(':', "_"));
        Self {
            remote,
            mirror_uri: format!("file://{}", mirror.display()),
            mirror,
            client_root: client_root.to_path_buf(),
            server_root: server_root.to_path_buf(),
        }
    }

    /// The editor's message with every path of a mirrored copy turned back into the node's.
    pub fn to_node(&self, raw: &str) -> String {
        let mirror = self.mirror.to_string_lossy();
        if !raw.contains(mirror.as_ref()) {
            return raw.to_string();
        }
        raw.replace(&self.mirror_uri, "file://")
            .replace(mirror.as_ref(), "")
    }

    /// The node path a `file://` URI of a server's message names, when the editor cannot open
    /// it: a path outside the checkout, or one inside it that only the node's copy has.
    pub fn node_path(&self, uri: &str) -> Option<PathBuf> {
        let path = PathBuf::from(prod_code_mcp::remote_fs::uri_to_path(uri));
        if path.starts_with(&self.mirror) {
            return None;
        }
        match path.strip_prefix(&self.client_root) {
            Ok(_) if path.exists() => None,
            Ok(rel) => Some(self.server_root.join(rel)),
            Err(_) => Some(path),
        }
    }

    /// Where the copy of the node's `node` path lives.
    pub fn mirror_path(&self, node: &Path) -> PathBuf {
        self.mirror.join(node.strip_prefix("/").unwrap_or(node))
    }

    /// The local copy of the file a URI names, fetched from the node unless an unchanging copy
    /// is there already; `None` when the editor can open the URI itself or the node has no
    /// such file to give.
    async fn copy(&self, uri: &str) -> Option<String> {
        let node = self.node_path(uri)?;
        let local = self.mirror_path(&node);
        let node_text = node.to_string_lossy();
        let immutable = IMMUTABLE.iter().any(|part| node_text.contains(part));
        if !(immutable && local.is_file()) {
            let (bytes, _truncated) =
                prod_code_mcp::remote_fs::read_remote_file(self.remote, &node_text, 0)
                    .await
                    .ok()?;
            write_read_only(&local, &bytes).ok()?;
        }
        url::Url::from_file_path(&local).ok().map(|u| u.to_string())
    }

    /// The server's message with each node path the editor cannot open replaced by the path
    /// of its local copy.
    pub async fn to_editor(&self, raw: String) -> String {
        if !raw.contains("file://") {
            return raw;
        }
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return raw;
        };
        let mut uris = Vec::new();
        collect_uris(&value, &mut uris);
        let mut copies = HashMap::new();
        for uri in uris {
            if copies.contains_key(&uri) {
                continue;
            }
            if let Some(local) = self.copy(&uri).await {
                copies.insert(uri, local);
            }
        }
        if copies.is_empty() {
            return raw;
        }
        replace_uris(&mut value, &copies);
        value.to_string()
    }
}

/// Every string in `value` that is a `file://` URI.
fn collect_uris(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) if s.starts_with("file://") => out.push(s.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_uris(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| collect_uris(v, out)),
        _ => {}
    }
}

fn replace_uris(value: &mut serde_json::Value, copies: &HashMap<String, String>) {
    match value {
        serde_json::Value::String(s) => {
            if let Some(local) = copies.get(s.as_str()) {
                *s = local.clone();
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(|v| replace_uris(v, copies)),
        serde_json::Value::Object(map) => map.values_mut().for_each(|v| replace_uris(v, copies)),
        _ => {}
    }
}

/// Writes a copy that is read-only, so that an edit meant for the node's file fails to save
/// rather than going nowhere.
fn write_read_only(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    if path.exists() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))?;
    }
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use prod_code_protocol::{ProdCodeCodec, ReadFileResponse, WireMessage};
    use tokio_util::codec::Framed;

    /// A node that serves `ReadFileRequest` from `files` and counts the reads.
    async fn node(
        files: HashMap<String, String>,
    ) -> (SocketAddr, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&reads);
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let files = files.clone();
                let counter = std::sync::Arc::clone(&counter);
                tokio::spawn(async move {
                    let mut framed = Framed::new(socket, ProdCodeCodec::new());
                    while let Some(Ok(WireMessage::ReadFileRequest(req))) = framed.next().await {
                        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let content = files.get(&req.path).map(|t| t.as_bytes().to_vec());
                        let error = content.is_none().then(|| format!("no {}", req.path));
                        let _ = framed
                            .send(WireMessage::ReadFileResponse(ReadFileResponse {
                                path: req.path,
                                content,
                                truncated: false,
                                error,
                            }))
                            .await;
                    }
                });
            }
        });
        (addr, reads)
    }

    #[tokio::test]
    async fn a_node_path_is_named_by_its_local_copy_and_back() {
        let checkout = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::write(checkout.path().join("lib.rs"), "fn main() {}\n").unwrap();
        let std_file =
            "/home/dev/.rustup/toolchains/stable/lib/rustlib/src/rust/library/alloc/src/vec/mod.rs";
        let generated = "/srv/workspaces/app/target/debug/build/app-1/out/gen.rs";
        let (remote, reads) = node(HashMap::from([
            (std_file.to_string(), "pub struct Vec;\n".to_string()),
            (generated.to_string(), "pub const X: u8 = 1;\n".to_string()),
        ]))
        .await;
        let files = RemoteFiles::new(
            remote,
            checkout.path(),
            Path::new("/srv/workspaces/app"),
            cache.path(),
        );

        let own = format!("file://{}/lib.rs", checkout.path().display());
        let missing_in_checkout = format!(
            "file://{}/target/debug/build/app-1/out/gen.rs",
            checkout.path().display()
        );
        let message = serde_json::json!({
            "jsonrpc": "2.0", "id": 7,
            "result": [
                { "uri": format!("file://{std_file}"), "range": {} },
                { "uri": own, "range": {} },
                { "uri": missing_in_checkout, "range": {} },
                { "uri": format!("file://{std_file}"), "range": {} },
                { "uri": "file:///nowhere/on/the/node.rs", "range": {} }
            ]
        });
        let shown: serde_json::Value =
            serde_json::from_str(&files.to_editor(message.to_string()).await).unwrap();
        let std_copy = files.mirror_path(Path::new(std_file));
        assert_eq!(
            shown["result"][0]["uri"],
            format!("file://{}", std_copy.display())
        );
        assert_eq!(
            std::fs::read_to_string(&std_copy).unwrap(),
            "pub struct Vec;\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&std_copy).unwrap().permissions().mode() & 0o777,
                0o444
            );
        }
        // The checkout's own file stays; a file only the node's copy has is copied too.
        assert_eq!(shown["result"][1]["uri"], own);
        let gen_copy = files.mirror_path(Path::new(generated));
        assert_eq!(
            shown["result"][2]["uri"],
            format!("file://{}", gen_copy.display())
        );
        assert_eq!(shown["result"][3]["uri"], shown["result"][0]["uri"]);
        // A path the node cannot give stays as it was.
        assert_eq!(shown["result"][4]["uri"], "file:///nowhere/on/the/node.rs");
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 3);

        // A toolchain file is not fetched again; a generated one is.
        files.to_editor(message.to_string()).await;
        assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), 5);

        // The editor's messages about a copy name the node's path.
        let hover = format!(
            r#"{{"method":"textDocument/hover","params":{{"textDocument":{{"uri":"file://{}"}}}}}}"#,
            std_copy.display()
        );
        assert_eq!(
            files.to_node(&hover),
            format!(
                r#"{{"method":"textDocument/hover","params":{{"textDocument":{{"uri":"file://{std_file}"}}}}}}"#
            )
        );
        // A message without file URIs is passed through untouched.
        assert_eq!(
            files.to_editor("{\"id\":1}".to_string()).await,
            "{\"id\":1}"
        );
        assert_eq!(files.to_node("{\"id\":1}"), "{\"id\":1}");
    }

    #[tokio::test]
    async fn frames_are_read_whatever_their_headers() {
        let input =
            b"Content-Length: 2\r\n\r\n{}content-length: 8\r\nContent-Type: x\r\n\r\n{\"id\":1}"
                as &[u8];
        let mut reader = tokio::io::BufReader::new(input);
        assert_eq!(
            read_frame(&mut reader).await.unwrap().as_deref(),
            Some("{}")
        );
        assert_eq!(
            read_frame(&mut reader).await.unwrap().as_deref(),
            Some("{\"id\":1}")
        );
        assert_eq!(read_frame(&mut reader).await.unwrap(), None);
    }

    #[test]
    fn a_method_is_read_from_the_text_and_a_language_names_its_engine() {
        assert_eq!(
            method_of(r#"{"jsonrpc":"2.0","method" : "textDocument/didSave","params":{}}"#),
            Some("textDocument/didSave")
        );
        assert_eq!(method_of(r#"{"jsonrpc":"2.0","id":3,"result":null}"#), None);
        assert_eq!(engine_for_language("Rust"), Some("rust"));
        assert_eq!(engine_for_language("C++"), Some("cpp"));
        assert_eq!(engine_for_language("TSX"), Some("typescript"));
        assert_eq!(engine_for_language("swift"), Some("swift"));
        assert_eq!(engine_for_language("cobol"), None);
    }

    #[tokio::test]
    async fn a_session_that_cannot_start_answers_the_editor_with_why() {
        let err = anyhow::anyhow!("No route to host (os error 65)")
            .context("Failed to connect to prod-code gateway at 192.0.2.7:9400");
        let message = startup_error_message(&err);
        assert!(
            message.starts_with("prod-code lsp could not start: Failed to connect"),
            "{message}"
        );
        assert_eq!(
            message.contains("Local Network"),
            cfg!(target_os = "macos"),
            "the hint is macOS's: {message}"
        );
        let frame = |body: &str| format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let input = [
            frame(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#),
            frame(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#),
            frame(r#"{"jsonrpc":"2.0","id":1,"method":"shutdown"}"#),
            frame(r#"{"jsonrpc":"2.0","method":"exit"}"#),
            frame(r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{}}"#),
        ]
        .concat();
        let mut reader = tokio::io::BufReader::new(input.as_bytes());
        let mut written = Vec::new();
        refuse_session(&mut reader, &mut written, &message)
            .await
            .unwrap();
        let mut replies = tokio::io::BufReader::new(written.as_slice());
        let first: serde_json::Value =
            serde_json::from_str(&read_frame(&mut replies).await.unwrap().unwrap()).unwrap();
        assert_eq!(first["id"], 0);
        assert_eq!(first["error"]["message"], message);
        let second: serde_json::Value =
            serde_json::from_str(&read_frame(&mut replies).await.unwrap().unwrap()).unwrap();
        assert_eq!(second["id"], 1, "notifications get no answer");
        assert!(
            read_frame(&mut replies).await.unwrap().is_none(),
            "nothing after exit"
        );
    }

    #[test]
    fn the_cache_is_the_users() {
        assert!(default_cache().ends_with("prod-code/remote"));
    }
}
