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
async fn scrape_listener_serves_get_bodies_and_keeps_accepting_after_empty_clients() {
    let temp = tempfile::tempdir().unwrap();
    let metrics = Arc::new(Metrics::new(temp.path().join("metrics")));
    let body = format_prometheus_metrics(&metrics, Some("scrape-node"));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(serve_prometheus(listener, metrics, "scrape-node".into()));
    let mut empty = TcpStream::connect(addr).await.unwrap();
    empty.shutdown().await.unwrap();
    let mut eof = Vec::new();
    empty.read_to_end(&mut eof).await.unwrap();
    assert_eq!(eof, b"");
    for (path, status, content_type, expected_body) in [
        (
            "/metrics",
            "200 OK",
            "text/plain; version=0.0.4; charset=utf-8",
            body.as_str(),
        ),
        (
            "/",
            "200 OK",
            "text/plain; version=0.0.4; charset=utf-8",
            body.as_str(),
        ),
        ("/health", "200 OK", "text/plain; charset=utf-8", "OK\n"),
        ("/healthz", "200 OK", "text/plain; charset=utf-8", "OK\n"),
        (
            "/missing",
            "404 Not Found",
            "text/plain; charset=utf-8",
            "404 Not Found\n",
        ),
    ] {
        let mut client = TcpStream::connect(addr).await.unwrap();
        // Two writes exercise a request whose header arrives in fragments.
        client
            .write_all(format!("GET {path} HTTP/1.1\r\n").as_bytes())
            .await
            .unwrap();
        client.write_all(b"Host: example\r\n\r\n").await.unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        assert_eq!(
            response,
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{expected_body}",
                expected_body.len()
            )
        );
    }
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn scrape_startup_preserves_an_occupied_listener() {
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let error = run_prometheus_server(
        listener.local_addr().unwrap(),
        Arc::new(Metrics::new(temp.path().join("metrics"))),
        "sample".into(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::AddrInUse
    );
}

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
