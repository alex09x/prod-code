/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

fn message() -> WireMessage {
    WireMessage::HttpProbe {
        method: "GET".into(),
        path: format!("/{}", "x".repeat(700)),
    }
}

fn frame(nul: bool, embedded: bool) -> Vec<u8> {
    let mut payload = serde_json::to_vec(&message()).unwrap();
    if embedded {
        payload.push(0);
    }
    let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
    bytes.extend(payload);
    if nul {
        bytes.push(0);
    }
    bytes
}

fn assert_message(message: WireMessage) {
    assert_eq!(
        serde_json::to_value(message).unwrap(),
        serde_json::to_value(self::message()).unwrap()
    );
}

#[test]
fn large_frames_at_every_boundary_with_optional_markers() {
    for nul in [false, true] {
        let encoded = frame(nul, false);
        let joined = [encoded.clone(), encoded.clone()].concat();
        for split in 0..=joined.len() {
            let mut codec = ProdCodeCodec::new();
            let mut input = BytesMut::from(&joined[..split]);
            let mut count = 0;
            while let Some(msg) = codec.decode(&mut input).unwrap() {
                assert_message(msg);
                count += 1;
            }
            input.extend_from_slice(&joined[split..]);
            while let Some(msg) = codec.decode(&mut input).unwrap() {
                assert_message(msg);
                count += 1;
            }
            assert!(codec.decode_eof(&mut input).unwrap().is_none());
            assert_eq!(count, 2, "nul={nul} split={split}");
            assert!(input.is_empty());
        }
    }
}

#[test]
fn marker_enforcement_and_payload_errors() {
    for marker in [false, true] {
        let mut codec = ProdCodeCodec::new().with_nul_marker(marker);
        let mut input = BytesMut::from(frame(marker, true).as_slice());
        assert_message(codec.decode(&mut input).unwrap().unwrap());
        assert!(input.is_empty());
        let mut codec = ProdCodeCodec::new().with_nul_marker(marker);
        let mut invalid = BytesMut::from(&b"\0\0\0\x02{}"[..]);
        if marker {
            invalid.extend_from_slice(&[0]);
        }
        let err = codec.decode(&mut invalid).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            err.to_string(),
            "Failed to parse WireMessage: missing field `type` at line 1 column 2"
        );
    }
    let mut codec = ProdCodeCodec::new().with_nul_marker(true);
    let mut input = BytesMut::from(frame(false, false).as_slice());
    assert!(codec.decode(&mut input).unwrap().is_none());
    input.extend_from_slice(&[1]);
    assert_eq!(
        codec.decode(&mut input).unwrap_err().to_string(),
        "Missing expected NUL completion marker after frame"
    );
    for suffix in [vec![0xff; 5], vec![0, 0, 0, 0, 2, b'{', b'}', 0]] {
        let mut codec = ProdCodeCodec::new();
        let mut input = BytesMut::from(frame(false, false).as_slice());
        assert_message(codec.decode(&mut input).unwrap().unwrap());
        assert!(codec.decode(&mut input).unwrap().is_none());
        input.extend_from_slice(&suffix);
        assert_eq!(
            codec.decode(&mut input).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn http_methods_targets_and_partial_headers() {
    for request in [
        "GET /health HTTP/1.1\r\n\r\n",
        "CUSTOM /é HTTP/1.1\r\n\r\n",
        "OPTIONS * HTTP/1.1\n\n",
        "GET /legacy\r\n\r\n",
        "GET /legacy\n\n",
        "HTTP/1.1 200 OK\r\n\r\n",
    ] {
        let mut codec = ProdCodeCodec::new();
        let mut src = BytesMut::from(request.as_bytes());
        if request.contains('é') {
            assert_eq!(
                codec.decode(&mut src).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        } else {
            let result = codec.decode(&mut src).unwrap().unwrap();
            let parts: Vec<_> = request.split_whitespace().collect();
            assert_eq!(
                serde_json::to_value(result).unwrap(),
                serde_json::to_value(WireMessage::HttpProbe {
                    method: parts[0].into(),
                    path: parts[1].into()
                })
                .unwrap()
            );
            assert!(src.is_empty());
        }
    }
    for partial in [
        "HEAD",
        "HEAD ",
        "HEAD /",
        "HEAD / HTTP",
        "HEAD / H",
        "HEAD / HTTP/",
    ] {
        let mut codec = ProdCodeCodec::new();
        let mut src = BytesMut::from(partial.as_bytes());
        assert!(codec.decode(&mut src).unwrap().is_none(), "{partial}");
        assert_eq!(&src[..], partial.as_bytes());
    }
    let mut src = BytesMut::from(format!("GET / HTTP/1.1\r\nX: {}", "a".repeat(8192)).as_bytes());
    assert_eq!(
        ProdCodeCodec::new()
            .decode(&mut src)
            .unwrap_err()
            .to_string(),
        "HTTP headers exceeded maximum limit for probe"
    );
}

#[test]
fn eof_reports_remaining_bytes_and_consumes_lone_completion_marker() {
    for bytes in [vec![0], vec![0, 0, 0], vec![0, 0, 0, 3, b'{']] {
        let mut codec = ProdCodeCodec::new();
        let mut input = BytesMut::from(bytes.as_slice());
        let error = codec.decode_eof(&mut input).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(
            error.to_string(),
            format!("Unexpected EOF with {} unparsed bytes", bytes.len())
        );
        assert_eq!(input.as_ref(), bytes);
    }
    let mut codec = ProdCodeCodec::new();
    let mut input = BytesMut::from(frame(false, false).as_slice());
    assert_message(codec.decode_eof(&mut input).unwrap().unwrap());
    input.extend_from_slice(&[0]);
    assert!(codec.decode_eof(&mut input).unwrap().is_none());
    assert!(input.is_empty());
    assert!(!codec.pending_trailing_nul);
}
