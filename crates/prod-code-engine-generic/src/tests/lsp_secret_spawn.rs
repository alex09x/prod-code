/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "../../../../tests/support/lsp_secret_spawn.rs"]
mod fixture;

use crate::{GenericLspConfig, GenericLspEngine, types::lock_unpoisoned};
use std::time::Duration;

const DRIVER: &str = concat!(module_path!(), "::lsp_secret_spawn_driver");

#[tokio::test]
async fn lsp_secret_spawn_inherited() {
    fixture::run_driver(DRIVER, "inherited").await;
}

#[tokio::test]
async fn lsp_secret_spawn_configured() {
    fixture::run_driver(DRIVER, "configured").await;
}

#[tokio::test]
#[ignore = "invoked in an isolated subprocess with fixture-only environment"]
async fn lsp_secret_spawn_driver() {
    let fixture = fixture::Fixture::new();
    let engine = tokio::time::timeout(
        Duration::from_secs(10),
        GenericLspEngine::spawn(
            fixture.root.path(),
            GenericLspConfig {
                command: "python3".to_string(),
                args: fixture.args.clone(),
                env: fixture.env.iter().cloned().collect(),
                health_probe_interval: None,
                ..Default::default()
            },
        ),
    )
    .await
    .expect("generic engine initializes its fake server")
    .unwrap();
    engine
        .send_notification("exit", serde_json::json!({}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while lock_unpoisoned(&engine._child)
            .try_wait()
            .unwrap()
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("generic engine reaps its fake server");
    fixture.assert_scrubbed();
}
