/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

enum Topology {
    Loose,
    Detached,
    Packed,
    Linked,
    Unborn,
}

fn git(root: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("fixture git starts");
    assert!(
        result.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn run(root: &Path) -> String {
    let result = Command::new("cargo")
        .args(["run", "--offline", "--quiet"])
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join("target"))
        .env("TEST_BUILD_COUNTER", root.join(".counter"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("PROD_CODE_GIT_COMMIT")
        .output()
        .expect("fixture Cargo starts");
    assert!(
        result.status.success(),
        "fixture Cargo: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn count(root: &Path) -> usize {
    std::fs::read(root.join(".counter")).unwrap().len()
}

fn fixture(container: &Path, topology: &Topology) -> PathBuf {
    let root = container.join("base");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='build-metadata-fixture'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n.counter\n").unwrap();
    std::fs::write(root.join("marker"), "first\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() { println!(\"{}\", option_env!(\"PROD_CODE_GIT_COMMIT\").unwrap_or(\"unknown\")); }\n").unwrap();
    std::fs::write(root.join("production.rs"), include_str!("../build.rs")).unwrap();
    std::fs::write(root.join("build.rs"), r#"
mod production {
    include!("production.rs");
    pub fn run() { main(); }
}
fn main() {
    use std::io::Write;
    let path = std::env::var_os("TEST_BUILD_COUNTER").unwrap();
    std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(b"x").unwrap();
    production::run();
}
"#).unwrap();
    git(&root, &["init", "--initial-branch=main"]);
    if matches!(topology, Topology::Unborn) {
        return root;
    }
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "first"]);
    if matches!(topology, Topology::Detached) {
        git(&root, &["checkout", "--detach"]);
    }
    if matches!(topology, Topology::Packed) {
        git(&root, &["pack-refs", "--all", "--prune"]);
    }
    if matches!(topology, Topology::Linked) {
        let linked = container.join("linked");
        git(
            &root,
            &["worktree", "add", "-b", "linked", linked.to_str().unwrap()],
        );
        return linked;
    }
    root
}

fn stable_then_updated(topology: Topology) {
    let temporary = tempfile::tempdir().unwrap();
    let root = fixture(temporary.path(), &topology);
    let initial = run(&root);
    assert_eq!(count(&root), 1);
    assert_eq!(run(&root), initial);
    assert_eq!(
        count(&root),
        1,
        "unchanged Git metadata must reuse the build script output"
    );
    std::fs::write(root.join("marker"), "second\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "second"]);
    let expected = git(&root, &["rev-parse", "--short", "HEAD"]);
    assert_ne!(expected, initial);
    assert_eq!(run(&root), expected);
    assert_eq!(
        count(&root),
        2,
        "moving the active branch must refresh commit metadata"
    );
    assert_eq!(run(&root), expected);
    assert_eq!(
        count(&root),
        2,
        "the updated reference must become stable again"
    );
    if matches!(topology, Topology::Loose) {
        git(&root, &["pack-refs", "--all", "--prune"]);
        assert_eq!(run(&root), expected);
        let after_packing = count(&root);
        assert_eq!(run(&root), expected);
        assert_eq!(
            count(&root),
            after_packing,
            "packing refs must not cause perpetual rebuilds"
        );
    }
}

#[test]
fn loose_git_refs_reuse_build_metadata_until_the_commit_changes() {
    stable_then_updated(Topology::Loose);
}

#[test]
fn detached_checkout_reuses_build_metadata_until_head_changes() {
    stable_then_updated(Topology::Detached);
}

#[test]
fn packed_git_refs_reuse_build_metadata_until_the_commit_changes() {
    stable_then_updated(Topology::Packed);
}

#[test]
fn linked_worktree_refs_reuse_build_metadata_until_the_commit_changes() {
    stable_then_updated(Topology::Linked);
}

#[test]
fn unborn_branch_refs_reuse_metadata_then_detect_the_first_commit() {
    stable_then_updated(Topology::Unborn);
}
