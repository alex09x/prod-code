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
fn coverage_w2_git_status_copy_rename_and_truncated_records() {
    for (input, expected) in [
        (
            &b"C100\0source.rs\0copy.rs\0D\0gone.rs\0"[..],
            vec![("copy.rs", false), ("gone.rs", true)],
        ),
        (
            &b"R100\0old.rs\0new.rs\0"[..],
            vec![("new.rs", false), ("old.rs", true)],
        ),
        (
            &b"M\0kept.rs\0C100\0source.rs\0"[..],
            vec![("kept.rs", false)],
        ),
        (&b"R100\0old.rs\0"[..], vec![]),
        (&b"M\0"[..], vec![]),
    ] {
        let mut paths = BTreeMap::new();
        parse_name_status(input, &mut paths);
        assert_eq!(
            paths,
            expected.into_iter().map(|(p, d)| (p.into(), d)).collect()
        );
    }
    for (input, expected) in [
        (
            &b"x\0C  copy.rs\0source.rs\0?? new file.rs\0"[..],
            vec![("copy.rs", false), ("new file.rs", false)],
        ),
        (
            &b" R new.rs\0old.rs\0 D gone.rs\0"[..],
            vec![("gone.rs", true), ("new.rs", false), ("old.rs", true)],
        ),
        (&b"R  new.rs\0"[..], vec![]),
        (&b"C  copy.rs\0"[..], vec![]),
    ] {
        let mut paths = BTreeMap::new();
        parse_porcelain_status(input, &mut paths);
        assert_eq!(
            paths,
            expected.into_iter().map(|(p, d)| (p.into(), d)).collect()
        );
    }
}

#[test]
fn coverage_w2_git_queries_report_non_checkout_and_outside_paths() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        git_head(dir.path()).unwrap_err().to_string(),
        "git rev-parse HEAD non-zero exit"
    );
    assert_eq!(
        git_output(dir.path(), ["status"]).unwrap_err().to_string(),
        "git command non-zero exit"
    );
    assert!(git_listed_paths(dir.path(), &["file.rs".into()]).is_empty());
    assert_eq!(git_listed_files(dir.path(), dir.path()), None);
    assert_eq!(
        git_listed_files(dir.path(), &dir.path().join("..").join("outside")),
        None
    );
}
