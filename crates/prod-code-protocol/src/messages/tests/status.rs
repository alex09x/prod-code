/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::status::{HostResources, RunningCommand, StatusResponse};

/// An older gateway sends no running commands, and a status lists the ones it has longest
/// first, with a long command line cut (#273).
#[test]
fn a_status_lists_its_running_commands_longest_first() {
    let old = r#"{"server_pid":1,"uptime_seconds":2,"active_sessions":0,"loaded_workspaces":0,"detected_engines":[]}"#;
    let status: StatusResponse = serde_json::from_str(old).expect("an older gateway's status");
    assert!(status.running_commands.is_empty());
    assert!(status.running_lines().is_empty());

    let status = StatusResponse {
        running_commands: vec![
            RunningCommand {
                workspace: "shop".to_string(),
                command: "cargo check".to_string(),
                running_seconds: 5,
            },
            RunningCommand {
                workspace: "shop--wt-1a2b".to_string(),
                command: format!("cargo test {}", "x".repeat(120)),
                running_seconds: 725,
            },
        ],
        ..status
    };
    let lines = status.running_lines();
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].starts_with("shop--wt-1a2b  12m 5s  cargo test xx"),
        "{}",
        lines[0]
    );
    assert!(lines[0].ends_with('…'), "{}", lines[0]);
    assert_eq!(lines[1], "shop  0m 5s  cargo check");
}

/// A host past 85% of its memory or under 10% of its disk is under pressure, and says which;
/// one that reports neither, as an older gateway does, is not (#396).
#[test]
fn a_host_short_of_memory_or_disk_says_so() {
    let old = r#"{"server_pid":1,"uptime_seconds":2,"active_sessions":0,"loaded_workspaces":0,"detected_engines":[]}"#;
    let status: StatusResponse = serde_json::from_str(old).expect("an older gateway's status");
    assert_eq!(status.host, HostResources::default());
    assert_eq!(status.host.pressure(), None);
    assert_eq!(status.host.describe(), "");

    let gib = 1 << 30;
    let roomy = HostResources {
        memory_available_bytes: Some(20 * gib),
        memory_total_bytes: Some(100 * gib),
        storage_free_millis: Some(620),
    };
    assert_eq!(roomy.pressure(), None);
    assert_eq!(roomy.describe(), "memory 80% used, disk 62% free");

    let short_of_memory = HostResources {
        memory_available_bytes: Some(9 * gib),
        ..roomy.clone()
    };
    assert_eq!(
        short_of_memory.pressure().as_deref(),
        Some("memory 91% used")
    );

    let short_of_both = HostResources {
        storage_free_millis: Some(40),
        ..short_of_memory
    };
    assert_eq!(
        short_of_both.pressure().as_deref(),
        Some("memory 91% used, disk 4% free")
    );

    let disk_only = HostResources {
        storage_free_millis: Some(99),
        ..HostResources::default()
    };
    assert_eq!(disk_only.pressure().as_deref(), Some("disk 9% free"));
    assert_eq!(disk_only.describe(), "disk 9% free");
}

#[test]
fn congestion_score_reflects_load_memory_disk_and_density() {
    let gib = 1 << 30;

    // 1. Idle node: plenty of resources, 0 load, 0 workspaces
    let idle = StatusResponse {
        server_pid: 1,
        uptime_seconds: 100,
        active_sessions: 0,
        loaded_workspaces: 0,
        detected_engines: vec!["rust".into()],
        memory_rss_bytes: None,
        total_queries: 0,
        active_queries: 0,
        load_average_millis: Some(20), // 0.02
        cpu_count: Some(1),
        platform: Some("linux x86_64".into()),
        running_commands: Vec::new(),
        host: HostResources {
            memory_available_bytes: Some(16 * gib),
            memory_total_bytes: Some(32 * gib), // 50% used
            storage_free_millis: Some(650),     // 65% free
        },
        version: None,
        git_commit: None,
    };
    assert!((idle.congestion_score() - 0.02).abs() < 1e-6);

    // 2. Memory penalty above 70%
    let mem_heavy = StatusResponse {
        host: HostResources {
            memory_available_bytes: Some(6 * gib),
            memory_total_bytes: Some(30 * gib), // 80% used (0.10 above 0.70 => +0.80)
            storage_free_millis: Some(500),
        },
        ..idle.clone()
    };
    // 0.02 + 0.10 * 8.0 = 0.82
    assert!((mem_heavy.congestion_score() - 0.82).abs() < 1e-4);

    // 3. Disk penalty below 25%
    let disk_heavy = StatusResponse {
        host: HostResources {
            memory_available_bytes: Some(16 * gib),
            memory_total_bytes: Some(32 * gib),
            storage_free_millis: Some(150), // 15% free (0.10 below 0.25 => +0.50)
        },
        ..idle.clone()
    };
    // 0.02 + 0.10 * 5.0 = 0.52
    assert!((disk_heavy.congestion_score() - 0.52).abs() < 1e-4);

    // 4. Density penalties (workspaces, sessions, running commands)
    let loaded = StatusResponse {
        loaded_workspaces: 5, // 5 * 0.08 = 0.40
        active_sessions: 4,   // 4 * 0.05 = 0.20
        running_commands: vec![
            RunningCommand {
                workspace: "w1".into(),
                command: "cargo check".into(),
                running_seconds: 10,
            },
            RunningCommand {
                workspace: "w2".into(),
                command: "cargo test".into(),
                running_seconds: 20,
            },
        ], // 2 * 0.35 = 0.70
        ..idle.clone()
    };
    // 0.02 + 0.40 + 0.20 + 0.70 = 1.32
    assert!((loaded.congestion_score() - 1.32).abs() < 1e-4);

    // 5. Hard pressure (e.g. storage < 10%)
    let pressured = StatusResponse {
        host: HostResources {
            storage_free_millis: Some(50), // 5% free => hard pressure
            ..idle.host.clone()
        },
        ..idle.clone()
    };
    assert!(pressured.congestion_score() >= 1000.0);

    // 6. Unknown telemetry penalized conservatively (never scored as 0.0 idle)
    let unmeasured = StatusResponse {
        load_average_millis: None,
        cpu_count: None,
        host: HostResources::default(),
        ..idle.clone()
    };
    // 0.50 (load) + 0.40 (mem) + 0.40 (disk) = 1.30
    assert!((unmeasured.congestion_score() - 1.30).abs() < 1e-4);
    assert!(unmeasured.congestion_score() > idle.congestion_score());
}
