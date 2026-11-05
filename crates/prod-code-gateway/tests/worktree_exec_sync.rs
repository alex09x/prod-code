/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A command run in a sibling Git worktree sees that worktree's uncommitted files, not the
//! committed ones its server copy was first seeded with (#1927).

use prod_code_mcp::tools::execute_tool;
use std::io::BufRead;
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[path = "common/live_gateway.rs"]
mod live_gateway;

struct Gateway {
    child: Child,
    reader: Option<std::thread::JoinHandle<()>>,
    addr: SocketAddr,
    _storage: tempfile::TempDir,
}

impl Gateway {
    fn start() -> Self {
        let storage = tempfile::tempdir().unwrap();
        let child = live_gateway::command(
            storage.path(),
            "a_sibling_worktree_runs_its_uncommitted_files",
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
        let mut gateway = Self {
            child,
            reader: None,
            addr: "127.0.0.1:0".parse().unwrap(),
            _storage: storage,
        };
        let stdout = gateway.child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        gateway.reader = Some(std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(address) = line.split("listening on ").nth(1)
                    && let Ok(address) = address.trim().parse::<SocketAddr>()
                {
                    let _ = tx.send(address);
                }
            }
        }));
        gateway.addr = rx.recv_timeout(Duration::from_secs(60)).unwrap();
        gateway
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn lines_of(gateway: SocketAddr, checkout: &Path, workspace: Option<&Path>) -> String {
    let mut args = serde_json::json!({ "argv": ["wc", "-l", "scripts/release/test_ci.py"] });
    if let Some(workspace) = workspace {
        args["workspace"] = serde_json::json!(workspace.to_string_lossy());
    }
    let result = execute_tool(gateway, checkout, "code_exec", args)
        .await
        .expect("the command runs");
    let text = serde_json::to_string(&result).unwrap();
    assert!(!result.is_error, "{text}");
    text
}

#[tokio::test]
async fn a_sibling_worktree_runs_its_uncommitted_files() {
    if live_gateway::serve_if_child().await {
        return;
    }
    let gateway = Gateway::start();
    let parent = tempfile::tempdir().unwrap();
    let main = parent.path().join("main");
    std::fs::create_dir_all(main.join("scripts/release")).unwrap();
    std::fs::write(main.join("README.md"), "fixture\n").unwrap();
    let committed: String = (0..40).map(|n| format!("# committed line {n}\n")).collect();
    std::fs::write(main.join("scripts/release/test_ci.py"), &committed).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["add", "."]);
    git(&main, &["commit", "-qm", "base"]);

    // The primary checkout runs a command first, as it does in practice: its copy on the node
    // is the one a worktree's copy is seeded from.
    let primary = lines_of(gateway.addr, &main, None).await;
    assert!(
        primary.contains("40 scripts/release/test_ci.py"),
        "{primary}"
    );

    let sibling = parent.path().join("sibling");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            sibling.to_str().unwrap(),
        ],
    );
    // An uncommitted edit of a tracked file: shorter than the committed one.
    std::fs::write(
        sibling.join("scripts/release/test_ci.py"),
        "# edited\n# in the sibling\n# worktree\n",
    )
    .unwrap();

    let seen = lines_of(gateway.addr, &main, Some(&sibling)).await;
    assert!(
        seen.contains("3 scripts/release/test_ci.py"),
        "the sibling's edited file must be what runs, not the committed 40 lines: {seen}"
    );

    // A second edit, run again: still the current content.
    std::fs::write(
        sibling.join("scripts/release/test_ci.py"),
        "# edited again\n",
    )
    .unwrap();
    let again = lines_of(gateway.addr, &main, Some(&sibling)).await;
    assert!(again.contains("1 scripts/release/test_ci.py"), "{again}");
}
