/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The toolchain a project names wins over the one the gateway runs with (#2042).
//!
//! `RUSTUP_TOOLCHAIN` outranks `rust-toolchain.toml` in rustup's order. A gateway started with
//! it (a service unit's `Environment=`) passed it to every command it ran, so a project that
//! asks for `1.100.0` was built with the node's `1.97.1` and refused by its `rust-version`.
//! Whatever toolchain the node runs, a project that names its own is built with that one; a
//! `RUSTUP_TOOLCHAIN` the request sets itself (`env`, `[rust] toolchain`) is still kept.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

const VARIABLE: &str = "RUSTUP_TOOLCHAIN";

/// Whether `rust-toolchain.toml` or `rust-toolchain` is in `run_dir` or a directory above it,
/// up to and including `workspace`.
fn names_toolchain(run_dir: &Path, workspace: &Path) -> bool {
    run_dir
        .ancestors()
        .take_while(|dir| dir.starts_with(workspace))
        .any(|dir| {
            dir.join("rust-toolchain.toml").is_file() || dir.join("rust-toolchain").is_file()
        })
}

/// Whether the inherited `RUSTUP_TOOLCHAIN` must be taken out of the command's environment.
fn drops_inherited(
    run_dir: &Path,
    workspace: &Path,
    request_env: &[(String, String)],
    inherited: Option<&OsStr>,
) -> bool {
    inherited.is_some()
        && !request_env.iter().any(|(key, _)| key == VARIABLE)
        && names_toolchain(run_dir, workspace)
}

/// Leaves the toolchain to the project's `rust-toolchain.toml` when it has one.
pub(crate) fn defer_to_project_toolchain(
    cmd: &mut Command,
    run_dir: &Path,
    workspace: &Path,
    request_env: &[(String, String)],
) {
    let inherited = std::env::var_os(VARIABLE);
    if drops_inherited(run_dir, workspace, request_env, inherited.as_deref()) {
        cmd.env_remove(VARIABLE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout(files: &[&str]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("crates/app")).unwrap();
        for file in files {
            std::fs::write(
                temp.path().join(file),
                "[toolchain]\nchannel = \"1.100.0\"\n",
            )
            .unwrap();
        }
        temp
    }

    /// #2042: the gateway's `RUSTUP_TOOLCHAIN=1.97.1` overrode the project's `1.100.0`.
    #[test]
    fn a_project_that_names_its_toolchain_does_not_get_the_gateways() {
        let temp = checkout(&["rust-toolchain.toml"]);
        let root = temp.path();
        let pinned = Some(OsStr::new("1.97.1"));
        assert!(drops_inherited(root, root, &[], pinned));
        assert!(drops_inherited(&root.join("crates/app"), root, &[], pinned));

        let legacy = checkout(&["rust-toolchain"]);
        assert!(drops_inherited(legacy.path(), legacy.path(), &[], pinned));
    }

    /// The request's own toolchain is kept, a project without a toolchain file keeps the node's,
    /// and nothing is removed when the gateway has none.
    #[test]
    fn the_requests_toolchain_and_a_project_without_one_are_left_alone() {
        let temp = checkout(&["rust-toolchain.toml"]);
        let root = temp.path();
        let pinned = Some(OsStr::new("1.97.1"));
        let asked = [(VARIABLE.to_string(), "nightly".to_string())];
        assert!(!drops_inherited(root, root, &asked, pinned));
        assert!(!drops_inherited(root, root, &[], None));

        let bare = checkout(&[]);
        assert!(!drops_inherited(bare.path(), bare.path(), &[], pinned));
        // A toolchain file above the workspace is not the project's.
        let nested = temp.path().join("crates/app");
        assert!(!drops_inherited(&nested, &nested, &[], pinned));
    }
}
