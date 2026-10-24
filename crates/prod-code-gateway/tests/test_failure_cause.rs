/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `code_test` through a real gateway and real cargo shows what failed the run (#1230), even
//! when the library's warnings come before the test target's error.

use prod_code_mcp::verify::{VerifyKind, run_verify};
use std::io::BufRead;
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[path = "common/live_gateway.rs"]
mod live_gateway;

/// The item cap `code_test` renders with.
const MCP_ITEMS: usize = 40;

struct Gateway {
    child: Child,
    addr: SocketAddr,
    _storage: tempfile::TempDir,
}

impl Gateway {
    fn start() -> Self {
        let storage = tempfile::tempdir().unwrap();
        let mut child = live_gateway::command(storage.path(), "a_failed_test_run_names_its_cause")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(address) = line.split("listening on ").nth(1)
                    && let Ok(address) = address.trim().parse::<SocketAddr>()
                {
                    let _ = tx.send(address);
                }
            }
        });
        let addr = rx.recv_timeout(Duration::from_secs(60)).unwrap();
        Self {
            child,
            addr,
            _storage: storage,
        }
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A library with enough unused variables that its warnings alone exceed the item cap.
fn noisy_library(test_module: &str) -> String {
    let mut text: String = (1..=50)
        .map(|i| format!("pub fn item{i}() {{ let unused{i} = {i}; }}\n"))
        .collect();
    text.push_str(test_module);
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_test_run_names_its_cause() {
    if live_gateway::serve_if_child().await {
        return;
    }
    let gateway = Gateway::start();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("noisy");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"noisy\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    git(&root, &["init", "-q"]);

    let unit = |body: &str| {
        format!("#[cfg(test)]\nmod tests {{\n    #[test]\n    fn focused() {{ {body} }}\n}}\n")
    };
    let integration = |body: &str| format!("#[test]\nfn focused() {{ noisy::item1(); {body} }}\n");
    // (case, library, integration test, ok, must contain, must not contain)
    let cases = [
        (
            "integration test does not compile",
            noisy_library(""),
            integration("let value: u32 = \"text\";"),
            false,
            vec!["error: [E0308] mismatched types (tests/it.rs:2:"],
            vec!["--- output tail ---"],
        ),
        (
            "unit test in the library does not compile",
            noisy_library(&unit("let value: u32 = \"text\";")),
            integration(""),
            false,
            vec!["error: [E0308] mismatched types (src/lib.rs:"],
            vec!["--- output tail ---"],
        ),
        (
            "integration test fails its assertion",
            noisy_library(""),
            integration("assert_eq!(1 + 1, 3, \"sum is off\");"),
            false,
            vec!["--- FAILED focused ---", "sum is off"],
            vec!["--- output tail ---"],
        ),
        (
            "integration test aborts the test process",
            noisy_library(""),
            integration("std::process::abort();"),
            false,
            vec!["SIGABRT"],
            vec![],
        ),
        (
            "integration test passes",
            noisy_library(""),
            integration(""),
            true,
            vec!["rust test: OK", "more diagnostic(s)"],
            vec!["--- output tail ---", "error:"],
        ),
    ];
    for (case, library, test, ok, present, absent) in cases {
        std::fs::write(root.join("src/lib.rs"), library).unwrap();
        std::fs::write(root.join("tests/it.rs"), test).unwrap();
        git(&root, &["add", "-A"]);
        let report = run_verify(
            gateway.addr,
            &root,
            None,
            VerifyKind::Test,
            Some("focused"),
            600,
        )
        .await
        .unwrap_or_else(|e| panic!("{case}: {e:#}"));
        let text = report.render(MCP_ITEMS);
        assert_eq!(report.ok(), ok, "{case}:\n{text}");
        assert!(
            report.warnings() > MCP_ITEMS,
            "{case}: the warnings must exceed the cap\n{text}"
        );
        for needle in present {
            assert!(
                text.contains(needle),
                "{case}: missing {needle:?} in\n{text}"
            );
        }
        for needle in absent {
            assert!(
                !text.contains(needle),
                "{case}: unexpected {needle:?} in\n{text}"
            );
        }
        eprintln!("test failure case passed: {case}");
    }
}
