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
use crate::messages::WireMessage;
use bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};

#[test]
fn test_codec_http_probe_and_response() {
    let mut codec = ProdCodeCodec::new();
    let mut buf = BytesMut::new();

    // 1. Incomplete HTTP request waits for more data
    buf.extend_from_slice(b"GET /health HTTP/1.1\r\nHost: localhost");
    assert_eq!(codec.decode(&mut buf).unwrap(), None);

    // 2. Complete HTTP request with \r\n\r\n
    buf.extend_from_slice(b"\r\n\r\n");
    let decoded = codec
        .decode(&mut buf)
        .unwrap()
        .expect("should decode HTTP probe");
    assert_eq!(
        decoded,
        WireMessage::HttpProbe {
            method: "GET".to_string(),
            path: "/health".to_string()
        }
    );
    assert!(buf.is_empty());

    // 3. CONNECT and TRACE methods (regression tests for generic HTTP request line parsing)
    buf.extend_from_slice(b"CONNECT host.internal:443 HTTP/1.1\r\nHost: host.internal\r\n\r\n");
    let decoded_connect = codec
        .decode(&mut buf)
        .unwrap()
        .expect("should decode CONNECT probe");
    assert_eq!(
        decoded_connect,
        WireMessage::HttpProbe {
            method: "CONNECT".to_string(),
            path: "host.internal:443".to_string(),
        }
    );

    buf.extend_from_slice(b"TRACE /debug HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let decoded_trace = codec
        .decode(&mut buf)
        .unwrap()
        .expect("should decode TRACE probe");
    assert_eq!(
        decoded_trace,
        WireMessage::HttpProbe {
            method: "TRACE".to_string(),
            path: "/debug".to_string(),
        }
    );

    buf.extend_from_slice(
        b"M-SEARCH * HTTP/1.1\r\nHost: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\n\r\n",
    );
    let decoded_msearch = codec
        .decode(&mut buf)
        .unwrap()
        .expect("should decode M-SEARCH probe");
    assert_eq!(
        decoded_msearch,
        WireMessage::HttpProbe {
            method: "M-SEARCH".to_string(),
            path: "*".to_string(),
        }
    );

    buf.extend_from_slice(b"CONNECT [2001:db8::1]:443 HTTP/1.1\r\nHost: [2001:db8::1]:443\r\n\r\n");
    let decoded_ipv6_connect = codec
        .decode(&mut buf)
        .unwrap()
        .expect("should decode CONNECT probe with IPv6 literal authority");
    assert_eq!(
        decoded_ipv6_connect,
        WireMessage::HttpProbe {
            method: "CONNECT".to_string(),
            path: "[2001:db8::1]:443".to_string(),
        }
    );

    // 4. HttpResponse encoding
    let resp = WireMessage::HttpResponse {
        status: 200,
        content_type: "application/json".to_string(),
        body: "{\"status\":\"ok\"}".to_string(),
    };
    codec.encode(resp, &mut buf).unwrap();
    let resp_str = String::from_utf8(buf.to_vec()).unwrap();
    assert!(resp_str.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(resp_str.contains("Content-Type: application/json\r\n"));
    assert!(resp_str.contains("Content-Length: 15\r\n"));
    assert!(resp_str.ends_with("\r\n\r\n{\"status\":\"ok\"}"));
}
