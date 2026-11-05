/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::{engine_project, expected_engine};
use std::path::Path;

mod review;

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn package(root: &Path, relative: &str, name: &str, independent: bool) {
    let extra = if independent { "\n[workspace]\n" } else { "" };
    write(
        root,
        &format!("{relative}/Cargo.toml"),
        &format!("[package]\nname = {name:?}\nversion = '0.1.0'\nedition = '2024'\n{extra}"),
    );
    write(
        root,
        &format!("{relative}/src/lib.rs"),
        "pub fn sample() {}\n",
    );
}

/// Cargo runs only inside this fixture on the remote test node. Its project model is
/// the oracle; these tests assert application routing, not TOML/glob parser behavior.
fn cargo_members(root: &Path, expected: &[&str]) {
    let output = std::process::Command::new("cargo")
        .args([
            "metadata",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .current_dir(root)
        .output()
        .expect("remote Cargo metadata");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let ids = metadata["workspace_members"].as_array().unwrap();
    let canonical = root.canonicalize().unwrap();
    let mut actual: Vec<_> = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|package| ids.contains(&package["id"]))
        .map(|package| {
            Path::new(package["manifest_path"].as_str().unwrap())
                .parent()
                .unwrap()
                .strip_prefix(&canonical)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    actual.sort();
    let mut expected = expected.to_vec();
    expected.sort();
    assert_eq!(actual, expected, "Cargo membership oracle");
}

fn routed(root: &Path, relative: &str, subpath: Option<&str>, engine: &'static str) {
    let expected = (subpath.map(str::to_owned), Some(engine));
    assert_eq!(
        engine_project(root, Path::new(relative)),
        expected,
        "{relative}"
    );
    assert_eq!(
        engine_project(root, &root.join(relative)),
        expected,
        "absolute {relative}"
    );
}

#[test]
fn project_root_regression_manifestless_go_child() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        "server/go.mod",
        "module example.com/server\n\ngo 1.23\n",
    );
    write(root, "server/cmd/main.go", "package main\nfunc main() {}\n");
    assert_eq!(expected_engine(root), Some("go"), "keep language inference");
    routed(root, "server/cmd/main.go", Some("server"), "go");
}

#[test]
fn project_root_regression_empty_virtual_workspace() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = '2'\nmembers = []\n",
    );
    package(root, "child", "child", true);
    cargo_members(&root.join("child"), &[""]);
    routed(root, "child/src/lib.rs", Some("child"), "rust");
}

#[test]
fn project_root_regression_literal_exclusion_in_glob() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = '2'\nmembers = [\"crates/*\"]\nexclude = ['crates/skip']\n",
    );
    package(root, "crates/keep", "keep", false);
    package(root, "crates/skip", "skip", true);
    cargo_members(root, &["crates/keep"]);
    routed(root, "crates/keep/src/lib.rs", None, "rust");
    routed(root, "crates/skip/src/lib.rs", Some("crates/skip"), "rust");
}

#[test]
fn project_root_regression_package_exclude_is_not_workspace_exclude() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = '2'\nmembers = [\"crates/member\"]\n[package]\nname = 'root'\nversion = '0.1.0'\nedition = '2024'\nexclude = [\"crates/member\"]\n",
    );
    write(root, "src/lib.rs", "pub fn root() {}\n");
    package(root, "crates/member", "member", false);
    cargo_members(root, &["", "crates/member"]);
    routed(root, "crates/member/src/lib.rs", None, "rust");
}

#[test]
fn project_root_regression_interior_member_glob() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = '2'\nmembers = [\"crates/foo-*\"]\n",
    );
    package(root, "crates/foo-one", "foo-one", false);
    package(root, "crates/foo-two", "foo-two", false);
    package(root, "crates/foo-one/nested", "nested", true);
    package(root, "other", "other", true);
    cargo_members(root, &["crates/foo-one", "crates/foo-two"]);
    routed(root, "other/src/lib.rs", Some("other"), "rust");
    routed(
        root,
        "crates/foo-one/nested/src/lib.rs",
        Some("crates/foo-one/nested"),
        "rust",
    );
    routed(root, "crates/foo-one/src/lib.rs", None, "rust");
    routed(root, "crates/foo-two/src/lib.rs", None, "rust");
}

#[test]
fn project_root_regression_cargo_string_forms_and_path_separators() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        "Cargo.toml",
        r#"workspace.resolver = '2'
workspace.members = ['./crates/foo-?/', "crates\u002fbar-[ab]", '''crates/baz''']
workspace.exclude = ['crates/bar-b'] # packaging tables must not affect these lists
[package]
name = 'root'
version = '0.1.0'
edition = '2024'
exclude = ['crates/baz']
"#,
    );
    write(root, "src/lib.rs", "pub fn root() {}\n");
    for (path, name, independent) in [
        ("crates/foo-a", "foo-a", false),
        ("crates/bar-a", "bar-a", false),
        ("crates/bar-b", "bar-b", true),
        ("crates/baz", "baz", false),
        ("crates/foo-a/nested", "nested", true),
        ("crates/foo-ab", "foo-ab", true),
    ] {
        package(root, path, name, independent);
    }
    cargo_members(root, &["", "crates/foo-a", "crates/bar-a", "crates/baz"]);
    for path in ["crates/foo-a", "crates/bar-a", "crates/baz"] {
        routed(root, &format!("{path}/src/lib.rs"), None, "rust");
    }
    for path in ["crates/bar-b", "crates/foo-a/nested", "crates/foo-ab"] {
        routed(root, &format!("{path}/src/lib.rs"), Some(path), "rust");
    }
}

#[test]
fn project_root_regression_preserves_direct_bare_language_root() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(root, "main.lua", "return {}\n");
    write(root, "child/.luarc.json", "{}\n");
    write(root, "child/main.lua", "return {}\n");
    assert_eq!(expected_engine(root), Some("lua"));
    routed(root, "child/main.lua", None, "lua");
}
