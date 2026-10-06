/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Lightweight HTTP scrape server for Prometheus.
//!
//! Listens on a configured address (e.g. `0.0.0.0:9401`) and serves gateway metrics
//! in Prometheus text format (version 0.0.4) on `GET /metrics` without external
//! web framework dependencies.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::Metrics;
use super::prometheus_format::format_prometheus_metrics;

/// Starts the Prometheus HTTP metrics server on `listen_addr`.
/// Runs indefinitely until the async task or runtime is cancelled.
pub async fn run_prometheus_server(
    listen_addr: SocketAddr,
    metrics: Arc<Metrics>,
    node: String,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(listen_addr).await?;
    let local_addr = listener.local_addr().unwrap_or(listen_addr);
    tracing::info!(
        addr = %local_addr,
        endpoint = %format!("http://{local_addr}/metrics"),
        "Prometheus scrape server listening"
    );

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(error = %e, "error accepting connection on prometheus listener");
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
        };

        let metrics_ref = Arc::clone(&metrics);
        let node_ref = node.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_http_client(stream, &metrics_ref, &node_ref).await {
                tracing::trace!(peer = %peer, error = %e, "http prometheus scrape connection ended");
            }
        });
    }
}

async fn handle_http_client(
    mut stream: TcpStream,
    metrics: &Metrics,
    node: &str,
) -> anyhow::Result<()> {
    let mut buf = [0u8; 4096];
    let mut read_bytes = 0;

    // Read until end of HTTP header (\r\n\r\n or \n\n) or buffer limit
    while read_bytes < buf.len() {
        let n = stream.read(&mut buf[read_bytes..]).await?;
        if n == 0 {
            return Ok(());
        }
        read_bytes += n;
        let slice = &buf[..read_bytes];
        if slice.windows(4).any(|w| w == b"\r\n\r\n") || slice.windows(2).any(|w| w == b"\n\n") {
            break;
        }
    }

    let request_str = std::str::from_utf8(&buf[..read_bytes]).unwrap_or("");
    let first_line = request_str.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    match (method, path) {
        ("GET" | "HEAD", "/metrics" | "/") => {
            let body = format_prometheus_metrics(metrics, Some(node));
            let header = format!(
                "HTTP/1.1 200 OK\r\n\
                Content-Type: text/plain; version=0.0.4; charset=utf-8\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                \r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await?;
            if method == "GET" {
                stream.write_all(body.as_bytes()).await?;
            }
        }
        ("GET" | "HEAD", "/health" | "/healthz") => {
            let body = "OK\n";
            let header = format!(
                "HTTP/1.1 200 OK\r\n\
                Content-Type: text/plain; charset=utf-8\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                \r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await?;
            if method == "GET" {
                stream.write_all(body.as_bytes()).await?;
            }
        }
        ("GET" | "HEAD", _) => {
            let not_found = "404 Not Found\n";
            let resp = format!(
                "HTTP/1.1 404 Not Found\r\n\
                Content-Type: text/plain; charset=utf-8\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                \r\n\
                {}",
                not_found.len(),
                not_found
            );
            stream.write_all(resp.as_bytes()).await?;
        }
        _ => {
            let not_allowed = "405 Method Not Allowed\n";
            let resp = format!(
                "HTTP/1.1 405 Method Not Allowed\r\n\
                Content-Type: text/plain; charset=utf-8\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                \r\n\
                {}",
                not_allowed.len(),
                not_allowed
            );
            stream.write_all(resp.as_bytes()).await?;
        }
    }

    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}
