/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::{MAX_FRAME_SIZE, ProdCodeCodec};
use crate::messages::{HandshakeRequest, WireMessage};
use bytes::{BufMut, BytesMut};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

#[test]
fn test_codec_roundtrip() {
    let mut codec = ProdCodeCodec::new();
    let mut buf = BytesMut::new();

    let original = WireMessage::HandshakeRequest(HandshakeRequest {
        protocol_version: 1,
        supported_versions: Some(vec![1]),
        capabilities: None,
        client_name: "test-client".to_string(),
        client_pid: 1234,
        auth_token: None,
        client_workspace_root: "/home/user/project".to_string(),
        preferred_engine: None,
        base_workspace_name: None,
        engine_subpath: None,
        client_agent: None,
        client_host: None,
        purpose: None,
        redirect_count: 0,
    });

    codec.encode(original.clone(), &mut buf).unwrap();
    assert!(buf.len() > 4);

    let decoded = codec
        .decode(&mut buf)
        .unwrap()
        .expect("should decode message");
    assert_eq!(decoded, original);
    assert_eq!(buf.len(), 0);
}

#[test]
fn test_codec_does_not_consume_next_frame_header_as_a_marker() {
    let mut codec = ProdCodeCodec::new();
    let mut buf = BytesMut::new();
    codec.encode(WireMessage::Ping, &mut buf).unwrap();
    codec.encode(WireMessage::Pong, &mut buf).unwrap();

    assert_eq!(codec.decode(&mut buf).unwrap(), Some(WireMessage::Ping));
    assert_eq!(codec.decode(&mut buf).unwrap(), Some(WireMessage::Pong));
    assert!(buf.is_empty());
}

#[test]
fn test_codec_encode_ref_roundtrip() {
    let codec = ProdCodeCodec::new();
    let mut buf = BytesMut::with_capacity(1024);
    let initial_cap = buf.capacity();

    let original = WireMessage::HandshakeRequest(HandshakeRequest {
        protocol_version: 1,
        supported_versions: Some(vec![1]),
        capabilities: None,
        client_name: "test-client".to_string(),
        client_pid: 1234,
        auth_token: None,
        client_workspace_root: "/home/user/project".to_string(),
        preferred_engine: None,
        base_workspace_name: None,
        engine_subpath: None,
        client_agent: None,
        client_host: None,
        purpose: None,
        redirect_count: 0,
    });

    codec.encode_ref(&original, &mut buf).unwrap();
    assert!(buf.len() > 4);
    assert_eq!(buf.capacity(), initial_cap);

    let mut dec_codec = ProdCodeCodec::new();
    let decoded = dec_codec
        .decode(&mut buf)
        .unwrap()
        .expect("should decode message");
    assert_eq!(decoded, original);
    assert_eq!(buf.len(), 0);
}

#[test]
fn test_incomplete_maximum_sized_frame_does_not_reserve_payload() {
    let mut codec = ProdCodeCodec::new();
    let mut buf = BytesMut::with_capacity(8);
    buf.put_u32(MAX_FRAME_SIZE as u32);
    let capacity_before_decode = buf.capacity();

    assert_eq!(codec.decode(&mut buf).unwrap(), None);
    assert_eq!(buf.capacity(), capacity_before_decode);
    assert_eq!(buf.len(), 4);
}

#[test]
fn test_shadow_codec_roundtrip() {
    use crate::messages::{
        FileDelta, ShadowHypothesis, ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse,
    };

    let mut codec = ProdCodeCodec::new();
    let mut buf = BytesMut::new();
    let req = WireMessage::ShadowRunRequest(ShadowRunRequest {
        client_workspace_root: "/Users/dev/repo".to_string(),
        base_workspace_name: Some("repo".to_string()),
        hypotheses: vec![ShadowHypothesis {
            name: "h1".to_string(),
            files: vec![
                FileDelta {
                    relative_path: "src/lib.rs".to_string(),
                    content: Some(b"pub fn x() {}".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: "old.rs".to_string(),
                    content: None,
                    is_executable: false,
                },
            ],
        }],
        command: vec!["cargo".to_string(), "test".to_string()],
        env: Vec::new(),
        timeout_secs: 0,
        subdir: None,
        parallel: 0,
        tail_bytes: 0,
        in_memory: false,
        client_agent: None,
        client_host: None,
    });
    codec.encode(req.clone(), &mut buf).unwrap();
    assert_eq!(codec.decode(&mut buf).unwrap().expect("decodes"), req);

    let resp = WireMessage::ShadowRunResponse(ShadowRunResponse {
        server_workspace_root: "/srv/ws/repo".to_string(),
        mode: "overlay".to_string(),
        results: vec![ShadowHypothesisResult {
            name: "h1".to_string(),
            exit_code: Some(0),
            duration_ms: 950,
            timed_out: false,
            error: None,
            output_tail: Some(b"test result: ok".to_vec()),
            output_len: 15,
        }],
        error: None,
    });
    codec.encode(resp.clone(), &mut buf).unwrap();
    assert_eq!(codec.decode(&mut buf).unwrap().expect("decodes"), resp);
}

#[test]
fn test_sync_codec_roundtrip() {
    use crate::messages::{FileDelta, SyncRequest, SyncResponse};

    let mut codec = ProdCodeCodec::new();
    let mut buf = BytesMut::new();

    let req = WireMessage::SyncRequest(SyncRequest {
        client_workspace_root: "/Users/dev/repo".to_string(),
        files: vec![
            FileDelta {
                relative_path: "src/main.rs".to_string(),
                content: Some(b"fn main() {}".to_vec()),
                is_executable: false,
            },
            FileDelta {
                relative_path: "old_file.rs".to_string(),
                content: None,
                is_executable: false,
            },
        ],
        clean_others: false,
        base_workspace_name: None,
    });

    codec.encode(req.clone(), &mut buf).unwrap();
    let decoded = codec
        .decode(&mut buf)
        .unwrap()
        .expect("should decode SyncRequest");
    assert_eq!(decoded, req);

    let resp = WireMessage::SyncResponse(SyncResponse {
        files_updated: 1,
        files_deleted: 1,
        bytes_transferred: 12,
        duration_ms: 15,
        server_workspace_root: "/srv/prod-code/workspaces/repo".to_string(),
        workspace_was_fresh: false,
        stale_paths: Vec::new(),
    });

    codec.encode(resp.clone(), &mut buf).unwrap();
    let decoded_resp = codec
        .decode(&mut buf)
        .unwrap()
        .expect("should decode SyncResponse");
    assert_eq!(decoded_resp, resp);
}

#[test]
fn test_codec_preserves_preexisting_bytes_and_appends() {
    let codec = ProdCodeCodec::new();
    let mut buf = BytesMut::new();
    buf.put_slice(b"preexisting-bytes");
    let initial_len = buf.len();

    let original = WireMessage::HandshakeRequest(HandshakeRequest {
        protocol_version: 1,
        supported_versions: Some(vec![1]),
        capabilities: None,
        client_name: "test-client".to_string(),
        client_pid: 1234,
        auth_token: None,
        client_workspace_root: "/home/user/project".to_string(),
        preferred_engine: None,
        base_workspace_name: None,
        engine_subpath: None,
        client_agent: None,
        client_host: None,
        purpose: None,
        redirect_count: 0,
    });

    codec.encode_ref(&original, &mut buf).unwrap();
    assert_eq!(&buf[..initial_len], b"preexisting-bytes");
    let mut frame_buf = buf.split_off(initial_len);
    let mut dec_codec = ProdCodeCodec::new();
    let decoded = dec_codec.decode(&mut frame_buf).unwrap().expect("decodes");
    assert_eq!(decoded, original);
}

#[test]
fn test_codec_oversized_frame_aborts_early_and_does_not_retain_capacity() {
    let codec = ProdCodeCodec::new();
    let mut buf = BytesMut::with_capacity(16);
    buf.put_slice(b"prefix8B");
    let initial_len = buf.len();
    let initial_cap = buf.capacity();
    assert_eq!(initial_len, 8);
    assert_eq!(initial_cap, 16);

    let original = WireMessage::HandshakeRequest(HandshakeRequest {
        protocol_version: 1,
        supported_versions: Some(vec![1]),
        capabilities: None,
        client_name: "test-client".to_string(),
        client_pid: 1234,
        auth_token: None,
        client_workspace_root: "/home/user/project".to_string(),
        preferred_engine: None,
        base_workspace_name: None,
        engine_subpath: None,
        client_agent: None,
        client_host: None,
        purpose: None,
        redirect_count: 0,
    });

    let err = codec
        .encode_ref_bounded(&original, &mut buf, 50)
        .expect_err("must fail early when frame exceeds 50 bytes");
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(
        err.to_string()
            .contains("Message size exceeds maximum allowed frame size")
    );

    assert_eq!(buf.len(), initial_len);
    assert_eq!(&buf[..initial_len], b"prefix8B");
    assert_eq!(buf.capacity(), initial_cap);
}

#[test]
fn test_codec_nul_marker_roundtrip() {
    let mut codec = ProdCodeCodec::new().with_nul_marker(true);
    assert!(codec.has_nul_marker());
    let mut buf = BytesMut::new();

    let original = WireMessage::HandshakeRequest(HandshakeRequest {
        protocol_version: 1,
        supported_versions: Some(vec![1]),
        capabilities: None,
        client_name: "test-client-nul".to_string(),
        client_pid: 5678,
        auth_token: None,
        client_workspace_root: "/home/user/project-nul".to_string(),
        preferred_engine: None,
        base_workspace_name: None,
        engine_subpath: None,
        client_agent: None,
        client_host: None,
        purpose: None,
        redirect_count: 0,
    });

    codec.encode(original.clone(), &mut buf).unwrap();
    assert_eq!(buf.last(), Some(&0));

    let decoded = codec
        .decode(&mut buf)
        .unwrap()
        .expect("should decode message with NUL marker");
    assert_eq!(decoded, original);
    assert_eq!(buf.len(), 0);
}

#[test]
fn test_codec_decodes_trailing_nul_without_flag() {
    let mut enc_codec = ProdCodeCodec::new().with_nul_marker(true);
    let mut dec_codec = ProdCodeCodec::new();
    assert!(!dec_codec.has_nul_marker());

    let mut buf = BytesMut::new();
    let original = WireMessage::HandshakeRequest(HandshakeRequest {
        protocol_version: 1,
        supported_versions: Some(vec![1]),
        capabilities: None,
        client_name: "compat-client".to_string(),
        client_pid: 9999,
        auth_token: None,
        client_workspace_root: "/home/user/compat".to_string(),
        preferred_engine: None,
        base_workspace_name: None,
        engine_subpath: None,
        client_agent: None,
        client_host: None,
        purpose: None,
        redirect_count: 0,
    });

    enc_codec.encode(original.clone(), &mut buf).unwrap();
    assert_eq!(buf.last(), Some(&0));

    let decoded = dec_codec
        .decode(&mut buf)
        .unwrap()
        .expect("decodes transparently");
    assert_eq!(decoded, original);
    assert_eq!(dec_codec.decode_eof(&mut buf).unwrap(), None);
    assert_eq!(buf.len(), 0);
}

#[test]
fn test_codec_streaming_single_nul_terminated_request_not_withheld() {
    let mut enc_codec = ProdCodeCodec::new().with_nul_marker(true);
    let mut dec_codec = ProdCodeCodec::new();

    let mut buf = BytesMut::new();
    enc_codec.encode(WireMessage::Ping, &mut buf).unwrap();
    assert_eq!(buf.last(), Some(&0));

    let msg = dec_codec
        .decode(&mut buf)
        .unwrap()
        .expect("must emit completed frame while stream is open");
    assert_eq!(msg, WireMessage::Ping);

    assert_eq!(dec_codec.decode(&mut buf).unwrap(), None);
    assert_eq!(dec_codec.decode_eof(&mut buf).unwrap(), None);
    assert!(buf.is_empty());
}
