/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::ProdCodeCodec;
use crate::messages::{HandshakeRequest, WireMessage};
use bytes::{BufMut, BytesMut};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

#[test]
fn test_codec_markerless_two_frames_split_after_first_byte_of_second_header() {
    let mut enc_codec = ProdCodeCodec::new();
    let mut dec_codec = ProdCodeCodec::new();

    let mut full_wire = BytesMut::new();
    enc_codec.encode(WireMessage::Ping, &mut full_wire).unwrap();
    enc_codec.encode(WireMessage::Pong, &mut full_wire).unwrap();

    let ping_len =
        u32::from_be_bytes([full_wire[0], full_wire[1], full_wire[2], full_wire[3]]) as usize;
    let split_idx = 4 + ping_len + 1;
    assert_eq!(
        full_wire[split_idx - 1],
        0,
        "first byte of second header must be 0x00"
    );

    let mut incoming = BytesMut::new();
    incoming.put_slice(&full_wire[..split_idx]);

    let msg1 = dec_codec
        .decode(&mut incoming)
        .unwrap()
        .expect("first frame decodes immediately");
    assert_eq!(msg1, WireMessage::Ping);
    assert!(
        !dec_codec.has_nul_marker(),
        "must not latch NUL marker on ambiguous single byte"
    );

    assert_eq!(dec_codec.decode(&mut incoming).unwrap(), None);

    incoming.put_slice(&full_wire[split_idx..]);

    let msg2 = dec_codec
        .decode(&mut incoming)
        .unwrap()
        .expect("second frame decodes");
    assert_eq!(msg2, WireMessage::Pong);
    assert!(!dec_codec.has_nul_marker(), "must remain markerless");
    assert!(incoming.is_empty());
}

#[test]
fn test_codec_decodes_multiple_trailing_nul_without_flag() {
    let mut enc_codec = ProdCodeCodec::new().with_nul_marker(true);
    let mut dec_codec = ProdCodeCodec::new();

    let mut buf = BytesMut::new();
    enc_codec.encode(WireMessage::Ping, &mut buf).unwrap();
    enc_codec.encode(WireMessage::Pong, &mut buf).unwrap();

    assert_eq!(dec_codec.decode(&mut buf).unwrap(), Some(WireMessage::Ping));
    assert_eq!(dec_codec.decode(&mut buf).unwrap(), Some(WireMessage::Pong));
    assert!(buf.is_empty());
}

#[test]
fn test_codec_fragmented_byte_by_byte_with_nul_marker() {
    let mut enc_codec = ProdCodeCodec::new().with_nul_marker(true);
    let mut dec_codec = ProdCodeCodec::new();

    let mut full_wire = BytesMut::new();
    enc_codec.encode(WireMessage::Ping, &mut full_wire).unwrap();
    enc_codec.encode(WireMessage::Pong, &mut full_wire).unwrap();

    let mut incoming = BytesMut::new();
    let mut decoded_messages = Vec::new();

    for byte in full_wire {
        incoming.put_u8(byte);
        while let Some(msg) = dec_codec.decode(&mut incoming).unwrap() {
            decoded_messages.push(msg);
        }
    }

    assert_eq!(decoded_messages, vec![WireMessage::Ping, WireMessage::Pong]);
    assert!(incoming.is_empty());
}

#[test]
fn test_codec_fragmented_splits_at_every_byte_boundary() {
    let mut enc_codec = ProdCodeCodec::new().with_nul_marker(true);
    let mut full_wire = BytesMut::new();
    enc_codec.encode(WireMessage::Ping, &mut full_wire).unwrap();
    enc_codec.encode(WireMessage::Pong, &mut full_wire).unwrap();

    for split in 1..full_wire.len() {
        let mut dec_codec = ProdCodeCodec::new();
        let mut incoming = BytesMut::new();
        let mut decoded = Vec::new();

        incoming.put_slice(&full_wire[..split]);
        while let Some(msg) = dec_codec.decode(&mut incoming).unwrap() {
            decoded.push(msg);
        }

        incoming.put_slice(&full_wire[split..]);
        while let Some(msg) = dec_codec.decode(&mut incoming).unwrap() {
            decoded.push(msg);
        }

        assert_eq!(
            decoded,
            vec![WireMessage::Ping, WireMessage::Pong],
            "failed decoding with split at index {split}"
        );
        assert!(
            incoming.is_empty(),
            "buffer not empty with split at index {split}"
        );
    }
}

#[test]
fn test_codec_handles_whitespace_prefixed_json() {
    let mut dec_codec = ProdCodeCodec::new();
    let mut buf = BytesMut::new();

    let json_ping = b"   {\"type\":\"Ping\"}";
    buf.put_u32(json_ping.len() as u32);
    buf.put_slice(json_ping);

    let json_pong = b"\n\t{\"type\":\"Pong\"}";
    buf.put_u32(json_pong.len() as u32);
    buf.put_slice(json_pong);

    assert_eq!(dec_codec.decode(&mut buf).unwrap(), Some(WireMessage::Ping));
    assert_eq!(dec_codec.decode(&mut buf).unwrap(), Some(WireMessage::Pong));
    assert!(buf.is_empty());
}

#[test]
fn test_codec_decodes_embedded_nul_in_frame_len() {
    let mut dec_codec = ProdCodeCodec::new();
    let mut buf = BytesMut::new();

    let original = WireMessage::HandshakeRequest(HandshakeRequest {
        protocol_version: 1,
        supported_versions: Some(vec![1]),
        capabilities: None,
        client_name: "embedded-nul".to_string(),
        client_pid: 1,
        auth_token: None,
        client_workspace_root: "/p".to_string(),
        preferred_engine: None,
        base_workspace_name: None,
        engine_subpath: None,
        client_agent: None,
        client_host: None,
        purpose: None,
        redirect_count: 0,
    });

    let mut json_with_nul = serde_json::to_vec(&original).unwrap();
    json_with_nul.push(0);
    buf.put_u32(json_with_nul.len() as u32);
    buf.put_slice(&json_with_nul);

    let decoded = dec_codec
        .decode(&mut buf)
        .unwrap()
        .expect("decodes frame with internal NUL");
    assert_eq!(decoded, original);
    assert_eq!(buf.len(), 0);
}

#[test]
fn test_codec_nul_marker_fails_when_nul_missing() {
    let mut enc_codec = ProdCodeCodec::new();
    let mut dec_codec = ProdCodeCodec::new().with_nul_marker(true);

    let mut buf = BytesMut::new();
    let original = WireMessage::HandshakeRequest(HandshakeRequest {
        protocol_version: 1,
        supported_versions: Some(vec![1]),
        capabilities: None,
        client_name: "missing-nul-client".to_string(),
        client_pid: 4321,
        auth_token: None,
        client_workspace_root: "/home/user/missing".to_string(),
        preferred_engine: None,
        base_workspace_name: None,
        engine_subpath: None,
        client_agent: None,
        client_host: None,
        purpose: None,
        redirect_count: 0,
    });

    enc_codec.encode(original, &mut buf).unwrap();
    assert_eq!(dec_codec.decode(&mut buf).unwrap(), None);

    buf.put_u8(b'X');
    let err = dec_codec.decode(&mut buf).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    assert!(
        err.to_string()
            .contains("Missing expected NUL completion marker")
    );
}
