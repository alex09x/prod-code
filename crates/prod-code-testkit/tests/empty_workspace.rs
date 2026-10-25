/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::Workspace;
use std::process::Command;

#[test]
fn empty_workspaces_have_a_committed_empty_tree() {
    for workspace in [Workspace::empty(), Workspace::new(&[])] {
        let head = Command::new("git")
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(workspace.root())
            .output()
            .unwrap();
        assert!(
            head.status.success(),
            "empty fixture must have an initial HEAD"
        );
        let tree = Command::new("git")
            .args(["ls-tree", "--name-only", "HEAD"])
            .current_dir(workspace.root())
            .output()
            .unwrap();
        assert!(tree.status.success());
        assert!(tree.stdout.is_empty(), "empty fixture must commit no files");
    }
}
