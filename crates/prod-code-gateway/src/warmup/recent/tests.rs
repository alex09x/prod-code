/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{DEBOUNCE_MS, FILE, MAX_KEPT, RecentWorkspaces};
use std::path::Path;
use std::time::Duration;

const DAY: Duration = Duration::from_secs(86_400);

#[test]
fn what_was_used_is_there_after_a_restart_newest_first() {
    let storage = tempfile::tempdir().unwrap();
    let recent = RecentWorkspaces::open(storage.path());
    recent.touch_at(Path::new("/w/a"), "rust", 1_000);
    recent.touch_at(Path::new("/w/b"), "swift", 2_000);
    recent.touch_at(Path::new("/w/c"), "go", 3_000);

    let again = RecentWorkspaces::open(storage.path());
    let roots: Vec<_> = again
        .newest(10, DAY, 4_000)
        .into_iter()
        .map(|r| (r.root.display().to_string(), r.engine))
        .collect();
    assert_eq!(
        roots,
        [
            ("/w/c".to_string(), "go".to_string()),
            ("/w/b".to_string(), "swift".to_string()),
            ("/w/a".to_string(), "rust".to_string()),
        ]
    );
}

#[test]
fn a_workspace_used_again_moves_up_but_not_more_often_than_the_debounce() {
    let storage = tempfile::tempdir().unwrap();
    let recent = RecentWorkspaces::open(storage.path());
    recent.touch_at(Path::new("/w/a"), "rust", 1_000);
    recent.touch_at(Path::new("/w/b"), "rust", 2_000);
    // Within the debounce: nothing changes, nothing is written.
    recent.touch_at(Path::new("/w/a"), "rust", 1_000 + DEBOUNCE_MS - 1);
    let order: Vec<_> = recent
        .newest(10, DAY, 3_000)
        .into_iter()
        .map(|r| r.root.display().to_string())
        .collect();
    assert_eq!(order, ["/w/b", "/w/a"]);
    // After it: a moves to the front with the new time.
    recent.touch_at(Path::new("/w/a"), "rust", 1_000 + DEBOUNCE_MS);
    let first = &recent.newest(10, DAY, 1_000 + DEBOUNCE_MS)[0];
    assert_eq!(first.root, Path::new("/w/a"));
    assert_eq!(first.last_used_ms, 1_000 + DEBOUNCE_MS);
}

#[test]
fn the_same_root_with_another_engine_is_another_entry() {
    let storage = tempfile::tempdir().unwrap();
    let recent = RecentWorkspaces::open(storage.path());
    recent.touch_at(Path::new("/w/a"), "rust", 1_000);
    recent.touch_at(Path::new("/w/a"), "python", 2_000);
    assert_eq!(recent.newest(10, DAY, 3_000).len(), 2);
}

#[test]
fn only_the_recent_ones_within_the_age_and_the_count_are_asked_for() {
    let storage = tempfile::tempdir().unwrap();
    let recent = RecentWorkspaces::open(storage.path());
    recent.touch_at(Path::new("/w/old"), "rust", 1_000);
    recent.touch_at(Path::new("/w/new1"), "rust", 90_000_000);
    recent.touch_at(Path::new("/w/new2"), "rust", 90_100_000);
    let now = 90_200_000;
    let names = |count| -> Vec<String> {
        recent
            .newest(count, DAY, now)
            .into_iter()
            .map(|r| r.root.display().to_string())
            .collect()
    };
    assert_eq!(
        names(10),
        ["/w/new2", "/w/new1"],
        "the old one is over a day old"
    );
    assert_eq!(names(1), ["/w/new2"]);
}

#[test]
fn an_unreadable_record_is_an_empty_one_and_the_next_write_replaces_it() {
    let storage = tempfile::tempdir().unwrap();
    std::fs::write(storage.path().join(FILE), "{ not json").unwrap();
    let recent = RecentWorkspaces::open(storage.path());
    assert!(recent.newest(10, DAY, 1).is_empty());
    recent.touch_at(Path::new("/w/a"), "rust", 5);
    let again = RecentWorkspaces::open(storage.path());
    assert_eq!(again.newest(10, DAY, 10).len(), 1);
}

#[test]
fn the_record_keeps_a_bounded_number() {
    let storage = tempfile::tempdir().unwrap();
    let recent = RecentWorkspaces::open(storage.path());
    for i in 0..(MAX_KEPT as u64 + 10) {
        recent.touch_at(Path::new(&format!("/w/{i}")), "rust", 1_000 + i);
    }
    let kept = recent.newest(1_000, DAY, 2_000);
    assert_eq!(kept.len(), MAX_KEPT);
    assert_eq!(kept[0].root, Path::new(&format!("/w/{}", MAX_KEPT + 9)));
}
