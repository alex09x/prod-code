/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::tests::{graph, write};
use super::*;

#[test]
fn discovers_members_and_all_dependency_kinds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "Cargo.toml",
        r#"[package]
name = "root"
[workspace]
members = ["crates/*", "explicit", "missing", "absent/*", 7]
"#,
    );
    write(
        root,
        "crates/alpha/Cargo.toml",
        r#"[package]
name = "alpha"
[dependencies]
beta = "1"
external = "1"
renamed_root = { path = "../.." }
missing_path = { path = "../missing" }
[dev-dependencies]
gamma = { path = "../gamma" }
[build-dependencies]
delta = "1"
"#,
    );
    for name in ["beta", "gamma", "delta"] {
        write(
            root,
            &format!("crates/{name}/Cargo.toml"),
            &format!("[package]\nname = '{name}'\n"),
        );
    }
    write(
        root,
        "explicit/Cargo.toml",
        "[package]\nname = 'explicit'\n",
    );
    write(root, "crates/broken/Cargo.toml", "invalid = [");
    write(
        root,
        "crates/nameless/Cargo.toml",
        "[package]\nversion = '1'\n",
    );
    write(root, "crates/unreadable/Cargo.toml", "unused");
    std::fs::write(root.join("crates/unreadable/Cargo.toml"), [0xff]).unwrap();
    write(root, "crates/not-a-package/README", "no manifest");
    let mut adj = BTreeMap::new();
    collect_cargo_dependencies(root, &mut adj).unwrap();
    assert_eq!(
        adj,
        graph(
            root,
            &[
                ("root", "", &[]),
                ("alpha", "crates/alpha", &["beta", "delta", "gamma", "root"]),
                ("beta", "crates/beta", &[]),
                ("gamma", "crates/gamma", &[]),
                ("delta", "crates/delta", &[]),
                ("explicit", "explicit", &[]),
            ]
        )
    );
}

#[test]
fn missing_empty_and_invalid_root_manifests() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let expected = graph(root, &[("existing", "existing", &["other"])]);
    let mut adj = expected.clone();
    collect_cargo_dependencies(root, &mut adj).unwrap();
    assert_eq!(adj, expected);
    for content in ["", "[workspace]\nmembers = []", "[package]\nname = 4"] {
        write(root, "Cargo.toml", content);
        collect_cargo_dependencies(root, &mut adj).unwrap();
        assert_eq!(adj, expected);
    }
    write(root, "Cargo.toml", "[broken");
    assert_eq!(
        collect_cargo_dependencies(root, &mut adj)
            .unwrap_err()
            .to_string(),
        "Failed to parse root Cargo.toml"
    );
    assert_eq!(adj, expected);
    std::fs::write(root.join("Cargo.toml"), [0xff]).unwrap();
    assert_eq!(
        collect_cargo_dependencies(root, &mut adj)
            .unwrap_err()
            .to_string(),
        format!("Failed to read {}", root.join("Cargo.toml").display())
    );
}

#[test]
fn renamed_path_dependencies_resolve_before_any_member_is_inserted() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = ['member', 'peer']\n",
    );
    write(
        root,
        "member/Cargo.toml",
        "[package]\nname = 'member'\n[dependencies]\nalias = { path = '../peer' }\n",
    );
    write(
        root,
        "peer/Cargo.toml",
        "[package]\nname = 'peer'\n[dev-dependencies]\nalias = { path = '../member' }\n",
    );
    let mut adj = BTreeMap::new();
    collect_cargo_dependencies(root, &mut adj).unwrap();
    assert_eq!(
        adj,
        graph(
            root,
            &[
                ("member", "member", &["peer"]),
                ("peer", "peer", &["member"])
            ]
        )
    );
}

#[test]
fn preserves_preexisting_root_and_resolves_external_path_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = 'root'\n[workspace]\nmembers = ['member']\n",
    );
    write(
        root,
        "member/Cargo.toml",
        "[package]\nname = 'member'\n[dependencies]\nalias = { path = '../external' }\n",
    );
    std::fs::create_dir(root.join("external")).unwrap();
    let mut adj = graph(
        root,
        &[
            ("root", "", &["external"]),
            ("external", "external", &[]),
            ("stale", "absent", &[]),
        ],
    );
    collect_cargo_dependencies(root, &mut adj).unwrap();
    assert_eq!(
        adj,
        graph(
            root,
            &[
                ("root", "", &["external"]),
                ("external", "external", &[]),
                ("stale", "absent", &[]),
                ("member", "member", &["external"])
            ]
        )
    );
}
