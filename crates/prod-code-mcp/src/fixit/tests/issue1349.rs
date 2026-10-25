/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Protocol tests: command output is scripted, not produced by a real linter.

mod boundary;
mod gateway;
mod tools;

use super::{UNUSED, parse_fixes, plan};
use crate::fixit::check_and_fix;
use crate::verify::VerifyKind;
use gateway::{Gateway, Step};
use prod_code_testkit::Workspace;

const SOURCE: &str = "use std::fmt;\npub fn sample() {}\n";
const FIXED: &str = "pub fn sample() {}\n";
const MANIFEST: &str = "[package]\nname='fixture'\nversion='0.1.0'\nedition='2024'\n";

#[tokio::test]
async fn issue1349_nested_command_directory_does_not_edit_outer_shadow() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[workspace]\nmembers=[]\nexclude=['nested']\n",
        ),
        ("src/lib.rs", SOURCE),
        ("nested/Cargo.toml", MANIFEST),
        ("nested/src/lib.rs", SOURCE),
    ]);
    let hint = ws.path("nested/src/lib.rs");
    assert_eq!(
        crate::sync::engine_project(&ws.root(), &hint),
        (Some("nested".into()), Some("rust"))
    );
    let gateway = Gateway::start(vec![Step::rust(UNUSED), Step::green()]).await;
    let result = check_and_fix(gateway.addr, &ws.root(), Some(&hint), VerifyKind::Lint, 10).await;
    let requests = gateway.finish().await;
    let result = result.unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(request.subdir.as_deref(), Some("nested"));
        assert_eq!(
            request.command,
            [
                "cargo",
                "clippy",
                "--workspace",
                "--all-targets",
                "--message-format=json",
                "--",
                "-D",
                "warnings"
            ]
        );
    }
    println!(
        "nested protocol: ok={}, outer={:?}, nested={:?}",
        result.ok(),
        ws.read("src/lib.rs"),
        ws.read("nested/src/lib.rs")
    );
    assert_eq!(
        ws.read("src/lib.rs").as_bytes(),
        SOURCE.as_bytes(),
        "outer shadow must stay byte-identical"
    );
    assert_eq!(ws.read("nested/src/lib.rs").as_bytes(), FIXED.as_bytes());
    assert!(result.ok());
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(result.outcomes[0].file, "nested/src/lib.rs");
    assert!(result.outcomes[0].skipped.is_none());
}

#[test]
fn issue1349_root_planner_still_resolves_root_relative_files() {
    let ws = Workspace::new(&[("src/lib.rs", SOURCE), ("nested/src/lib.rs", SOURCE)]);
    let (files, outcomes) = plan(&ws.root(), &parse_fixes(UNUSED));
    assert_eq!(files.len(), 1);
    assert_eq!(files[&ws.path("src/lib.rs")].as_bytes(), FIXED.as_bytes());
    assert!(outcomes[0].skipped.is_none());
    assert_eq!(
        ws.read("src/lib.rs").as_bytes(),
        SOURCE.as_bytes(),
        "planning does not write"
    );
}

#[test]
fn issue1349_planner_separates_command_directory_from_outer_boundary() {
    let ws = Workspace::new(&[("src/lib.rs", SOURCE), ("nested/src/lib.rs", SOURCE)]);
    let fixes = parse_fixes(UNUSED);
    let (nested, outcomes) = crate::fixit::plan::plan_in(&ws.root(), &ws.path("nested"), &fixes);
    assert_eq!(nested.len(), 1);
    assert_eq!(
        nested[&ws.path("nested/src/lib.rs")].as_bytes(),
        FIXED.as_bytes()
    );
    assert_eq!(outcomes[0].file, "nested/src/lib.rs");
    let mut parent = fixes[0].clone();
    parent.edits[0].file = "../src/lib.rs".into();
    let (outer, outcomes) = crate::fixit::plan::plan_in(&ws.root(), &ws.path("nested"), &[parent]);
    assert_eq!(outer[&ws.path("src/lib.rs")].as_bytes(), FIXED.as_bytes());
    assert!(
        outcomes[0].skipped.is_none(),
        "parent components legitimately within outer root are allowed"
    );
    assert_eq!(ws.read("src/lib.rs").as_bytes(), SOURCE.as_bytes());
    assert_eq!(ws.read("nested/src/lib.rs").as_bytes(), SOURCE.as_bytes());
}
