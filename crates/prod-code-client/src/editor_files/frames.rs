/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

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
