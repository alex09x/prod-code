/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

async fn send(writer: &mut tokio::process::ChildStdin, message: Value) {
    let text = message.to_string();
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n{text}", text.len()).as_bytes())
        .await
        .unwrap();
    writer.flush().await.unwrap();
}

async fn response(reader: &mut BufReader<tokio::process::ChildStdout>, id: u64) -> Value {
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            assert_ne!(
                reader.read_line(&mut line).await.unwrap(),
                0,
                "clangd closed stdout"
            );
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut bytes = vec![0; length.expect("LSP content length")];
        reader.read_exact(&mut bytes).await.unwrap();
        let message: Value = serde_json::from_slice(&bytes).unwrap();
        if message["id"].as_u64() == Some(id) {
            assert!(message.get("error").is_none(), "clangd error: {message}");
            return message["result"].clone();
        }
    }
}

/// Real clangd identity evidence, replayed through the public API's gateway.
/// Runtime/compiler checks remain independent of the scripted diagnostics.
pub async fn cpp_references(root: &Path, source: &str) -> (Vec<Value>, Vec<Value>) {
    let mut child = tokio::process::Command::new("clangd")
        .args([
            "--background-index=false",
            "--clang-tidy=false",
            "--log=error",
        ])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("required real clangd");
    let mut writer = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        send(&mut writer, json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params": {
            "processId":null, "rootUri":url::Url::from_directory_path(root).unwrap().to_string(),
            "capabilities":{}
        }})).await;
        response(&mut reader, 1).await;
        send(
            &mut writer,
            json!({"jsonrpc":"2.0", "method":"initialized", "params":{}}),
        )
        .await;
        let uri = url::Url::from_file_path(root.join("main.cpp"))
            .unwrap()
            .to_string();
        send(
            &mut writer,
            json!({"jsonrpc":"2.0", "method":"textDocument/didOpen", "params":{
                "textDocument":{"uri":uri,"languageId":"cpp","version":1,"text":source}
            }}),
        )
        .await;
        send(
            &mut writer,
            json!({"jsonrpc":"2.0", "id":2, "method":"textDocument/references", "params":{
                "textDocument":{"uri":uri}, "position":{"line":3,"character":4},
                "context":{"includeDeclaration":true}
            }}),
        )
        .await;
        let refs = response(&mut reader, 2)
            .await
            .as_array()
            .expect("clangd reference list")
            .clone();
        send(
            &mut writer,
            json!({"jsonrpc":"2.0", "id":3,"method":"textDocument/declaration","params":{
                "textDocument":{"uri":uri}, "position":{"line":3,"character":4}
            }}),
        )
        .await;
        let declaration = response(&mut reader, 3).await;
        let declarations = declaration
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![declaration]);
        send(
            &mut writer,
            json!({"jsonrpc":"2.0", "id":4,"method":"shutdown","params":null}),
        )
        .await;
        response(&mut reader, 4).await;
        send(
            &mut writer,
            json!({"jsonrpc":"2.0", "method":"exit","params":null}),
        )
        .await;
        (refs, declarations)
    })
    .await;
    // Always reap the foreground test's language server, including timeout paths.
    if result.is_err() {
        child.kill().await.unwrap();
    }
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    if status.is_err() {
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }
    let (refs, declarations) = result.expect("clangd identity query timed out");
    assert!(!refs.is_empty(), "clangd must resolve the selected symbol");
    for reference in refs.iter().chain(&declarations) {
        let uri = url::Url::parse(reference["uri"].as_str().unwrap()).unwrap();
        let path = uri.to_file_path().unwrap();
        eprintln!(
            "real clangd reference: {}:{}:{}",
            path.file_name().unwrap().to_string_lossy(),
            reference["range"]["start"]["line"].as_u64().unwrap() + 1,
            reference["range"]["start"]["character"].as_u64().unwrap() + 1
        );
    }
    (refs, declarations)
}
