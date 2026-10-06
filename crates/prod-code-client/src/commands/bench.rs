/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::workspace::detect_workspace_name;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use prod_code_client::divergent_bench::{self, DivergentBenchConfig};
use prod_code_protocol::{
    HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage, supported_protocol_versions,
    validate_selected_protocol_version,
};
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio_util::codec::Framed;

/// Run concurrent pipelined benchmark against remote gateway across workspaces and worktrees.
pub async fn run_benchmark(
    remote: SocketAddr,
    workspaces: Vec<PathBuf>,
    concurrency: usize,
    depth: usize,
    duration_secs: u64,
) -> Result<()> {
    let target_workspaces: Vec<PathBuf> = if workspaces.is_empty() {
        vec![env::current_dir()?]
    } else {
        workspaces
    };

    println!("⚡ prod-code Multi-Tenant Benchmark (Pipelined Concurrent Load)");
    println!("────────────────────────────────────────────────────────────────");
    println!("Target Remote:       {remote}");
    println!("Concurrency:         {concurrency} worker connections");
    println!("Pipeline Depth:      {depth} in-flight queries per worker");
    println!("Duration:            {duration_secs}s");
    println!("Target Workspaces:   {} total", target_workspaces.len());
    for (i, ws) in target_workspaces.iter().enumerate() {
        let name = detect_workspace_name(ws).unwrap_or_else(|| "default".to_string());
        println!("  • [{}] {} (base: {})", i + 1, ws.display(), name);
    }
    println!("────────────────────────────────────────────────────────────────");
    println!("Connecting workers and starting load test...");

    let start_instant = std::time::Instant::now();
    let end_deadline = start_instant + std::time::Duration::from_secs(duration_secs);

    let mut handles = Vec::new();

    for worker_id in 0..concurrency {
        let ws_path = target_workspaces[worker_id % target_workspaces.len()].clone();
        let handle = tokio::spawn(async move {
            let mut latencies_us = Vec::new();
            let mut completed: u64 = 0;
            let mut errors: u64 = 0;

            let ws_str = ws_path.to_string_lossy().to_string();
            let base_name = detect_workspace_name(&ws_path);

            let (file_path, line, col) = match crate::workspace::find_first_code_file(&ws_path) {
                Some((f, l, c)) => (f, l, c),
                None => {
                    return (0, 1, Vec::new());
                }
            };

            let Ok(stream) = prod_code_protocol::transport::connect(remote).await else {
                return (0, 1, Vec::new());
            };
            let mut framed = Framed::new(stream, ProdCodeCodec::new());

            let (engine_subpath, expected_engine) =
                prod_code_mcp::sync::engine_project(&ws_path, &file_path);
            let supported_versions = supported_protocol_versions();
            let handshake_req = WireMessage::HandshakeRequest(HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                supported_versions: Some(supported_versions.clone()),
                capabilities: None,
                client_name: format!("bench-worker-{worker_id}"),
                client_pid: std::process::id(),
                auth_token: None,
                client_workspace_root: ws_str.clone(),
                preferred_engine: engine_subpath
                    .as_ref()
                    .and(expected_engine)
                    .map(str::to_string),
                base_workspace_name: base_name,
                engine_subpath,
                client_agent: Some(prod_code_protocol::detect_client_agent()),
                client_host: Some(prod_code_protocol::client_host()),
                purpose: None,
                redirect_count: 0,
            });

            if framed.send(handshake_req).await.is_err() {
                return (0, 1, Vec::new());
            }

            match framed.next().await {
                Some(Ok(WireMessage::HandshakeResponse(response)))
                    if validate_selected_protocol_version(
                        response.protocol_version,
                        &supported_versions,
                    )
                    .is_ok() => {}
                _ => return (0, 1, Vec::new()),
            }

            let init_req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": format!("file://{ws_str}"),
                    "capabilities": {}
                }
            });
            if framed
                .send(WireMessage::LspPayload(init_req.to_string()))
                .await
                .is_err()
            {
                return (0, 1, Vec::new());
            }

            loop {
                match framed.next().await {
                    Some(Ok(WireMessage::LspPayload(resp))) => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp) {
                            if v.get("method").is_none()
                                && v.get("id").and_then(|i| i.as_u64()) == Some(1)
                            {
                                break;
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                    _ => return (0, 1, Vec::new()),
                }
            }

            let file_uri = match url::Url::from_file_path(&file_path) {
                Ok(u) => u.to_string(),
                Err(_) => return (0, 1, Vec::new()),
            };

            let mut req_counter: u64 = 2;
            let mut in_flight: std::collections::HashMap<u64, std::time::Instant> =
                std::collections::HashMap::new();

            while std::time::Instant::now() < end_deadline {
                while in_flight.len() < depth && std::time::Instant::now() < end_deadline {
                    let id = req_counter;
                    req_counter += 1;
                    let hover_req = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": "textDocument/hover",
                        "params": {
                            "textDocument": { "uri": file_uri },
                            "position": { "line": line, "character": col }
                        }
                    });

                    let send_time = std::time::Instant::now();
                    if framed
                        .send(WireMessage::LspPayload(hover_req.to_string()))
                        .await
                        .is_ok()
                    {
                        in_flight.insert(id, send_time);
                    } else {
                        errors += 1;
                        break;
                    }
                }

                tokio::select! {
                    msg_opt = framed.next() => {
                        match msg_opt {
                            Some(Ok(WireMessage::LspPayload(payload))) => {
                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&payload) {
                                    let maybe_id = val
                                        .get("id")
                                        .filter(|_| val.get("method").is_none())
                                        .and_then(|v| v.as_u64());
                                    if let Some(send_time) = maybe_id.and_then(|id| in_flight.remove(&id)) {
                                        let elapsed = send_time.elapsed().as_micros() as u64;
                                        latencies_us.push(elapsed);
                                        completed += 1;
                                    }
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) | None => {
                                errors += 1;
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
                }
            }

            let _ = framed
                .send(WireMessage::Disconnect {
                    reason: "bench finished".to_string(),
                })
                .await;
            (completed, errors, latencies_us)
        });
        handles.push(handle);
    }

    let mut total_completed = 0;
    let mut total_errors = 0;
    let mut all_latencies = Vec::new();

    for h in handles {
        if let Ok((comp, errs, mut lats)) = h.await {
            total_completed += comp;
            total_errors += errs;
            all_latencies.append(&mut lats);
        }
    }

    let elapsed = start_instant.elapsed().as_secs_f64();
    let qps = if elapsed > 0.0 {
        (total_completed as f64) / elapsed
    } else {
        0.0
    };

    all_latencies.sort_unstable();
    let p50 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 50 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let p90 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 90 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let p95 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 95 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let p99 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 99 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let min = if !all_latencies.is_empty() {
        all_latencies[0] as f64 / 1000.0
    } else {
        0.0
    };

    println!("\n📊 Benchmark Results:");
    println!("────────────────────────────────────────────────────────────────");
    println!("Elapsed Time:        {:.2}s", elapsed);
    println!("Completed Queries:   {}", total_completed);
    println!("Errors:              {}", total_errors);
    println!("Throughput:          \x1b[1;32m{:.1} QPS\x1b[0m", qps);
    println!("Latency (min):       {:.2} ms", min);
    println!("Latency (p50):       {:.2} ms", p50);
    println!("Latency (p90):       {:.2} ms", p90);
    println!("Latency (p95):       {:.2} ms", p95);
    println!("Latency (p99):       {:.2} ms", p99);
    println!("────────────────────────────────────────────────────────────────");

    Ok(())
}

/// Run the multi-worktree divergence and correctness benchmark against the remote gateway.
pub async fn run_divergent_bench(config: DivergentBenchConfig) -> Result<()> {
    println!("⚡ prod-code Divergent Worktree Benchmark (Multi-Agent Fleet Simulation)");
    println!("────────────────────────────────────────────────────────────────");
    println!("Target Remote:       {}", config.remote);
    println!(
        "Base Repo:           {}",
        config
            .base_repo
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<scratch fixture repo>".to_string())
    );
    println!("Workspace Mode:      {}", config.mode.label());
    println!(
        "Session Model:       {}",
        if config.persistent {
            "persistent"
        } else {
            "connect per query"
        }
    );
    println!("Concurrent Workers:  {}", config.workers);
    println!("Queries Per Worker:  {}", config.queries_per_worker);
    println!("────────────────────────────────────────────────────────────────");
    println!("Forking git worktrees, applying controlled mutations, syncing to gateway...");

    let report = divergent_bench::run(config).await?;
    report.print();

    if !report.all_passed {
        anyhow::bail!("divergent worktree benchmark FAILED correctness verification");
    }

    Ok(())
}
