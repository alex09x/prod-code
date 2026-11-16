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

use prod_code_protocol::FileDelta;

pub(crate) const VARIABLE: &str = "RUSTUP_TOOLCHAIN";
const FILES: [&str; 2] = ["rust-toolchain.toml", "rust-toolchain"];

/// Whether `rust-toolchain.toml` or `rust-toolchain` is in `run_dir` or a directory above it,
/// up to and including `workspace`. `changes` (a shadow hypothesis's files, not yet on disk)
/// are laid over the workspace: one of them adds or deletes a toolchain file.
fn names_toolchain(run_dir: &Path, workspace: &Path, changes: &[FileDelta]) -> bool {
    run_dir
        .ancestors()
        .take_while(|dir| dir.starts_with(workspace))
        .flat_map(|dir| FILES.map(|name| dir.join(name)))
        .any(|path| {
            match changes
                .iter()
                .rev()
                .find(|change| workspace.join(&change.relative_path) == path)
            {
                Some(change) => change.content.is_some(),
                None => path.is_file(),
            }
        })
}

/// Whether the inherited `RUSTUP_TOOLCHAIN` must be taken out of the command's environment.
fn drops_inherited(
    project_names_one: bool,
    request_env: &[(String, String)],
    inherited: Option<&OsStr>,
) -> bool {
    inherited.is_some() && !request_env.iter().any(|(key, _)| key == VARIABLE) && project_names_one
}

/// Whether a command run in `run_dir` must not get the gateway's own `RUSTUP_TOOLCHAIN`: the
/// gateway has one, the request sets none, and the project names its toolchain. The caller
/// then removes [`VARIABLE`] from the command.
pub(crate) fn yields_to_project(
    run_dir: &Path,
    workspace: &Path,
    changes: &[FileDelta],
    request_env: &[(String, String)],
) -> bool {
    let inherited = std::env::var_os(VARIABLE);
    // The files are read only when the gateway has a toolchain of its own to withhold.
    inherited.is_some()
        && drops_inherited(
            names_toolchain(run_dir, workspace, changes),
            request_env,
            inherited.as_deref(),
        )
}

/// Leaves the toolchain to the project's `rust-toolchain.toml` when it has one.
pub(crate) fn defer_to_project_toolchain(
    cmd: &mut Command,
    run_dir: &Path,
    workspace: &Path,
    request_env: &[(String, String)],
) {
    if yields_to_project(run_dir, workspace, &[], request_env) {
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

    fn change(path: &str, content: Option<&str>) -> FileDelta {
        FileDelta {
            relative_path: path.to_string(),
            content: content.map(|text| text.as_bytes().to_vec()),
            is_executable: false,
        }
    }

    /// #2042: the gateway's `RUSTUP_TOOLCHAIN=1.97.1` overrode the project's `1.100.0`.
    #[test]
    fn a_project_that_names_its_toolchain_does_not_get_the_gateways() {
        let temp = checkout(&["rust-toolchain.toml"]);
        let root = temp.path();
        assert!(names_toolchain(root, root, &[]));
        assert!(names_toolchain(&root.join("crates/app"), root, &[]));
        let legacy = checkout(&["rust-toolchain"]);
        assert!(names_toolchain(legacy.path(), legacy.path(), &[]));

        let pinned = Some(OsStr::new("1.97.1"));
        assert!(drops_inherited(true, &[], pinned));
    }

    /// The request's own toolchain is kept, a project without a toolchain file keeps the node's,
    /// and nothing is removed when the gateway has none.
    #[test]
    fn the_requests_toolchain_and_a_project_without_one_are_left_alone() {
        let pinned = Some(OsStr::new("1.97.1"));
        let asked = [(VARIABLE.to_string(), "nightly".to_string())];
        assert!(!drops_inherited(true, &asked, pinned));
        assert!(!drops_inherited(true, &[], None));
        assert!(!drops_inherited(false, &[], pinned));

        let bare = checkout(&[]);
        assert!(!names_toolchain(bare.path(), bare.path(), &[]));
        // A toolchain file above the workspace is not the project's.
        let temp = checkout(&["rust-toolchain.toml"]);
        let nested = temp.path().join("crates/app");
        assert!(!names_toolchain(&nested, &nested, &[]));
    }

    /// A shadow hypothesis's files are not on disk: one that adds a toolchain file names one,
    /// one that deletes it does not.
    #[test]
    fn a_hypothesis_that_adds_or_deletes_the_toolchain_file_decides() {
        let bare = checkout(&[]);
        let root = bare.path();
        let added = [change("rust-toolchain.toml", Some("[toolchain]\n"))];
        assert!(names_toolchain(&root.join("crates/app"), root, &added));
        assert!(!names_toolchain(
            root,
            root,
            &[change("crates/app/rust-toolchain", Some("1.100.0\n"))]
        ));

        let named = checkout(&["rust-toolchain.toml"]);
        let deleted = [change("rust-toolchain.toml", None)];
        assert!(!names_toolchain(named.path(), named.path(), &deleted));
    }
}
