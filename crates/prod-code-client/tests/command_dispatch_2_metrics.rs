/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{MetricsResponse, ProdCodeCodec, WireMessage};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

fn response(full: bool) -> MetricsResponse {
    let mut value = json!({
        "node":"sample-node","since_secs":3660,"events_in_memory":8,
        "queries":[],"execs":[],"sync_rounds":2,"sync_files":3,"sync_bytes":1048576
    });
    if full {
        value["snapshots"] = json!([{
            "ts_ms":1,"node":"sample-node","version":"1","git_commit":"unknown",
            "platform":"sample","cpu_count":4,"cpu_usage_millis":12500,
            "process_rss_bytes":2097152,"storage_free_millis":725,
            "active_sessions":2,"active_queries":0,"running_commands":0,"workspace_count":3,"engine_count":4
        }]);
        value["inventory"] = json!({"ts_ms":1,"node":"sample-node","engines":[
            {"engine":"rust","available":true,"toolchains":[{"tool":"rustc","version":"rustc 1.97.1"}]},
            {"engine":"go","available":true,"toolchains":[]},
            {"engine":"python","available":true,"toolchains":[{"tool":"python","version":"preview"}]},
            {"engine":"swift","available":false,"toolchains":[]}
        ]});
    } else {
        value["snapshots"] = json!([{
            "ts_ms":1,"node":"sample-node","version":"1","git_commit":"unknown",
            "platform":"sample","cpu_count":4,
            "active_sessions":0,"active_queries":0,"running_commands":0,"workspace_count":0,"engine_count":0
        }]);
        value["inventory"] = json!({"ts_ms":1,"node":"sample-node","engines":[]});
    }
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn metrics_snapshot_inventory_and_json_table() {
    let ws = prod_code_testkit::Workspace::empty();
    for (full, as_json) in [(true, false), (false, false), (true, true)] {
        let response = response(full);
        let expected_json = serde_json::to_value(vec![response.clone()]).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut framed = Framed::new(socket, ProdCodeCodec::new());
            match framed.next().await.unwrap().unwrap() {
                WireMessage::MetricsRequest(request) => assert_eq!(request.since_secs, 3660),
                other => panic!("unexpected request: {other:?}"),
            }
            framed
                .send(WireMessage::MetricsResponse(response))
                .await
                .unwrap();
        });
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"));
        command
            .arg("-r")
            .arg(address.to_string())
            .args(["metrics", "--since", "3660"])
            .env("HOME", ws.root())
            .current_dir(ws.root());
        if as_json {
            command.arg("--json");
        }
        let output = command.output().await.unwrap();
        task.await.unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        if as_json {
            assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), expected_json);
        } else {
            let separator =
                "────────────────────────────────────────────────────────────────────────";
            let snapshot = if full {
                "  snapshot: CPU 12.5% | RSS 2 MB | Disk 72% free | Sessions 2 | Workspaces 3 | Engines 4\n  toolchains: rust:1.97.1, go, python:preview\n"
            } else {
                "  snapshot: CPU - | RSS - | Disk - | Sessions 0 | Workspaces 0 | Engines 0\n"
            };
            let expected = format!(
                "⚡ prod-code usage (last 1h01m, 1 node(s))\n{separator}\nno queries in the window\n{separator}\n{:<22} syncs {:>5}  files {:>7}  {:>8.1} MB  events in memory {}\n{snapshot}",
                "sample-node", 2, 3, 1.0, 8
            );
            assert_eq!(text, expected);
        }
    }
}
