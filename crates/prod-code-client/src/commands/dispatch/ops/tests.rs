/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `sync --pull` and `pull` need the workspace root and a gateway before they pull anything;
//! relative file names are taken from the directory the command runs in, absolute ones as
//! they are.

use crate::commands::dispatch::tests::{answered_locally, refusal_in};
use std::path::Path;

#[test]
fn pulling_needs_a_workspace_root() {
    for args in [vec!["pull", "src/a.rs"], vec!["sync", "--pull", "src/a.rs"]] {
        assert_eq!(
            refusal_in(None, &args),
            "Failed to resolve workspace root",
            "{args:?}"
        );
    }
}

#[test]
fn relative_and_absolute_files_reach_the_pull_and_stop_at_the_unreachable_gateway() {
    let root: &'static Path = Box::leak(std::env::temp_dir().into_boxed_path());
    let absolute = root.join("abs.rs").to_string_lossy().into_owned();
    for args in [
        vec!["pull", "src/a.rs", absolute.as_str()],
        vec!["sync", "--pull", "src/a.rs"],
        vec!["sync", "--pull", absolute.as_str()],
    ] {
        let message = refusal_in(Some(root), &args);
        assert!(
            message.starts_with(
                "cannot pull from node 127.0.0.1:9; use the node printed by prod-code exec (--remote NODE)"
            ),
            "{args:?}: {message}"
        );
    }
}

#[test]
fn expression_proposals_and_the_cluster_report_are_answered_from_the_checkout() {
    let file = std::env::current_dir()
        .expect("cwd")
        .join("src/commands/common.rs");
    let file = file.to_string_lossy().into_owned();
    answered_locally(&["propose-expression", &file, "20", "String", "--json"])
        .expect("the proposal is answered locally");
    let message = refusal_in(
        None,
        &[
            "propose-expression",
            "missing_scratch_file.rs",
            "1",
            "String",
            "--json",
        ],
    );
    assert!(message.starts_with("Failed to read file "), "{message}");
    assert!(
        message.ends_with("/missing_scratch_file.rs: No such file or directory (os error 2)"),
        "{message}"
    );
    answered_locally(&["cluster", "--json"]).expect("a report of no nodes");
}

#[test]
#[should_panic(expected = "handled before placement")]
fn status_never_reaches_the_ops_dispatcher() {
    let _ = answered_locally(&["status"]);
}

#[test]
#[should_panic(expected = "handled before placement")]
fn report_issue_never_reaches_the_ops_dispatcher() {
    let _ = answered_locally(&["report-issue", "--title", "scratch", "--body", "scratch"]);
}

#[test]
fn an_internal_domain_is_resolved_without_a_lookup() {
    answered_locally(&["resolve", "shop.code.internal", "--json"])
        .expect("an internal domain resolves from the cluster view alone");
}
