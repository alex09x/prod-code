/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::create_test_fixture;
use crate::RustEngine;

// Isolate a possible Salsa deadlock so a regression cannot hang the test runner's shutdown.
fn run_writer_case(case: &str) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::snapshot_cancellation::snapshot_writer_child",
            "--nocapture",
        ])
        .env("PROD_CODE_SNAPSHOT_WRITER_CASE", case)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            assert!(
                status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "{case}: snapshot/write exceeded 30s; killed and reaped child: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn snapshot_overlay_writer_finishes() {
    run_writer_case("overlay-query");
}

#[test]
fn snapshot_base_writer_finishes() {
    run_writer_case("base");
}

#[test]
fn snapshot_direct_writer_finishes() {
    run_writer_case("direct");
}

#[test]
fn snapshot_active_query_is_cancelled_before_writer_takes_vfs() {
    run_writer_case("active");
}

#[test]
fn snapshot_writer_child() {
    let Ok(case) = std::env::var("PROD_CODE_SNAPSHOT_WRITER_CASE") else {
        return;
    };
    let (temp, path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).unwrap();
    let snapshot = engine.snapshot();
    assert!(!snapshot.document_symbols(&path).unwrap().is_empty());
    let file_id = snapshot.file_id_for_path(&path).unwrap();
    let active = (case == "active").then(|| snapshot.analysis().unwrap());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let writer_path = path.clone();
    let query_while_writing = case == "overlay-query";
    let writer = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let text = Some("pub fn updated() {}\n".to_owned());
        let result = match case.as_str() {
            "overlay-query" => engine.set_session_overlay(1, &writer_path, text),
            "base" => engine.update_base(&writer_path, text),
            "direct" | "active" => engine.apply_file_change(&writer_path, text.unwrap()),
            _ => unreachable!(),
        };
        done_tx.send(result).unwrap();
        engine
    });
    started_rx.recv().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    if let Some(analysis) = active {
        let deadline = Instant::now() + Duration::from_secs(2);
        while analysis
            .file_structure(
                &ra_ap_ide::FileStructureConfig {
                    exclude_locals: true,
                },
                file_id,
            )
            .is_ok()
        {
            assert!(
                Instant::now() < deadline,
                "active Salsa call was not cancelled"
            );
            std::thread::yield_now();
        }
        drop(analysis);
    }
    if query_while_writing {
        assert!(snapshot.document_symbols(&path).is_err());
    }
    // A held but idle snapshot must not keep the writer waiting either.
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("writer retained idle snapshot")
        .unwrap();
    assert!(
        snapshot.document_symbols(&path).is_err(),
        "stale snapshot survived write"
    );
    let engine = writer.join().unwrap();
    assert!(
        engine
            .document_symbols(&path)
            .unwrap()
            .iter()
            .any(|s| s.name == "updated")
    );
}
