/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{MAX_REMOTE_SOURCES, remote_sources};
use crate::tools::symbols::types::SymbolHit;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, ReadFileResponse, WireMessage};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

/// A gateway that records every path asked of it. It answers a path ending in `ok.rs` with a
/// text and every other path with an error.
async fn recording_gateway(asked: Arc<Mutex<Vec<String>>>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let asked = Arc::clone(&asked);
            tokio::spawn(async move {
                let mut framed = Framed::new(stream, ProdCodeCodec::new());
                if let Some(Ok(WireMessage::ReadFileRequest(req))) = framed.next().await {
                    asked.lock().unwrap().push(req.path.clone());
                    let ok = req.path.ends_with("ok.rs");
                    let _ = framed
                        .send(WireMessage::ReadFileResponse(ReadFileResponse {
                            path: req.path,
                            content: ok.then(|| b"pub fn scan() {}\n".to_vec()),
                            truncated: false,
                            is_executable: Some(false),
                            error: (!ok).then(|| "no such file".to_string()),
                        }))
                        .await;
                }
            });
        }
    });
    addr
}

fn hit(path: PathBuf) -> SymbolHit {
    SymbolHit {
        path,
        name: "scan".into(),
        kind: "Function",
        container: None,
        line: 1,
        col: 8,
    }
}

#[tokio::test]
async fn failed_reads_count_against_the_limit_and_are_not_repeated() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let remote = recording_gateway(Arc::clone(&asked)).await;
    let gone = |i: usize| PathBuf::from(format!("/nonexistent/prod-code-1413/dep{i}/lib.rs"));
    // Every failing path twice, many more than the limit.
    let hits: Vec<SymbolHit> = (0..40).flat_map(|i| [hit(gone(i)), hit(gone(i))]).collect();
    let texts = remote_sources(remote, &hits).await;
    assert!(texts.is_empty());
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), MAX_REMOTE_SOURCES, "{asked:?}");
    let mut unique = asked.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        asked.len(),
        "a failed path is asked once: {asked:?}"
    );
}

#[tokio::test]
async fn local_files_are_not_asked_and_readable_ones_are_kept() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let remote = recording_gateway(Arc::clone(&asked)).await;
    let local = tempfile::NamedTempFile::new().unwrap();
    let readable = PathBuf::from("/nonexistent/prod-code-1413/dep/ok.rs");
    let hits = [
        hit(local.path().to_path_buf()),
        hit(readable.clone()),
        hit(readable.clone()),
    ];
    let texts = remote_sources(remote, &hits).await;
    assert_eq!(
        texts.get(&readable).map(String::as_str),
        Some("pub fn scan() {}\n")
    );
    assert_eq!(
        *asked.lock().unwrap(),
        [readable.to_string_lossy().into_owned()]
    );
}
