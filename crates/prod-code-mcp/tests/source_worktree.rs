/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::tools::{execute_tool, resolve_workspace_root};
use serde_json::json;
use std::path::Path;

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("main");
    let sibling = temp.path().join("other worktree");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("sample.rs"), "pub const VALUE: u32 = 1;\n").unwrap();
    git(&root, &["add", "."]);
    git(
        &root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    );
    git(
        &root,
        &[
            "worktree",
            "add",
            "--detach",
            sibling.to_str().unwrap(),
            "HEAD",
        ],
    );
    std::fs::write(sibling.join("sample.rs"), "pub const VALUE: u32 = 42;\n").unwrap();
    (
        temp,
        root.canonicalize().unwrap(),
        sibling.canonicalize().unwrap(),
    )
}

#[tokio::test]
async fn issue_1830_source_reads_verified_sibling_absolute_paths_uris_and_explicit_workspace() {
    let (_temp, root, sibling) = fixture();
    let absolute = sibling.join("sample.rs");
    let uri = format!("file://{}", absolute.to_str().unwrap().replace(' ', "%20"));
    for args in [
        json!({"path":absolute, "line":1,"context":0}),
        json!({"path":uri,"line":1,"context":0}),
        json!({"workspace":sibling,"path":"sample.rs","line":1,"context":0}),
        json!({"workspace":sibling,"path":absolute,"line":1,"context":0}),
    ] {
        // A dead node proves this is a confined local read, without sync or an analyzer.
        let result = execute_tool("127.0.0.1:9".parse().unwrap(), &root, "code_source", args)
            .await
            .unwrap();
        assert!(!result.is_error, "{result:?}");
        let text = serde_json::to_string(&result).unwrap();
        assert!(text.contains("42"), "{text}");
        assert!(!text.contains("VALUE: u32 = 1;"), "{text}");
    }
    let tool = prod_code_mcp::tools::list_tools()
        .into_iter()
        .find(|t| t.name == "code_source")
        .unwrap();
    assert_eq!(
        tool.input_schema["properties"]["workspace"]["type"],
        "string"
    );
}

#[test]
fn issue_1830_source_rejects_unrelated_workspaces_and_keeps_external_paths_remote() {
    let (temp, root, sibling) = fixture();
    let unrelated = temp.path().join("unrelated");
    std::fs::create_dir(&unrelated).unwrap();
    git(&unrelated, &["init", "-q"]);
    std::fs::write(unrelated.join("private.rs"), "private").unwrap();
    let mut args = json!({"workspace":unrelated,"path":"private.rs"});
    assert!(resolve_workspace_root(&root, "code_source", &mut args).is_err());
    let mut args = json!({"workspace":sibling,"path":"../main/sample.rs"});
    assert!(resolve_workspace_root(&root, "code_source", &mut args).is_err());
    for path in [
        unrelated.join("private.rs"),
        temp.path().join("sdk/include/source.h"),
    ] {
        let mut args = json!({"path":path});
        assert_eq!(
            resolve_workspace_root(&root, "code_source", &mut args).unwrap(),
            root
        );
        assert_eq!(
            prod_code_mcp::remote_fs::checkout_source_path(&root, path.to_str().unwrap()).unwrap(),
            None
        );
    }
}
