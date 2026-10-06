/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::frames::{
    engine_for_language, method_of, read_frame, refuse_session, startup_error_message,
};
use super::remote::{RemoteFiles, default_cache};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, ReadFileResponse, WireMessage};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
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
                            is_executable: Some(false),
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
    let std_copy = files.mirror_path(Path::new(std_file)).unwrap();
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
    let gen_copy = files.mirror_path(Path::new(generated)).unwrap();
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
    let input = b"Content-Length: 2\r\n\r\n{}content-length: 8\r\nContent-Type: x\r\n\r\n{\"id\":1}"
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
        method_of(r#"{"jsonrpc":"2.0","method" : "textDocument/didSave","params":{}}"#).as_deref(),
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
