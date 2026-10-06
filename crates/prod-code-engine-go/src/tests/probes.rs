/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;

use crate::config::{GoConfig, HEALTH_PROBE_METHOD};
use crate::engine::GoEngine;
use crate::types::lock_unpoisoned;

#[cfg(unix)]
use super::fake_server::{
    assert_process_exits, fake_engine_with_probe, wait_for_gopls_probe_count,
};

#[tokio::test]
async fn zero_health_probe_interval_is_rejected_before_spawn() {
    let config = GoConfig {
        health_probe_interval: Some(Duration::ZERO),
        ..Default::default()
    };
    let error = match GoEngine::load(Path::new("."), config).await {
        Ok(_) => panic!("a zero interval would create an unbounded poll loop"),
        Err(error) => error,
    };
    assert!(
        format!("{error:#}").contains("greater than zero"),
        "{error:#}"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn matching_health_successes_and_errors_are_private_dispatch_evidence() {
    for health in ["success", "error"] {
        let (dir, engine) = fake_engine_with_probe(
            Some(("FAKE_HEALTH", health)),
            Duration::from_millis(80),
            Duration::from_millis(20),
        )
        .await;
        let mut subscriber = engine.subscribe();
        let seen_file = dir.path().join("seen");
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let seen = std::fs::read_to_string(&seen_file).unwrap_or_default();
                if seen
                    .lines()
                    .filter(|method| *method == HEALTH_PROBE_METHOD)
                    .count()
                    >= 2
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("two probes run after readiness settles");
        assert!(
            engine.is_alive(),
            "{health} replies keep the generation alive"
        );
        let seen = std::fs::read_to_string(seen_file).unwrap();
        assert!(
            seen.lines()
                .filter(|method| *method == HEALTH_PROBE_METHOD)
                .count()
                >= 2,
            "{seen}"
        );
        assert!(
            matches!(
                subscriber.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ),
            "probe replies are not broadcast"
        );
        drop(engine);
        assert_process_exits(&dir.path().join("pid")).await;
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_valid_probe_responses_reset_failures_without_ordinary_traffic() {
    let (dir, engine) = fake_engine_with_probe(
        Some(("FAKE_HEALTH", "delay")),
        Duration::from_millis(40),
        Duration::from_millis(100),
    )
    .await;
    let mut subscriber = engine.subscribe();
    wait_for_gopls_probe_count(&dir.path().join("seen"), 4).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while engine.health_probe_completions() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("at least two delayed matching replies are validated");
    assert!(
        engine.is_alive(),
        "late valid replies reset the failure streak"
    );
    assert!(matches!(
        subscriber.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_malformed_probe_responses_do_not_reset_failures() {
    let (dir, engine) = fake_engine_with_probe(
        Some(("FAKE_HEALTH", "delay-malformed")),
        Duration::from_millis(40),
        Duration::from_millis(100),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while engine.is_alive() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("three timeouts retire despite late malformed traffic");
    assert_eq!(engine.health_probe_completions(), 0);
    let seen = std::fs::read_to_string(dir.path().join("seen")).unwrap();
    assert_eq!(
        seen.lines()
            .filter(|method| *method == HEALTH_PROBE_METHOD)
            .count(),
        3,
        "{seen}"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_probe_timeouts_retain_at_most_one_response_slot() {
    let (dir, engine) = fake_engine_with_probe(
        Some(("FAKE_HEALTH", "silence")),
        Duration::from_millis(80),
        Duration::from_millis(10),
    )
    .await;
    let seen = dir.path().join("seen");
    for count in 1..=20 {
        wait_for_gopls_probe_count(&seen, count).await;
        engine
            .send_notification("prodCode/activity", serde_json::json!({}))
            .await
            .expect("ordinary activity overlaps the probe wait");
        assert!(engine.retained_health_responses() <= 1);
    }
    wait_for_gopls_probe_count(&seen, 21).await;
    assert!(
        engine.is_alive(),
        "busy deferrals do not become timeout failures"
    );
    assert_eq!(engine.retained_health_responses(), 1);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_idle_probe_response_timeouts_retire_the_owned_gopls() {
    let (dir, engine) = fake_engine_with_probe(
        Some(("FAKE_HEALTH", "silence")),
        Duration::from_millis(40),
        Duration::from_millis(20),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while engine.is_alive() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("three idle response timeouts retire gopls");
    assert!(engine.capabilities.read().await.is_none());
    assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
    assert_process_exits(&dir.path().join("pid")).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_during_a_probe_response_wait_kills_the_exact_owned_gopls() {
    let (dir, engine) = fake_engine_with_probe(
        Some(("FAKE_HEALTH", "silence")),
        Duration::from_secs(10),
        Duration::from_millis(20),
    )
    .await;
    let seen_file = dir.path().join("seen");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if std::fs::read_to_string(&seen_file)
                .unwrap_or_default()
                .lines()
                .any(|method| method == HEALTH_PROBE_METHOD)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the probe reaches gopls");
    drop(engine);
    assert_process_exits(&dir.path().join("pid")).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn primary_delayed_probe_without_ordinary_traffic_stays_alive() {
    let (dir, engine) = fake_engine_with_probe(
        Some(("FAKE_HEALTH", "delay")),
        Duration::from_millis(40),
        Duration::from_millis(60),
    )
    .await;
    let seen_file = dir.path().join("seen");
    let observation = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let seen = std::fs::read_to_string(&seen_file).unwrap_or_default();
            let scheduled = seen
                .lines()
                .filter(|method| *method == HEALTH_PROBE_METHOD)
                .count();
            if !engine.is_alive() || scheduled >= 5 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    let seen = std::fs::read_to_string(&seen_file).expect("probe log");
    let scheduled = seen
        .lines()
        .filter(|method| *method == HEALTH_PROBE_METHOD)
        .count();
    let alive = engine.is_alive();
    drop(engine);
    assert_process_exits(&dir.path().join("pid")).await;
    assert!(
        alive,
        "valid matching delayed probe replies must keep the responsive generation alive without ordinary traffic: {seen}"
    );
    assert!(
        observation.is_ok(),
        "scheduled probes must settle within their absolute observation budget: {seen}"
    );
    assert!(
        scheduled >= 5,
        "five scheduled probes reached the controlled server: {seen}"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocked_scheduled_probe_write_retires_its_exact_owned_child() {
    let (dir, engine) = fake_engine_with_probe(
        Some(("FAKE_STOP_READING", "1")),
        Duration::from_millis(80),
        Duration::from_millis(50),
    )
    .await;
    let mut writer = engine.stdin.lock().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let seen = std::fs::read_to_string(dir.path().join("seen")).unwrap_or_default();
            if seen.lines().any(|method| method == "initialized") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("controlled child stops reading after initialization");
    let deadline = tokio::time::Instant::now() + Duration::from_millis(30);
    loop {
        match tokio::time::timeout_at(deadline, writer.write(&[b'x'; 8192])).await {
            Ok(Ok(0)) => panic!("the owned pipe unexpectedly closed"),
            Ok(Ok(_)) => {}
            Ok(Err(error)) => panic!("filling the controlled pipe failed: {error}"),
            Err(_) => break,
        }
    }
    drop(writer);
    tokio::time::timeout(Duration::from_secs(2), async {
        while engine.is_alive() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the complete probe write budget retires the stalled stream");
    let refusal = engine
        .send_notification("prodCode/afterRetirement", serde_json::json!({}))
        .await;
    drop(engine);
    assert_process_exits(&dir.path().join("pid")).await;
    assert!(
        refusal.is_err(),
        "a queued notification cannot reuse the retired probe stream"
    );
}
