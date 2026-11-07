/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{Ahead, Outcome, Skip, restore};
use crate::warmup::{LanguagePolicy, RecentWorkspaces, WarmupConfig, When};
use std::path::PathBuf;
use std::sync::Mutex;

const NOW: u64 = 10_000_000;

struct Fixture {
    _storage: tempfile::TempDir,
    checkouts: tempfile::TempDir,
    recent: RecentWorkspaces,
}

impl Fixture {
    /// A record whose workspaces `a`, `b`, `c` (oldest first) exist on disk.
    fn new() -> Self {
        Self::with(&[("a", "rust"), ("b", "swift"), ("c", "rust")])
    }

    /// A record of the given `(root name, engine)` uses, oldest first, the roots on disk.
    fn with(uses: &[(&str, &str)]) -> Self {
        let storage = tempfile::tempdir().unwrap();
        let checkouts = tempfile::tempdir().unwrap();
        let recent = RecentWorkspaces::open(storage.path());
        for (i, (name, engine)) in uses.iter().enumerate() {
            let root = checkouts.path().join(name);
            std::fs::create_dir_all(&root).unwrap();
            recent.touch_at(&root, engine, NOW - 3_000 + i as u64 * 1_000);
        }
        Self {
            _storage: storage,
            checkouts,
            recent,
        }
    }

    fn root(&self, name: &str) -> PathBuf {
        self.checkouts.path().join(name)
    }
}

async fn run(
    fixture: &Fixture,
    config: &WarmupConfig,
    busy: impl Fn() -> bool,
    failing: &[&str],
    present: &[&str],
) -> (Vec<String>, Vec<(String, Outcome)>) {
    let loaded = Mutex::new(Vec::new());
    let outcomes = Mutex::new(Vec::new());
    restore(
        config,
        &fixture.recent,
        NOW,
        busy,
        |entry| {
            let name = entry
                .root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
            let fail = failing.contains(&name.as_str());
            let there = present.contains(&name.as_str());
            loaded
                .lock()
                .unwrap()
                .push(format!("{name}/{}", entry.engine));
            async move {
                if fail {
                    anyhow::bail!("no memory for {name}")
                }
                Ok(if there { Ahead::Present } else { Ahead::Loaded })
            }
        },
        |entry, outcome| {
            let name = entry
                .root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
            outcomes.lock().unwrap().push((name, outcome));
        },
    )
    .await;
    (loaded.into_inner().unwrap(), outcomes.into_inner().unwrap())
}

#[tokio::test]
async fn the_newest_are_loaded_first_one_at_a_time_and_no_more_than_asked() {
    let fixture = Fixture::new();
    let config = WarmupConfig {
        max_workspaces: 2,
        ..WarmupConfig::default()
    };
    let (loaded, outcomes) = run(&fixture, &config, || false, &[], &[]).await;
    assert_eq!(loaded, ["c/rust", "b/swift"]);
    assert!(
        outcomes
            .iter()
            .all(|(_, outcome)| matches!(outcome, Outcome::Loaded(_)))
    );
}

#[tokio::test]
async fn a_language_set_to_never_and_a_missing_checkout_are_skipped_with_the_reason() {
    let fixture = Fixture::new();
    std::fs::remove_dir(fixture.root("c")).unwrap();
    let mut config = WarmupConfig::default();
    config.languages.insert(
        "swift".to_string(),
        LanguagePolicy {
            when: When::Never,
            ..LanguagePolicy::default()
        },
    );
    let (loaded, outcomes) = run(&fixture, &config, || false, &[], &[]).await;
    assert_eq!(loaded, ["a/rust"]);
    let reason = |name: &str| {
        outcomes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, o)| match o {
                Outcome::Skipped(skip) => skip.as_str().to_string(),
                other => format!("{other:?}"),
            })
            .unwrap()
    };
    assert_eq!(reason("c"), "skipped_missing");
    assert_eq!(reason("b"), "skipped_language");
}

#[tokio::test]
async fn a_busy_node_loads_nothing_and_says_so_for_each() {
    let fixture = Fixture::new();
    let (loaded, outcomes) = run(&fixture, &WarmupConfig::default(), || true, &[], &[]).await;
    assert!(loaded.is_empty());
    assert_eq!(outcomes.len(), 3);
    assert!(
        outcomes
            .iter()
            .all(|(_, o)| *o == Outcome::Skipped(Skip::Busy))
    );
}

#[tokio::test]
async fn a_node_that_becomes_busy_stops_loading_and_a_failed_load_does_not_stop_the_rest() {
    let fixture = Fixture::new();
    let asked = Mutex::new(0);
    // Not busy for the first two questions, busy from the third.
    let busy = || {
        let mut asked = asked.lock().unwrap();
        *asked += 1;
        *asked > 2
    };
    let (loaded, outcomes) = run(&fixture, &WarmupConfig::default(), busy, &["c"], &[]).await;
    assert_eq!(loaded, ["c/rust", "b/swift"]);
    let find = |name: &str| {
        outcomes
            .iter()
            .find(|(n, _)| n == name)
            .unwrap()
            .1
            .clone_kind()
    };
    assert_eq!(find("c"), "failed: no memory for c");
    assert_eq!(find("b"), "loaded");
    assert_eq!(find("a"), "skipped_busy");
}

#[tokio::test]
async fn restoring_can_be_turned_off() {
    let fixture = Fixture::new();
    let config = WarmupConfig {
        after_restart: false,
        ..WarmupConfig::default()
    };
    let (loaded, outcomes) = run(&fixture, &config, || false, &[], &[]).await;
    assert!(loaded.is_empty() && outcomes.is_empty());
}

/// One checkout is kept once, whatever its engine, and a load of a second engine replaces the
/// first: so only its newest engine is loaded, and it takes one slot, not two.
#[tokio::test]
async fn a_checkout_used_with_two_engines_is_loaded_once_with_the_newest() {
    // x: python, then rust (newer); y: go (newest of all).
    let fixture = Fixture::with(&[("x", "python"), ("x", "rust"), ("y", "go")]);
    let config = WarmupConfig {
        max_workspaces: 2,
        ..WarmupConfig::default()
    };
    let (loaded, outcomes) = run(&fixture, &config, || false, &[], &[]).await;
    assert_eq!(loaded, ["y/go", "x/rust"], "x once, and y has its slot");
    assert_eq!(outcomes.len(), 2, "the older engine of x is not reported");
}

/// An engine set to `never` does not hide the older, loadable engine of the same checkout.
#[tokio::test]
async fn an_engine_that_is_never_loaded_leaves_the_older_engine_of_its_checkout() {
    let fixture = Fixture::with(&[("x", "python"), ("x", "swift")]);
    let mut config = WarmupConfig::default();
    config.languages.insert(
        "swift".to_string(),
        LanguagePolicy {
            when: When::Never,
            ..LanguagePolicy::default()
        },
    );
    let (loaded, outcomes) = run(&fixture, &config, || false, &[], &[]).await;
    assert_eq!(loaded, ["x/python"]);
    let kinds: Vec<_> = outcomes.iter().map(|(_, o)| o.clone_kind()).collect();
    assert!(kinds.contains(&"skipped_language".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"loaded".to_string()), "{kinds:?}");
}

/// A workspace that is there already (a client loaded it first) is left as it is, and is
/// reported as such, not as a load.
#[tokio::test]
async fn a_workspace_that_is_there_already_is_reported_not_loaded() {
    let fixture = Fixture::new();
    let (_, outcomes) = run(&fixture, &WarmupConfig::default(), || false, &[], &["c"]).await;
    assert_eq!(outcomes.len(), 3);
    let kind = |name: &str| {
        outcomes
            .iter()
            .find(|(n, _)| n == name)
            .unwrap()
            .1
            .clone_kind()
    };
    assert_eq!(kind("c"), "skipped_present");
    assert_eq!(kind("b"), "loaded");
}

impl Outcome {
    /// A comparable name for an outcome, without its duration.
    fn clone_kind(&self) -> String {
        match self {
            Outcome::Loaded(_) => "loaded".to_string(),
            Outcome::Skipped(skip) => skip.as_str().to_string(),
            Outcome::Failed(error) => format!("failed: {error}"),
        }
    }
}
