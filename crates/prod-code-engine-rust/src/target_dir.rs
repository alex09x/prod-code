/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Whether Cargo would build a workspace into a directory outside it. Each mirrored worktree has
//! its own copy and must have its own target too (#4, #21); a `CARGO_TARGET_DIR` the gateway
//! inherited, or an absolute `build.target-dir` in a Cargo config, would make every copy share
//! one, and analyzer build scripts and commands of different worktrees contend for it (#1446).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The target directory Cargo would use for `root` when it lies outside `root`, from the
/// environment this process inherited or a Cargo config file; `None` when Cargo's default, or a
/// directory inside `root`, applies.
pub fn shared_target_dir(root: &Path) -> Option<PathBuf> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")));
    shared_target_dir_with(
        root,
        std::env::var_os("CARGO_TARGET_DIR"),
        cargo_home.as_deref(),
    )
}

/// [`shared_target_dir`] with the environment given: `CARGO_TARGET_DIR` wins over any config,
/// and the config nearest `root` (its own `.cargo`, then each parent's, then `cargo_home`'s)
/// over the ones after it, as Cargo merges them.
pub fn shared_target_dir_with(
    root: &Path,
    env_target_dir: Option<OsString>,
    cargo_home: Option<&Path>,
) -> Option<PathBuf> {
    let outside = |dir: PathBuf| (!dir.starts_with(root)).then_some(dir);
    if let Some(dir) = env_target_dir.filter(|d| !d.is_empty()) {
        let dir = PathBuf::from(dir);
        // A relative one is relative to where the command runs, inside the workspace.
        return if dir.is_absolute() {
            outside(dir)
        } else {
            None
        };
    }
    let dirs = root
        .ancestors()
        .map(|dir| dir.join(".cargo"))
        .chain(cargo_home.map(Path::to_path_buf));
    for cargo_dir in dirs {
        for name in ["config.toml", "config"] {
            let Ok(text) = std::fs::read_to_string(cargo_dir.join(name)) else {
                continue;
            };
            let Some(dir) = configured_target_dir(&text) else {
                continue;
            };
            let dir = PathBuf::from(dir);
            // A relative `build.target-dir` is relative to the directory holding `.cargo`.
            let base = cargo_dir.parent().unwrap_or(&cargo_dir);
            return outside(if dir.is_absolute() {
                dir
            } else {
                base.join(dir)
            });
        }
    }
    None
}

fn configured_target_dir(text: &str) -> Option<String> {
    let value: toml::Table = toml::from_str(text).ok()?;
    value
        .get("build")?
        .get("target-dir")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_outside_the_workspace_is_found_from_the_environment_or_a_config() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("ws");
        let home = temp.path().join("cargo-home");
        std::fs::create_dir_all(root.join(".cargo")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let shared = temp.path().join("shared-target");

        assert_eq!(shared_target_dir_with(&root, None, Some(&home)), None);
        assert_eq!(
            shared_target_dir_with(&root, Some(shared.clone().into()), Some(&home)),
            Some(shared.clone())
        );
        assert_eq!(
            shared_target_dir_with(&root, Some(root.join("target").into()), Some(&home)),
            None
        );
        assert_eq!(
            shared_target_dir_with(&root, Some("tgt".into()), None),
            None
        );

        // The node's Cargo home names one shared directory.
        let config = format!("[build]\ntarget-dir = {:?}\n", shared.to_string_lossy());
        std::fs::write(home.join("config.toml"), &config).unwrap();
        assert_eq!(
            shared_target_dir_with(&root, None, Some(&home)),
            Some(shared.clone())
        );
        // The workspace's own config is nearer and wins; a relative one stays inside.
        std::fs::write(
            root.join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"build/out\"\n",
        )
        .unwrap();
        assert_eq!(shared_target_dir_with(&root, None, Some(&home)), None);
        // A synced config with an absolute directory outside it.
        std::fs::write(root.join(".cargo/config.toml"), &config).unwrap();
        assert_eq!(
            shared_target_dir_with(&root, None, None),
            Some(shared.clone())
        );
    }
}
