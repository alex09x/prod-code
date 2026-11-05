/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Files a command created in a workspace's copy on the node, which the client's manifest does
//! not list: a generated framework, a downloaded archive, a script written for the next command.
//! They are the node's, not the checkout's. A manifest probe deletes what the client lacks, so
//! without this list the next command finds them gone (#1895, #1888). The list lives on disk, as
//! the stale files' does, so that a gateway restart does not forget it.

use std::collections::BTreeSet;
use std::path::Path;

/// The file in a workspace's copy listing checkout-relative paths that commands created there.
pub const EXEC_CREATED_MARKER: &str = ".prod-code-exec-created";

/// The paths recorded by [`record_exec_created`], sorted.
pub fn exec_created_paths(workspace_dir: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(workspace_dir.join(EXEC_CREATED_MARKER))
        .map(|text| {
            text.lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The files of the copy now, by checkout-relative path (build output and VCS internals
/// excluded, as everywhere the sync layer walks).
pub fn present_paths(workspace_dir: &Path) -> BTreeSet<String> {
    let mut present = Vec::new();
    crate::walk_files(workspace_dir, workspace_dir, &mut present);
    present.into_iter().map(|(rel, _)| rel).collect()
}

/// Adds the files that exist now and did not in `before` to the workspace's list, and drops the
/// listed files that no longer exist, so the list does not outlive its files.
pub fn record_exec_created(workspace_dir: &Path, before: &BTreeSet<String>) {
    let now = present_paths(workspace_dir);
    let mut all = exec_created_paths(workspace_dir);
    all.extend(now.difference(before).cloned());
    all.retain(|path| now.contains(path));
    write(workspace_dir, &all);
}

fn write(workspace_dir: &Path, paths: &BTreeSet<String>) {
    let marker = workspace_dir.join(EXEC_CREATED_MARKER);
    if paths.is_empty() {
        let _ = std::fs::remove_file(&marker);
        return;
    }
    let mut text = String::new();
    for path in paths {
        text.push_str(path);
        text.push('\n');
    }
    if let Err(e) = std::fs::write(&marker, text) {
        tracing::warn!(error = %e, marker = %marker.display(), "failed to record the files a command created");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_files_a_command_created_are_listed_and_only_while_they_exist() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        let before = present_paths(root);
        assert_eq!(before, BTreeSet::from(["src/lib.rs".to_string()]));

        std::fs::create_dir_all(root.join(".scratch")).unwrap();
        std::fs::write(root.join(".scratch/probe.py"), "").unwrap();
        std::fs::write(root.join("src/lib.rs"), "changed").unwrap();
        record_exec_created(root, &before);
        assert_eq!(
            exec_created_paths(root),
            BTreeSet::from([".scratch/probe.py".to_string()])
        );
        // The marker is not itself one of the copy's files.
        assert!(!present_paths(root).contains(EXEC_CREATED_MARKER));

        // A later command adds to the list; a removed file leaves it.
        std::fs::write(root.join("out.bin"), "").unwrap();
        std::fs::remove_file(root.join(".scratch/probe.py")).unwrap();
        record_exec_created(
            root,
            &present_paths(root)
                .difference(&BTreeSet::from(["out.bin".to_string()]))
                .cloned()
                .collect(),
        );
        assert_eq!(
            exec_created_paths(root),
            BTreeSet::from(["out.bin".to_string()])
        );
    }
}
