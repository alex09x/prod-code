/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{cargo_members, package, routed, write};

#[test]
fn project_root_regression_review_exclude_is_a_literal_prefix() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = '2'\nmembers = ['crates/*']\nexclude = ['./crates/foo-*/', './fixtures/']\n",
    );
    package(root, "crates/foo-one", "foo-one", false);
    package(root, "crates/bar", "bar", false);
    package(root, "fixtures/probe", "probe", true);
    cargo_members(root, &["crates/foo-one", "crates/bar"]);
    routed(
        root,
        "fixtures/probe/src/lib.rs",
        Some("fixtures/probe"),
        "rust",
    );
    routed(root, "crates/bar/src/lib.rs", None, "rust");
    routed(root, "crates/foo-one/src/lib.rs", None, "rust");
}

#[test]
fn project_root_regression_review_implicit_path_member() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = 'root'\nversion = '0.1.0'\nedition = '2024'\n[workspace]\nmembers = []\n[dependencies]\nchild = { path = './child' }\n",
    );
    write(root, "src/lib.rs", "pub fn root() {}\n");
    package(root, "child", "child", false);
    package(root, "unrelated", "unrelated", true);
    cargo_members(root, &["", "child"]);
    routed(root, "unrelated/src/lib.rs", Some("unrelated"), "rust");
    routed(root, "child/src/lib.rs", None, "rust");
}

#[test]
fn project_root_regression_review_transitive_member_dependencies() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = '2'\nmembers = ['crates/app-*']\nexclude = ['independent']\n[workspace.dependencies]\nshared = { path = 'shared' }\n",
    );
    package(root, "crates/app-one", "app-one", false);
    let manifest = "[package]\nname = 'app-one'\nversion = '0.1.0'\nedition = '2024'\n\
        [dependencies]\nshared.workspace = true\nindependent = { path = '../../independent' }\n\
        [target.'cfg(unix)'.build-dependencies]\nbuilder = { path = '../../builder' }\n";
    write(root, "crates/app-one/Cargo.toml", manifest);
    package(root, "shared", "shared", false);
    write(
        root,
        "shared/Cargo.toml",
        "[package]\nname = 'shared'\nversion = '0.1.0'\nedition = '2024'\n[dev-dependencies]\nhelper = { path = '../helper' }\n",
    );
    package(root, "helper", "helper", false);
    package(root, "builder", "builder", false);
    package(root, "independent", "independent", false);
    package(root, "shared/nested", "nested", true);
    cargo_members(root, &["crates/app-one", "shared", "helper", "builder"]);
    routed(root, "independent/src/lib.rs", Some("independent"), "rust");
    routed(
        root,
        "shared/nested/src/lib.rs",
        Some("shared/nested"),
        "rust",
    );
    for member in ["crates/app-one", "shared", "helper", "builder"] {
        routed(root, &format!("{member}/src/lib.rs"), None, "rust");
    }
}
