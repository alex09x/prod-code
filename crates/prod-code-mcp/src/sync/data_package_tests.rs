/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A data directory that is a package goes to the mirror (#2048, #2049).

use super::*;
use crate::sync::scan::scan_workspace_files_with_omitted;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn git(root: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .is_ok_and(|s| s.success())
}

/// #2049: a vendored Go package under a `data/` path segment is source, not data.
#[test]
fn tracked_source_in_a_data_directory_is_synced() {
    assert!(is_synced_git_path(
        "vendor/example.com/mod/data/v3/msg.pb.go"
    ));
    assert!(is_synced_git_path("data/loader/load.py"));
    // Data stays out.
    assert!(!is_synced_git_path(
        "vendor/example.com/mod/data/v3/blob.yaml"
    ));
    assert!(!is_synced_git_path("data/dump.csv"));
    // Output directories are not packages because they hold code.
    assert!(!is_synced_git_path("dist/bundle.js"));
}

/// #2048: `docs/data/backends/*.yaml` are embedded by `embed.go` beside them; a data directory
/// without code keeps its files out, and so does a checkout read without its root.
#[test]
fn the_files_of_a_data_directory_holding_code_are_synced() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    if !git(&root, &["init", "-q"]) {
        return; // no git here
    }
    write(&root, "go.mod", "module example.com/m\n");
    write(&root, "docs/data/backends/embed.go", "package backends\n");
    write(&root, "docs/data/backends/alias.yaml", "name: alias\n");
    write(&root, "docs/data/backends/sub/notes.yaml", "x: 1\n");
    write(&root, "data/dump.yaml", "rows: []\n");
    write(&root, "data/settings.json", "{}\n");
    write(&root, "dist/data/main.js", "x\n");
    write(&root, "dist/data/out.yaml", "x: 1\n");
    assert!(git(&root, &["add", "."]));

    let include = SyncInclude::load(&root);
    assert!(is_synced_git_path_with(
        "docs/data/backends/alias.yaml",
        &include
    ));
    assert!(!is_synced_git_path_with(
        "docs/data/backends/sub/notes.yaml",
        &include
    ));
    assert!(!is_synced_git_path_with("data/dump.yaml", &include));
    assert!(!is_synced_git_path_with("dist/data/out.yaml", &include));
    assert!(!is_synced_git_path_with(
        "docs/data/backends/alias.yaml",
        &SyncInclude::default()
    ));

    let (deltas, omitted) = scan_workspace_files_with_omitted(&root, None).unwrap();
    let synced: Vec<&str> = deltas.iter().map(|d| d.relative_path.as_str()).collect();
    assert!(
        synced.contains(&"docs/data/backends/alias.yaml"),
        "{synced:?}"
    );
    assert!(
        synced.contains(&"docs/data/backends/embed.go"),
        "{synced:?}"
    );
    let left: Vec<&str> = omitted.iter().map(|o| o.path.as_str()).collect();
    assert!(left.contains(&"data/dump.yaml"), "{left:?}");
    assert!(!left.contains(&"docs/data/backends/alias.yaml"), "{left:?}");
}
