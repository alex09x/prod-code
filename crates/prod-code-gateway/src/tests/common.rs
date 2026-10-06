/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::*;

pub(crate) fn peer(addr: &str, platform: &str, engines: &[&str], load_per_cpu: f64) -> PeerInfo {
    let cpus = 8usize;
    PeerInfo {
        addr: addr.to_string(),
        status: StatusResponse {
            server_pid: 1,
            uptime_seconds: 1,
            active_sessions: 0,
            loaded_workspaces: 0,
            detected_engines: engines.iter().map(|e| e.to_string()).collect(),
            memory_rss_bytes: None,
            total_queries: 0,
            active_queries: 0,
            load_average_millis: Some((load_per_cpu * cpus as f64 * 1000.0) as u32),
            cpu_count: Some(cpus),
            platform: Some(platform.to_string()),
            running_commands: Vec::new(),
            host: Default::default(),
            version: None,
            git_commit: None,
        },
        workspaces: Vec::new(),
        last_seen_secs: 0,
        alive: true,
    }
}
