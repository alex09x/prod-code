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

#[tokio::test]
async fn head_requests_invalid_utf8_and_empty_connections_have_exact_responses() {
    let temp = tempfile::tempdir().unwrap();
    let metrics = Metrics::new(temp.path().join("metrics"));
    let body = format_prometheus_metrics(&metrics, Some("sample"));
    for (request, expected) in [
        (b"HEAD /metrics HTTP/1.1\r\n\r\n".as_slice(), format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())),
        (b"HEAD /healthz HTTP/1.1\n\n", "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 3\r\nConnection: close\r\n\r\n".into()),
        (b"HEAD /unknown HTTP/1.1\r\n\r\n", "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 14\r\nConnection: close\r\n\r\n".into()),
        (b"POST /metrics HTTP/1.1\r\n\r\n", "HTTP/1.1 405 Method Not Allowed\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 23\r\nConnection: close\r\n\r\n405 Method Not Allowed\n".into()),
        (b"\xff\r\n\r\n", "HTTP/1.1 405 Method Not Allowed\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 23\r\nConnection: close\r\n\r\n405 Method Not Allowed\n".into()),
        (b"", String::new()),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let serve = handle_http_client(stream, &metrics, "sample");
        let read = async {
            client.write_all(request).await.unwrap();
            client.shutdown().await.unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).await.unwrap();
            response
        };
        let (served, response) = tokio::join!(serve, read);
        served.unwrap();
        assert_eq!(response, expected);
    }
}
