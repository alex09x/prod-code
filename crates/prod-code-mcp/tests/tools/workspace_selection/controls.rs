/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::fixture::DivergentWorktrees;
use super::*;

#[tokio::test]
async fn navigation_workspace_selector_rejects_unrelated_and_invalid_roots() {
    let trees = DivergentWorktrees::create();
    let unrelated = rust_workspace("pub fn unrelated_fn() {}\n");
    commit(&unrelated);
    let plain_dir = tempfile::tempdir().expect("plain non-git tempdir");
    let file_in_sibling = trees.sibling.join("src/lib.rs");
    let subdir_in_sibling = trees.sibling.join("src");
    let subdir_in_primary = trees.primary.join("src");
    let missing_dir = trees
        .primary
        .parent()
        .unwrap()
        .join("does-not-exist-worktree");
    let dotdot_root = format!("{}/src/..", trees.sibling.display());

    // Unrelated repository rejected across read-only navigation and grep tools.
    for (tool, mut args) in [
        (
            "code_definition",
            serde_json::json!({ "symbol": "target_fn" }),
        ),
        ("code_symbols", serde_json::json!({ "query": "target_fn" })),
        ("code_outline", serde_json::json!({ "path": "src/lib.rs" })),
        ("code_grep", serde_json::json!({ "pattern": "fn" })),
        (
            "code_references",
            serde_json::json!({ "symbol": "target_fn" }),
        ),
        ("code_callers", serde_json::json!({ "symbol": "target_fn" })),
        ("code_callees", serde_json::json!({ "symbol": "target_fn" })),
        ("code_hover", serde_json::json!({ "symbol": "target_fn" })),
        (
            "code_implementations",
            serde_json::json!({ "symbol": "TargetTrait" }),
        ),
        (
            "code_supertypes",
            serde_json::json!({ "symbol": "TargetTrait" }),
        ),
        ("code_type_at", serde_json::json!({ "symbol": "target_fn" })),
    ] {
        args["workspace"] = serde_json::json!(unrelated.root().to_string_lossy());
        let err = execute_tool(nowhere(), &trees.ws.root(), tool, args)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("same Git repository"),
            "{tool}: {err:#}"
        );
    }

    // Invalid workspace roots: relative, empty, non-string, missing, file, subdirectory, '..', non-Git.
    for (bad_workspace, expected) in [
        (serde_json::json!("relative/worktree"), "absolute path"),
        (serde_json::json!("   "), "cannot be empty"),
        (serde_json::json!(42), "absolute path"),
        (
            serde_json::json!(missing_dir.to_string_lossy()),
            "cannot be read",
        ),
        (
            serde_json::json!(file_in_sibling.to_string_lossy()),
            "not a directory",
        ),
        (
            serde_json::json!(subdir_in_sibling.to_string_lossy()),
            "root of a Git worktree",
        ),
        (
            serde_json::json!(subdir_in_primary.to_string_lossy()),
            "root of a Git worktree",
        ),
        (serde_json::json!(dotdot_root), "'..'"),
        (
            serde_json::json!(plain_dir.path().to_string_lossy()),
            "Git worktree",
        ),
    ] {
        let err = execute_tool(
            nowhere(),
            &trees.ws.root(),
            "code_definition",
            serde_json::json!({
                "workspace": bad_workspace,
                "symbol": "target_fn"
            }),
        )
        .await
        .expect_err("invalid workspace root must fail before network dispatch");
        assert!(
            format!("{err:#}").contains(expected),
            "expected `{expected}` in `{err:#}`"
        );
    }

    // The already configured root remains valid even when it is not a Git checkout.
    let plain_canon = std::fs::canonicalize(plain_dir.path()).unwrap();
    let err = execute_tool(
        nowhere(),
        &plain_canon,
        "code_definition",
        serde_json::json!({
            "workspace": plain_canon.to_string_lossy(),
            "symbol": "target_fn"
        }),
    )
    .await
    .expect_err("the placeholder gateway cannot be reached");
    assert!(!format!("{err:#}").contains("Git worktree"), "{err:#}");

    // Unsupported tools continue to reject explicit workspace selection.
    for unsupported in ["code_rename", "code_check", "code_slice", "code_source"] {
        let err = execute_tool(
            nowhere(),
            &trees.ws.root(),
            unsupported,
            serde_json::json!({
                "workspace": trees.sibling.to_string_lossy(),
                "symbol": "target_fn"
            }),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("workspace is supported only by"),
            "{unsupported}: {err:#}"
        );
    }

    trees.cleanup();
}

#[tokio::test]
async fn navigation_workspace_selector_rejects_paths_escaping_selected_worktree() {
    let trees = DivergentWorktrees::create();
    let outside_dir = tempfile::tempdir_in(trees.primary.parent().unwrap()).expect("outside dir");
    let outside_file = outside_dir.path().join("secret.rs");
    std::fs::write(&outside_file, "pub fn leaked() {}\n").expect("write outside file");
    let outside_uri = url::Url::from_file_path(&outside_file)
        .expect("outside uri")
        .to_string();

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside_file, trees.sibling.join("src/escaped_link.rs"))
            .expect("create escaping symlink");
        std::os::unix::fs::symlink(
            outside_dir.path().join("missing_secret.rs"),
            trees.sibling.join("src/dangling_escape.rs"),
        )
        .expect("create dangling escaping symlink");
    }

    let nested_git = trees.sibling.join("nested-checkout");
    std::fs::create_dir_all(&nested_git).expect("create nested dir");
    std::fs::write(nested_git.join(".git"), "gitdir: /tmp/fake-nested-gitdir\n")
        .expect("write nested .git marker");

    let sibling_str = trees.sibling.to_string_lossy().into_owned();
    let primary_file = trees
        .primary
        .join("src/lib.rs")
        .to_string_lossy()
        .into_owned();
    let sibling_file = trees
        .sibling
        .join("src/lib.rs")
        .to_string_lossy()
        .into_owned();
    let outside_str = outside_file.to_string_lossy().into_owned();

    let mut escaping_paths = vec![
        "../outside.rs".to_string(),
        "nonexistent/../../outside.rs".to_string(),
        primary_file,
        outside_str.clone(),
    ];
    #[cfg(unix)]
    {
        escaping_paths.push("src/escaped_link.rs".to_string());
        escaping_paths.push("src/dangling_escape.rs".to_string());
    }

    for bad_path in &escaping_paths {
        for (tool, mut args) in [
            ("code_outline", serde_json::json!({ "path": bad_path })),
            (
                "code_grep",
                serde_json::json!({ "pattern": "fn", "path": bad_path }),
            ),
            (
                "code_definition",
                serde_json::json!({ "symbol": "target_fn", "path": bad_path }),
            ),
            (
                "code_hover",
                serde_json::json!({ "path": bad_path, "line": 1, "character": 1 }),
            ),
        ] {
            args["workspace"] = serde_json::json!(sibling_str);
            let err = execute_tool(nowhere(), &trees.ws.root(), tool, args)
                .await
                .unwrap_err();
            assert!(
                format!("{err:#}").contains("outside the selected workspace"),
                "{tool} ({bad_path}): {err:#}"
            );
        }
    }

    for uri in [outside_uri, "file:/outside.rs".to_string()] {
        let err = execute_tool(
            nowhere(),
            &trees.ws.root(),
            "code_grep",
            serde_json::json!({ "workspace": sibling_str, "pattern": "fn", "path": uri }),
        )
        .await
        .expect_err("file URIs must be refused before dispatch");
        assert!(format!("{err:#}").contains("file URI"), "{err:#}");
    }

    // Alias keys (`file_path`) and primary-selected -> sibling-path must also be confined.
    let alias_err = execute_tool(
        nowhere(),
        &trees.ws.root(),
        "code_definition",
        serde_json::json!({
            "workspace": sibling_str,
            "symbol": "target_fn",
            "file_path": outside_str
        }),
    )
    .await
    .expect_err("file_path alias must be confined");
    assert!(format!("{alias_err:#}").contains("outside the selected workspace"));

    let cross_err = execute_tool(
        nowhere(),
        &trees.ws.root(),
        "code_outline",
        serde_json::json!({
            "workspace": trees.primary.to_string_lossy(),
            "path": sibling_file
        }),
    )
    .await
    .expect_err("explicit primary workspace must reject sibling path");
    assert!(format!("{cross_err:#}").contains("outside the selected workspace"));

    let nested_err = execute_tool(
        nowhere(),
        &trees.ws.root(),
        "code_outline",
        serde_json::json!({
            "workspace": sibling_str,
            "path": "nested-checkout/lib.rs"
        }),
    )
    .await
    .expect_err("nested git marker inside selected worktree must be rejected");
    assert!(
        format!("{nested_err:#}").contains("nested Git worktree"),
        "{nested_err:#}"
    );

    trees.cleanup();
}
