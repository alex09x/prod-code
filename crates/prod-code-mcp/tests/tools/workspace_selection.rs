/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::process::Command;

fn git(root: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git starts");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn alternate_worktree(ws: &Workspace) -> PathBuf {
    let root = ws.root();
    let name = root.file_name().unwrap().to_string_lossy().into_owned();
    let path = root
        .parent()
        .unwrap()
        .join(format!("{name}-caller-worktree"));
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["worktree", "add", "--detach"])
        .arg(&path)
        .arg("HEAD")
        .output()
        .expect("git worktree add starts");
    assert!(
        output.status.success(),
        "git worktree add failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(
        path.join("Cargo.toml"),
        "[package]\nname = \"caller_worktree\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("alternate manifest is written");
    path
}

#[tokio::test]
async fn code_test_and_exec_sync_the_explicit_caller_worktree() {
    let advertised = prod_code_mcp::tools::list_tools();
    for name in ["code_test", "code_exec"] {
        let tool = advertised.iter().find(|tool| tool.name == name).unwrap();
        assert!(
            tool.input_schema["properties"].get("workspace").is_some(),
            "{name} must advertise the workspace selector"
        );
    }

    let ws = rust_workspace("pub fn base() {}\n");
    commit(&ws);
    let primary = std::fs::canonicalize(ws.root()).unwrap();

    let primary_script = Script {
        exec_stdout: b"running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored\n".to_vec(),
        exec_exit: Some(0),
        ..Script::default()
    };
    let primary_roots = primary_script.workspace_roots.clone();
    let primary_remote = mock_gateway(primary_script).await;
    let empty = execute_tool(
        primary_remote,
        &ws.root(),
        "code_test",
        serde_json::json!({ "filter": "branch_only_test" }),
    )
    .await
    .expect("the no-match report is returned");
    assert!(empty.is_error, "a filtered zero-test run looked green");
    assert!(
        text_of(&empty).contains("No Rust tests matched filter `branch_only_test`"),
        "{}",
        text_of(&empty)
    );
    assert!(
        primary_roots
            .lock()
            .unwrap()
            .contains(&primary.display().to_string())
    );

    let alternate = alternate_worktree(&ws);
    let alternate = std::fs::canonicalize(alternate).unwrap();
    let test_script = Script {
        exec_stdout: b"running 1 test\ntest branch_only_test ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored\n".to_vec(),
        exec_exit: Some(0),
        ..Script::default()
    };
    let test_roots = test_script.workspace_roots.clone();
    let test_remote = mock_gateway(test_script).await;
    let tested = execute_tool(
        test_remote,
        &ws.root(),
        "code_test",
        serde_json::json!({
            "workspace": alternate.to_string_lossy(),
            "path": "src/lib.rs",
            "filter": "branch_only_test"
        }),
    )
    .await
    .expect("the selected worktree tests run");
    assert!(!tested.is_error, "{}", text_of(&tested));
    assert!(
        text_of(&tested).contains("cargo test -p caller_worktree branch_only_test"),
        "{}",
        text_of(&tested)
    );
    assert!(
        test_roots
            .lock()
            .unwrap()
            .contains(&alternate.display().to_string())
    );

    let exec_script = Script {
        exec_stdout: b"caller worktree command\n".to_vec(),
        exec_exit: Some(0),
        ..Script::default()
    };
    let exec_roots = exec_script.workspace_roots.clone();
    let exec_remote = mock_gateway(exec_script).await;
    let executed = execute_tool(
        exec_remote,
        &ws.root(),
        "code_exec",
        serde_json::json!({
            "workspace": alternate.to_string_lossy(),
            "cwd": ".",
            "argv": ["pwd"]
        }),
    )
    .await
    .expect("exec runs in the selected worktree");
    assert!(!executed.is_error, "{}", text_of(&executed));
    assert!(
        text_of(&executed).contains("caller worktree command"),
        "{}",
        text_of(&executed)
    );
    assert!(
        exec_roots
            .lock()
            .unwrap()
            .contains(&alternate.display().to_string())
    );

    let automatic_test_script = Script {
        exec_stdout: b"running 1 test\ntest branch_only_test ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored\n".to_vec(),
        exec_exit: Some(0),
        ..Script::default()
    };
    let automatic_test_roots = automatic_test_script.workspace_roots.clone();
    let automatic_test_remote = mock_gateway(automatic_test_script).await;
    let automatic_path = alternate.join("src/lib.rs").to_string_lossy().to_string();
    let automatic_test = execute_tool(
        automatic_test_remote,
        &ws.root(),
        "code_test",
        serde_json::json!({ "path": automatic_path, "filter": "branch_only_test" }),
    )
    .await
    .expect("the worktree path selects its own checkout");
    assert!(!automatic_test.is_error, "{}", text_of(&automatic_test));
    assert!(
        text_of(&automatic_test).contains("cargo test -p caller_worktree branch_only_test"),
        "{}",
        text_of(&automatic_test)
    );
    assert!(
        automatic_test_roots
            .lock()
            .unwrap()
            .contains(&alternate.display().to_string())
    );

    let automatic_exec_script = Script {
        exec_stdout: b"automatically selected worktree\n".to_vec(),
        exec_exit: Some(0),
        ..Script::default()
    };
    let automatic_exec_roots = automatic_exec_script.workspace_roots.clone();
    let automatic_exec_remote = mock_gateway(automatic_exec_script).await;
    let automatic_exec = execute_tool(
        automatic_exec_remote,
        &ws.root(),
        "code_exec",
        serde_json::json!({
            "cwd": alternate.to_string_lossy(),
            "argv": ["pwd"]
        }),
    )
    .await
    .expect("the worktree cwd selects its own checkout");
    assert!(!automatic_exec.is_error, "{}", text_of(&automatic_exec));
    assert!(
        automatic_exec_roots
            .lock()
            .unwrap()
            .contains(&alternate.display().to_string())
    );
    git(
        &ws.root(),
        &["worktree", "remove", "--force", alternate.to_str().unwrap()],
    );
}

#[tokio::test]
async fn workspace_selector_rejects_unrelated_and_invalid_nested_roots() {
    let ws = rust_workspace("pub fn base() {}\n");
    commit(&ws);
    let other = rust_workspace("pub fn other() {}\n");
    commit(&other);
    let error = execute_tool(
        nowhere(),
        &ws.root(),
        "code_test",
        serde_json::json!({ "workspace": other.root().to_string_lossy() }),
    )
    .await
    .expect_err("a different repository cannot be selected");
    assert!(
        format!("{error:#}").contains("same Git repository"),
        "{error:#}"
    );

    let fake_nested = ws.root().join("review-worktrees/pr50");
    std::fs::create_dir_all(&fake_nested).unwrap();
    std::fs::write(fake_nested.join(".git"), "gitdir: /tmp/mock-gitdir\n").unwrap();
    let nested_error = execute_tool(
        nowhere(),
        &ws.root(),
        "code_exec",
        serde_json::json!({
            "argv": ["pwd"],
            "cwd": "review-worktrees/pr50"
        }),
    )
    .await
    .expect_err("an invalid nested worktree must be refused");
    assert!(
        format!("{nested_error:#}").contains("nested Git worktree"),
        "{nested_error:#}"
    );
}

#[tokio::test]
async fn code_test_refuses_an_existing_path_outside_the_selected_workspace() {
    let ws = rust_workspace("pub fn base() {}\n");
    commit(&ws);
    let other = rust_workspace("pub fn other() {}\n");
    commit(&other);
    for args in [
        serde_json::json!({ "path": other.path("src/lib.rs").to_string_lossy() }),
        serde_json::json!({
            "workspace": ws.root().to_string_lossy(),
            "path": other.path("src/lib.rs").to_string_lossy()
        }),
    ] {
        let error = execute_tool(nowhere(), &ws.root(), "code_test", args)
            .await
            .expect_err("an existing out-of-scope path must not widen to --workspace");
        assert!(format!("{error:#}").contains("outside"), "{error:#}");
    }

    let linked = alternate_worktree(&ws);
    let error = execute_tool(
        nowhere(),
        &ws.root(),
        "code_test",
        serde_json::json!({
            "workspace": ws.root().to_string_lossy(),
            "path": linked.join("src/lib.rs").to_string_lossy()
        }),
    )
    .await
    .expect_err("an explicit primary workspace cannot silently widen to a linked worktree");
    assert!(
        format!("{error:#}").contains("outside the selected workspace"),
        "{error:#}"
    );
    git(
        &ws.root(),
        &["worktree", "remove", "--force", linked.to_str().unwrap()],
    );
}

#[tokio::test]
async fn a_filtered_compile_failure_is_not_reported_as_no_tests_matched() {
    let ws = rust_workspace("pub fn base() {}\n");
    let remote = mock_gateway(Script {
        exec_stderr: b"error: could not compile the test target\n".to_vec(),
        exec_exit: Some(101),
        ..Script::default()
    })
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_test",
        serde_json::json!({ "filter": "branch_only_test" }),
    )
    .await
    .expect("the compile failure is reported");
    let text = text_of(&result);
    assert!(result.is_error, "{text}");
    assert!(!text.contains("No Rust tests matched"), "{text}");
    assert!(text.contains("could not compile"), "{text}");
}

#[path = "workspace_selection/controls.rs"]
mod controls;
#[path = "workspace_selection/fixture.rs"]
mod fixture;
#[path = "workspace_selection/navigation.rs"]
mod navigation;
