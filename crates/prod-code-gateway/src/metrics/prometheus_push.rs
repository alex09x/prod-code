/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Periodic Prometheus Pushgateway client.
//!
//! Formats gateway telemetry in standard Prometheus text format and pushes it
//! via HTTP to a configured Prometheus Pushgateway endpoint at regular intervals.

use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;

use super::Metrics;
use super::prometheus_format::format_prometheus_metrics;

/// Runs the periodic Prometheus Pushgateway pusher loop.
pub async fn run_prometheus_push_loop(
    push_base_url: String,
    job: String,
    instance: String,
    interval: Duration,
    metrics: Arc<Metrics>,
) {
    tracing::info!(
        url = %push_base_url,
        job = %job,
        instance = %instance,
        interval_secs = interval.as_secs(),
        "Prometheus Pushgateway client loop starting"
    );

    let mut ticker = tokio::time::interval(interval);
    // Skip the first immediate tick so startup metrics have time to populate
    ticker.tick().await;

    loop {
        ticker.tick().await;

        let body = format_prometheus_metrics(&metrics, Some(&instance));
        if let Err(e) = push_metrics_to_gateway(&push_base_url, &job, &instance, &body).await {
            tracing::warn!(
                error = %e,
                url = %push_base_url,
                "failed to push metrics to Prometheus Pushgateway"
            );
        } else {
            tracing::debug!(
                bytes = body.len(),
                url = %push_base_url,
                "successfully pushed metrics to Prometheus Pushgateway"
            );
        }
    }
}

/// Constructs the Pushgateway endpoint URL:
/// `{base}/metrics/job/{job}/instance/{instance}`
pub fn build_pushgateway_url(base: &str, job: &str, instance: &str) -> anyhow::Result<Url> {
    let mut url = Url::parse(base)?;
    let mut path = url.path().trim_end_matches('/').to_string();
    if !path.ends_with("/metrics") {
        path.push_str("/metrics");
    }
    path.push_str(&format!(
        "/job/{}/instance/{}",
        urlencoding_simple(job),
        urlencoding_simple(instance)
    ));
    url.set_path(&path);
    Ok(url)
}

fn urlencoding_simple(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

pub(crate) async fn push_metrics_to_gateway(
    base: &str,
    job: &str,
    instance: &str,
    metrics_body: &str,
) -> anyhow::Result<()> {
    let url = build_pushgateway_url(base, job, instance)?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("missing host in push URL: {url}"))?;
    let port = url.port_or_known_default().unwrap_or(9091);
    let path = url.path();

    let host_header = if url.port().is_some() {
        format!("{host}:{port}")
    } else {
        host.to_string()
    };

    let request = format!(
        "POST {path} HTTP/1.1\r\n\
        Host: {host_header}\r\n\
        User-Agent: prod-code-gateway\r\n\
        Content-Type: text/plain; version=0.0.4\r\n\
        Content-Length: {}\r\n\
        Connection: close\r\n\
        \r\n\
        {}",
        metrics_body.len(),
        metrics_body
    );

    let mut stream = TcpStream::connect((host, port)).await?;
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let mut resp_buf = [0u8; 1024];
    let n = stream.read(&mut resp_buf).await?;
    let resp_str = std::str::from_utf8(&resp_buf[..n]).unwrap_or("");
    let status_line = resp_str.lines().next().unwrap_or("");

    // HTTP/1.1 200 OK or HTTP/1.1 202 Accepted
    if status_line.contains(" 200 ")
        || status_line.contains(" 202 ")
        || status_line.contains(" 204 ")
    {
        Ok(())
    } else {
        anyhow::bail!("Pushgateway error response: {status_line}");
    }
}
