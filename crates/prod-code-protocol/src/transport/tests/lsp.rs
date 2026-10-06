/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::lsp::{read_lsp_frame, read_lsp_frame_with_limits};
use std::io::ErrorKind;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn optional_headers_in_either_order_preserve_successive_utf8_frames() {
    for headers in [
        "Content-Length: 4\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n",
        "Content-Type: application/vscode-jsonrpc; Charset=UTF8\r\ncontent-length: 4\r\n",
        "X-Extension: ignored\r\nCONTENT-LENGTH: 4\r\nContent-Type: application/vscode-jsonrpc; charset=\"UTF-8\"; ignored=yes\r\n",
        "Content-Length: 4\r\nContent-Type: application/vscode-jsonrpc\r\n",
    ] {
        let frames = format!("{headers}\r\n\"é\"Content-Length: 2\r\n\r\n{{}}");
        let mut reader = frames.as_bytes();
        assert_eq!(
            read_lsp_frame(&mut reader).await.unwrap().as_deref(),
            Some("\"é\"")
        );
        assert_eq!(
            read_lsp_frame(&mut reader).await.unwrap().as_deref(),
            Some("{}")
        );
        assert!(read_lsp_frame(&mut reader).await.unwrap().is_none());
    }
}

#[tokio::test]
async fn partial_pipe_reads_and_lf_headers_are_supported() {
    let (mut writer, reader) = tokio::io::duplex(2);
    let sender = tokio::spawn(async move {
        for byte in b"Content-Length: 2\n\n{}" {
            writer.write_all(&[*byte]).await.unwrap();
            tokio::task::yield_now().await;
        }
    });
    let mut reader = BufReader::new(reader);
    assert_eq!(
        read_lsp_frame(&mut reader).await.unwrap().as_deref(),
        Some("{}")
    );
    sender.await.unwrap();
    assert!(read_lsp_frame(&mut reader).await.unwrap().is_none());
}

#[tokio::test]
async fn malformed_headers_and_bodies_refuse_instead_of_resynchronizing() {
    for bytes in [
        b"\r\n".as_slice(),
        b"X: y\r\n\r\n",
        b"bad header\r\n\r\n",
        b"Content-Length: \r\n\r\n",
        b"Content-Length: +2\r\n\r\n{}",
        b"Content-Length: -1\r\n\r\n",
        b"Content-Length: 0\r\n\r\n",
        b"Content-Length: 9999999999999999999999999999999\r\n\r\n",
        b"Content-Length: 268435457\r\n\r\n",
        b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
        b"Content-Length: 2\r\nContent-Type: a\r\nContent-Type: b\r\n\r\n{}",
        b"Content-Length: 2\r\nContent-Type: application/json; charset=utf-16\r\n\r\n{}",
        b"X: \xff\r\nContent-Length: 2\r\n\r\n{}",
        b"Content-Length: 1\r\n\r\n\xff",
    ] {
        let mut reader = bytes;
        assert_eq!(
            read_lsp_frame(&mut reader).await.unwrap_err().kind(),
            ErrorKind::InvalidData,
            "{bytes:?}"
        );
    }
    for bytes in [
        b"Content-Len".as_slice(),
        b"Content-Length: 2\r\n",
        b"Content-Length: 2\r\n\r\n{",
    ] {
        let mut reader = bytes;
        assert_eq!(
            read_lsp_frame(&mut reader).await.unwrap_err().kind(),
            ErrorKind::UnexpectedEof,
            "{bytes:?}"
        );
    }
}

#[tokio::test]
async fn size_limits_apply_while_reading_and_include_the_complete_header_block() {
    let (mut writer, reader) = tokio::io::duplex(128);
    writer.write_all(&[b'A'; 65]).await.unwrap();
    // Keep the pipe open: rejection cannot depend on a newline or EOF.
    let mut reader = BufReader::new(reader);
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        read_lsp_frame_with_limits(&mut reader, 64, 16),
    )
    .await
    .expect("bounded headers must fail without waiting for newline")
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    let mut tail = [0];
    reader.read_exact(&mut tail).await.unwrap();
    assert_eq!(tail, [b'A']);
    let frame = b"Content-Length: 2\r\n\r\n{}";
    let mut exact = frame.as_slice();
    assert_eq!(
        read_lsp_frame_with_limits(&mut exact, frame.len() - 2, 2)
            .await
            .unwrap()
            .as_deref(),
        Some("{}")
    );
    let mut too_short = frame.as_slice();
    assert_eq!(
        read_lsp_frame_with_limits(&mut too_short, frame.len() - 4, 2)
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidData
    );
    let mut too_long = frame.as_slice();
    assert_eq!(
        read_lsp_frame_with_limits(&mut too_long, 64, 1)
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidData
    );
}
