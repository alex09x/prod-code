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
    remember_applied(dir.path(), &[("file".into(), Some(b"old".to_vec())), ("missing".into(), None)]);
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
