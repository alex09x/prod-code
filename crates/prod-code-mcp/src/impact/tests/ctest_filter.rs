/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::impact::Symbol;
use crate::impact::test_cmd::test_command;
use crate::verify::{CppBuild, ProjectTools};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

async fn capture(root: &Path, argv: &[String], label: &str) -> Vec<u8> {
    let stdout = root.join(format!("{label}.out"));
    let stderr = root.join(format!("{label}.err"));
    let mut child = tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::fs::File::create(&stdout).unwrap()))
        .stderr(Stdio::from(std::fs::File::create(&stderr).unwrap()))
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .expect("CMake/CTest fixture prerequisite must be installed");
    let pid = child.id().unwrap();
    let status = match tokio::time::timeout(Duration::from_secs(20), child.wait()).await {
        Ok(status) => status.unwrap(),
        Err(_) => {
            // SAFETY: the process group belongs to this fixture's owned child.
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            panic!("{label} fixture timed out");
        }
    };
    assert!(
        status.success(),
        "{label}: {}",
        std::fs::read_to_string(stderr).unwrap()
    );
    std::fs::read(stdout).unwrap()
}

#[tokio::test]
async fn ctest_filter_selects_registered_names_literally() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let intended = [
        "adds [two]",
        "Price.Doubles",
        "works (fast)",
        "a+b",
        "left|right",
    ];
    let decoys = [
        "adds t",
        "PriceXDoubles",
        "works fast",
        "aaab",
        "left",
        "right",
    ];
    let mut project = String::from(
        "cmake_minimum_required(VERSION 3.20)\nproject(filters NONE)\nenable_testing()\n",
    );
    for name in intended.into_iter().chain(decoys) {
        project.push_str(&format!(
            "add_test(NAME [=[{name}]=] COMMAND cmake -E true)\n"
        ));
    }
    std::fs::write(root.join("CMakeLists.txt"), project).unwrap();
    capture(
        root,
        &["cmake", "-S", ".", "-B", "build"].map(str::to_string),
        "configure",
    )
    .await;
    let tests: Vec<_> = intended
        .iter()
        .map(|name| Symbol {
            name: (*name).into(),
            file: "tests.cpp".into(),
            line: 1,
            col: 1,
        })
        .collect();
    let tools = ProjectTools {
        cpp: CppBuild::CMake,
        ..Default::default()
    };
    let mut command = test_command("cpp", &tools, &tests).unwrap();
    // Keep the generated build and selector unchanged; ask CTest to expose its selection.
    command[2].push_str(" --show-only=json-v1");
    let output = capture(root, &command, "selection").await;
    let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
    let mut selected: Vec<_> = report["tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|test| test["name"].as_str().unwrap())
        .collect();
    let mut expected = intended.to_vec();
    selected.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        selected, expected,
        "CTest must select only exact registered names"
    );
}
