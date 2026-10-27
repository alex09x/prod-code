/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn coverage_b2_history_tracks_only_the_written_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    assert_eq!(text_before_apply(&path), "");
    std::fs::write(&path, b"new").unwrap();
    remember_applied(
        dir.path(),
        &[
            ("file".into(), Some(b"old".to_vec())),
            ("missing".into(), None),
        ],
    );
    assert_eq!(text_before_apply(&path), "old");
    std::fs::write(&path, b"later").unwrap();
    assert_eq!(text_before_apply(&path), "later");
    remember_applied_multi(&[(path.clone(), None)]);
    assert_eq!(text_before_apply(&path), "");
    remember_applied_multi(&[(path.clone(), Some(vec![0xff]))]);
    assert_eq!(text_before_apply(&path), "�");
    std::fs::remove_file(&path).unwrap();
    assert_eq!(text_before_apply(&path), "");
}

/// After an applied edit, its own report still diffs against the old text, but a later
/// proposal on the unchanged result diffs against what is on disk: the applied change is not
/// shown again as part of it (#1470).
#[test]
fn a_later_proposal_is_diffed_against_the_file_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.rs");
    std::fs::write(&path, b"fn renamed() {}\n").unwrap();
    remember_applied(
        dir.path(),
        &[("lib.rs".into(), Some(b"fn original() {}\n".to_vec()))],
    );
    assert_eq!(
        text_before_report(&path, true),
        "fn original() {}\n",
        "the applying report"
    );
    assert_eq!(
        text_before_report(&path, true),
        "fn original() {}\n",
        "rendered again, the same"
    );
    assert_eq!(
        text_before_report(&path, false),
        "fn renamed() {}\n",
        "a later dry run"
    );
}
