/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[tokio::test]
async fn virtual_workspace_container_paths_refuse_before_any_earlier_write() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers=[\"crates/member\"]\nresolver=\"3\"\n",
    );
    write(
        root,
        "crates/member/Cargo.toml",
        "[package]\nname=\"scope_member\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    write(
        root,
        "crates/member/src/lib.rs",
        "pub fn value() -> u32 { 1 }\n",
    );
    write(root, "README.md", "overview\n");
    write(root, "docs/readme.md", "notes\n");
    let preview = edits::preview(
        root,
        &[edits::BlockEdit {
            path: "crates/member/src/lib.rs".into(),
            old: "{ 1 }".into(),
            new: "{ 2 }".into(),
        }],
    )
    .unwrap();
    for kind in ["check", "lint", "test"] {
        for path in ["crates", "docs", "Cargo.toml", "README.md"] {
            let mut args = json!({"kind":kind,"path":path});
            if kind == "test" {
                args["filter"] = json!("scope_regression");
            }
            let result = execute(
                "127.0.0.1:1".parse().unwrap(),
                root,
                json!({"apply":true,"steps":[
                    {"id":"write","tool":"apply","args":{"files":preview.files}},
                    {"id":"check","tool":"verify","args":args}
                ]}),
            )
            .await;
            assert!(result.is_err(), "accepted {kind} at {path}");
            assert!(
                std::fs::read_to_string(root.join("crates/member/src/lib.rs"))
                    .unwrap()
                    .contains("{ 1 }")
            );
        }
    }
}

#[test]
fn go_root_files_cannot_silently_select_every_package() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "go.mod", "module example.test/scope\n\ngo 1.22\n");
    write(root, "README.md", "overview\n");
    write(root, "main.go", "package main\nfunc main() {}\n");
    for kind in ["check", "lint", "test"] {
        for path in ["README.md", "go.mod", "main.go"] {
            let mut args = json!({"kind":kind,"path":path});
            if kind == "test" {
                args["filter"] = json!("TestScope");
            }
            assert!(
                verification::request(root, args).is_err(),
                "accepted {kind} at {path}"
            );
        }
    }
}

#[test]
fn scoped_packages_and_python_root_source_remain_accepted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers=[\"crates/member\"]\nresolver=\"3\"\n",
    );
    write(
        root,
        "crates/member/Cargo.toml",
        "[package]\nname=\"scope_member\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    write(root, "crates/member/src/lib.rs", "pub fn value() {}\n");
    for kind in ["check", "lint", "test"] {
        let mut args = json!({"kind":kind,"path":"crates/member"});
        if kind == "test" {
            args["filter"] = json!("scope_regression");
        }
        assert!(
            verification::request(root, args).is_ok(),
            "refused member {kind}"
        );
    }
    write(root, "go.mod", "module example.test/scope\n\ngo 1.22\n");
    write(root, "sub/main.go", "package sub\nfunc Value() {}\n");
    verification::request(root, json!({"kind":"check","path":"sub/main.go"})).unwrap();
    write(
        root,
        "pyproject.toml",
        "[project]\nname=\"scope\"\nversion=\"0.1.0\"\n",
    );
    write(root, "sample.py", "value: int = 1\n");
    verification::request(root, json!({"kind":"check","path":"sample.py"})).unwrap();
}

#[test]
fn a_test_script_that_ignores_filters_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "package.json",
        r#"{"name":"scope","scripts":{"test":"node tests.js"}}"#,
    );
    write(root, "src/index.ts", "export const value = 1;\n");
    // The filter happens to equal an existing command argument. Presence alone
    // cannot prove that the runner used it to narrow the test selection.
    assert!(
        verification::request(root, json!({"kind":"test","path":"src","filter":"test"})).is_err()
    );
    write(
        root,
        "package.json",
        r#"{"name":"scope","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"1.0.0"}}"#,
    );
    assert!(
        verification::request(
            root,
            json!({"kind":"test","path":"src","filter":"scope_regression"})
        )
        .is_ok()
    );
}

#[test]
fn root_package_source_selects_only_that_package_even_with_sibling_members() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname=\"root_package\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\nmembers=[\"member\"]\n",
    );
    write(root, "src/lib.rs", "pub fn value() {}\n");
    write(
        root,
        "member/Cargo.toml",
        "[package]\nname=\"sibling\"\nversion=\"0.1.0\"\n",
    );
    write(root, "member/src/lib.rs", "pub fn other() {}\n");
    for kind in [
        crate::verify::VerifyKind::Check,
        crate::verify::VerifyKind::Lint,
    ] {
        let plan = crate::verify::plan::scoped::plan_scoped(
            root,
            Some(&root.join("src/lib.rs")),
            kind,
            None,
        )
        .unwrap();
        assert!(
            plan.command
                .windows(2)
                .any(|args| args == ["-p", "root_package"]),
            "{:?}",
            plan.command
        );
        assert!(!plan.command.iter().any(|arg| arg == "--workspace"));
        verification::request(root, json!({"kind":kind.label(),"path":"src/lib.rs"})).unwrap();
    }
}

#[tokio::test]
async fn a_changed_manifest_is_rechecked_before_any_remote_execution() {
    use crate::verify::{
        VerifyKind,
        plan::scoped::{WorkflowPlanRefusal, plan_narrow, with_narrow_scope},
    };
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    write(
        &root,
        "Cargo.toml",
        "[package]\nname=\"scope\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    );
    write(&root, "src/lib.rs", "pub fn value() {}\n");
    let target = root.join("src/lib.rs");
    plan_narrow(&root, Some(&target), VerifyKind::Check, None).unwrap();
    write(&root, "Cargo.toml", "[workspace]\nmembers=[]\n");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let result = with_narrow_scope(crate::verify::run_verify(
        listener.local_addr().unwrap(),
        &root,
        Some(Path::new("src/lib.rs")),
        VerifyKind::Check,
        None,
        1,
    ))
    .await;
    let error = result.unwrap_err();
    assert!(error.is::<WorkflowPlanRefusal>(), "{error:#}");
    assert!(format!("{error:#}").contains("whole rust workspace"));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}
