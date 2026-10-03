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

/// One bounded, UTF-8 LSP frame, or `None` only at a clean frame boundary.
pub async fn read_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    prod_code_protocol::transport::read_lsp_frame(reader).await
}

/// The decoded top-level method of a JSON-RPC message. Ordinary methods borrow the input;
/// escaped strings are decoded without building a whole JSON value tree.
pub fn method_of(raw: &str) -> Option<std::borrow::Cow<'_, str>> {
    #[derive(serde::Deserialize)]
    struct Message<'a> {
        #[serde(borrow)]
        method: std::borrow::Cow<'a, str>,
    }
    if !raw.trim_start().starts_with('{') {
        return None;
    }
    Some(serde_json::from_str::<Message<'_>>(raw).ok()?.method)
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
        "java" => "java",
        "kotlin" | "kt" => "kotlin",
        "csharp" | "cs" | "c#" | "dotnet" => "csharp",
        "php" => "php",
        "ruby" | "rb" => "ruby",
        "dart" => "dart",
        "zig" => "zig",
        "elixir" | "ex" | "exs" => "elixir",
        "scala" | "sbt" => "scala",
        "lua" => "lua",
        "haskell" | "hs" => "haskell",
        "ocaml" | "ml" => "ocaml",
        "clojure" | "clj" | "cljs" | "edn" => "clojure",
        "julia" | "jl" => "julia",
        "shell" | "sh" | "bash" | "zsh" => "shell",
        "r" | "rstats" => "r",
        "erlang" | "erl" => "erlang",
        "fsharp" | "fs" | "f#" => "fsharp",
        "perl" | "pl" | "pm" => "perl",
        "solidity" | "sol" => "solidity",
        "nim" => "nim",
        "d" | "dlang" => "d",
        "fortran" | "f90" | "f95" => "fortran",
        "sql" => "sql",
        "graphql" | "gql" => "graphql",
        "proto" | "protobuf" => "protobuf",
        "crystal" | "cr" => "crystal",
        "groovy" | "gvy" => "groovy",
        "ada" | "adb" | "ads" => "ada",
        "v" | "vsh" => "v",
        "racket" | "rkt" => "racket",
        "terraform" | "tf" | "tofu" | "hcl" => "terraform",
        "nix" => "nix",
        "markdown" | "md" => "markdown",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "json" | "jsonc" => "json",
        "html" | "htm" => "html",
        "css" | "scss" | "less" => "css",
        "dockerfile" | "docker" | "containerfile" => "dockerfile",
        "svelte" => "svelte",
        "vue" => "vue",
        "assembly" | "asm" | "s" => "assembly",
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

/// Writes one LSP message to the editor without intermediate heap string formatting.
pub async fn write_frame<W>(writer: &mut W, body: &str) -> std::io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    let mut header_buf = [0u8; 48];
    let mut cursor = std::io::Cursor::new(&mut header_buf[..]);
    let _ = std::io::Write::write_fmt(
        &mut cursor,
        format_args!("Content-Length: {}\r\n\r\n", body.len()),
    );
    let header_len = cursor.position() as usize;
    writer.write_all(&header_buf[..header_len]).await?;
    writer.write_all(body.as_bytes()).await?;
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
    mirror_paths: prod_code_protocol::path::PathTranslator,
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
            mirror_paths: prod_code_protocol::path::PathTranslator::new(
                &mirror.to_string_lossy(),
                "/",
            ),
            mirror,
            client_root: client_root.to_path_buf(),
            server_root: server_root.to_path_buf(),
        }
    }

    /// The editor's message with every path of a mirrored copy turned back into the node's.
    pub fn to_node(&self, raw: &str) -> String {
        self.mirror_paths.translate_lsp_to_server(raw)
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
        prod_code_protocol::path::map_lsp_locations(&mut value, &mut |text| {
            if text.starts_with("file://") {
                uris.push(text.to_string());
            }
            None
        });
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
        prod_code_protocol::path::map_lsp_locations(&mut value, &mut |text| {
            copies.get(text).cloned()
        });
        value.to_string()
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
            method_of(r#"{"jsonrpc":"2.0","method" : "textDocument/didSave","params":{}}"#)
                .as_deref(),
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
    #[tokio::test]
    async fn malformed_editor_frames_are_errors_instead_of_lossy_or_clean_eof() {
        for input in [
            b"Content-Length: 1\r\nContent-Length: 2\r\n\r\n{}".as_slice(),
            b"Content-Length: 2\r\n".as_slice(),
            b"Content-Length: 1\r\n\r\n\xff".as_slice(),
            b"Content-Length: nope\r\n\r\n".as_slice(),
            b"X-Header: value\r\n\r\n".as_slice(),
        ] {
            let mut reader = tokio::io::BufReader::new(input);
            assert!(read_frame(&mut reader).await.is_err(), "accepted {input:?}");
        }
    }

    #[tokio::test]
    async fn oversized_editor_headers_and_bodies_are_rejected_before_allocation() {
        let cases = [
            format!("X-Header: {}", "x".repeat(64 * 1024)).into_bytes(),
            format!(
                "Content-Length: {}\r\n\r\n",
                prod_code_protocol::codec::MAX_FRAME_SIZE + 1
            )
            .into_bytes(),
        ];
        for input in cases {
            let mut reader = tokio::io::BufReader::new(input.as_slice());
            let error = read_frame(&mut reader).await.expect_err("bounded frame");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        }
    }
}

#[cfg(test)]
mod method_boundary_tests {
    use super::method_of;

    #[test]
    fn plain_methods_borrow_and_escaped_methods_decode() {
        assert!(matches!(
            method_of(r#"{"method":"textDocument/didSave"}"#),
            Some(std::borrow::Cow::Borrowed("textDocument/didSave"))
        ));
        assert!(
            matches!(method_of(r#"{"method":"textDocument/did\u0053ave"}"#), Some(std::borrow::Cow::Owned(value)) if value == "textDocument/didSave")
        );
    }

    #[test]
    fn top_level_method_decodes_json_without_taking_nested_fields() {
        for (raw, expected) in [
            (
                r#"{"params":{"method":"nested"},"method":"textDocument/didSave"}"#,
                Some("textDocument/didSave"),
            ),
            (
                r#"{"params":{"method":"textDocument/didSave"},"method":"custom/notify"}"#,
                Some("custom/notify"),
            ),
            (r#"{"params":{"method":"textDocument/didSave"}}"#, None),
            (
                r#"{"meth\u006fd":"textDocument/didSave"}"#,
                Some("textDocument/didSave"),
            ),
            (
                r#"{"method":"textDocument/did\u0053ave"}"#,
                Some("textDocument/didSave"),
            ),
            (r#"{"method":null}"#, None),
            (r#"{"method":42}"#, None),
            (r#"{"method":"first","method":"second"}"#, None),
            (r#"{"method":"unterminated"#, None),
            (r#"[ {"method":"textDocument/didSave"} ]"#, None),
            (r#"["textDocument/didSave"]"#, None),
            (
                r#"{"method":"custom/\"quoted\""}"#,
                Some("custom/\"quoted\""),
            ),
        ] {
            assert_eq!(method_of(raw).as_deref(), expected, "{raw}");
        }
    }
}
