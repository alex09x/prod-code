/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::runner::run;
use super::super::session::{extract_hover_text, locate_symbol};
use super::super::setup::{bench_workspace_name, setup};
use super::super::target::{
    go_package_name, is_candidate_source, mutate_signature, parse_signature_line,
};
use super::super::types::{
    DivergentBenchConfig, Expectations, Language, MIN_WORKERS, QueryOutcome, WorkspaceMode,
    WorktreeKind,
};
use super::super::verify::verify;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BASE_SIGNATURE: &str = "pub fn compute_signal(input: i64) -> i64";
const MUTATED_SIGNATURE: &str = "pub fn compute_signal(input: i64, divergent_marker: i64) -> i64";

fn expectations() -> Expectations {
    Expectations {
        symbol: "compute_signal".to_string(),
        marker: Language::Rust.marker().to_string(),
        untracked_symbol: Language::Rust.untracked_symbol().to_string(),
    }
}

fn synthetic(kind: WorktreeKind, ok: bool, detail: &str) -> QueryOutcome {
    QueryOutcome {
        worker_id: 0,
        kind,
        latency: Duration::from_millis(1),
        ok,
        detail: detail.to_string(),
    }
}

fn clean_outcomes() -> Vec<QueryOutcome> {
    vec![
        synthetic(WorktreeKind::Master, true, BASE_SIGNATURE),
        synthetic(WorktreeKind::SignatureChange, true, MUTATED_SIGNATURE),
        synthetic(WorktreeKind::DependencyChange, true, BASE_SIGNATURE),
        synthetic(
            WorktreeKind::UntrackedFile,
            true,
            "pub fn divergent_untracked_symbol() -> &'static str",
        ),
    ]
}

#[test]
fn verify_passes_with_clean_divergence() {
    let results = verify(&clean_outcomes(), &expectations());
    assert!(results.iter().all(|r| r.passed), "{results:?}");
}

#[test]
fn verify_catches_cross_worktree_bleed_into_master() {
    let mut outcomes = clean_outcomes();
    outcomes[0] = synthetic(WorktreeKind::Master, true, MUTATED_SIGNATURE);
    let results = verify(&outcomes, &expectations());
    let master = results
        .iter()
        .find(|r| r.kind == WorktreeKind::Master)
        .unwrap();
    assert!(!master.passed);
    assert_eq!(master.violations, 1);
    assert!(master.sample.contains("divergent_marker"));
}

#[test]
fn verify_catches_bleed_from_master_into_worktree_a() {
    let mut outcomes = clean_outcomes();
    outcomes[1] = synthetic(WorktreeKind::SignatureChange, true, BASE_SIGNATURE);
    let results = verify(&outcomes, &expectations());
    let a = results
        .iter()
        .find(|r| r.kind == WorktreeKind::SignatureChange)
        .unwrap();
    assert!(!a.passed);
}

#[test]
fn verify_catches_untracked_symbol_not_resolved() {
    let mut outcomes = clean_outcomes();
    outcomes[3] = synthetic(WorktreeKind::UntrackedFile, true, "no symbol here");
    let results = verify(&outcomes, &expectations());
    let untracked = results
        .iter()
        .find(|r| r.kind == WorktreeKind::UntrackedFile)
        .unwrap();
    assert!(!untracked.passed);
}

#[test]
fn verify_fails_when_no_successful_responses() {
    let outcomes = vec![synthetic(WorktreeKind::Master, false, "connection refused")];
    let results = verify(&outcomes, &expectations());
    for kind in WorktreeKind::all() {
        let r = results.iter().find(|r| r.kind == kind).unwrap();
        assert!(!r.passed);
    }
    assert!(results[0].sample.contains("connection refused"));
}

#[test]
fn locate_symbol_finds_line_and_column() {
    let content = "line one\npub fn compute_signal(input: i64) -> i64 {\n";
    let (line, col) = locate_symbol(content, "compute_signal").unwrap();
    assert_eq!(line, 1);
    assert_eq!(
        col,
        content
            .lines()
            .nth(1)
            .unwrap()
            .find("compute_signal")
            .unwrap() as u32
    );
}

#[test]
fn locate_symbol_prefers_definition_over_doc_comment() {
    let content = "package main\n\n// DivergentUntrackedSymbol is introduced by the benchmark.\nfunc DivergentUntrackedSymbol() string {\n";
    let (line, col) = locate_symbol(content, "DivergentUntrackedSymbol").unwrap();
    assert_eq!(line, 3);
    assert_eq!(col, 5);
    let rust = "/// compute_signal docs\npub fn compute_signal(input: i64) -> i64 {\n";
    assert_eq!(locate_symbol(rust, "compute_signal").unwrap(), (1, 7));
}

#[test]
fn locate_symbol_missing_errors() {
    assert!(locate_symbol("nothing to see here", "compute_signal").is_err());
}

#[test]
fn extract_hover_text_from_plain_value() {
    let result = serde_json::json!({ "contents": { "value": "hello world" } });
    assert_eq!(extract_hover_text(&result), "hello world");
}

#[test]
fn extract_hover_text_from_array() {
    let result = serde_json::json!({ "contents": [ { "value": "a" }, "b" ] });
    assert_eq!(extract_hover_text(&result), "a\nb");
}

#[test]
fn parse_signature_line_rust_and_go() {
    assert_eq!(
        parse_signature_line("pub fn compute_signal(input: i64) -> i64 {", Language::Rust),
        Some(("compute_signal".to_string(), 7))
    );
    assert_eq!(
        parse_signature_line("    pub fn new<T: Clone>(x: T) -> Self {", Language::Rust),
        Some(("new".to_string(), 11))
    );
    assert_eq!(
        parse_signature_line("pub fn trait_item(x: i64) -> i64;", Language::Rust),
        None
    );
    assert_eq!(
        parse_signature_line("pub fn multi_line(", Language::Rust),
        None
    );
    assert_eq!(
        parse_signature_line("func Compute(a int, b string) error {", Language::Go),
        Some(("Compute".to_string(), 5))
    );
    assert_eq!(
        parse_signature_line("func (s *Server) Method() error {", Language::Go),
        None
    );
    assert_eq!(parse_signature_line("func main() {", Language::Go), None);
    assert_eq!(parse_signature_line("func init() {", Language::Go), None);
    assert_eq!(
        parse_signature_line("pub fn main() {", Language::Rust),
        None
    );
}

#[test]
fn mutate_signature_appends_marker_parameter() {
    assert_eq!(
        mutate_signature("pub fn compute_signal(input: i64) -> i64 {", Language::Rust).unwrap(),
        MUTATED_SIGNATURE.to_string() + " {"
    );
    assert_eq!(
        mutate_signature("pub fn empty() {", Language::Rust).unwrap(),
        "pub fn empty(divergent_marker: i64) {"
    );
    assert_eq!(
        mutate_signature("pub fn nested(f: fn(i64) -> i64) -> i64 {", Language::Rust).unwrap(),
        "pub fn nested(f: fn(i64) -> i64, divergent_marker: i64) -> i64 {"
    );
    assert_eq!(
        mutate_signature("func Compute(a int) error {", Language::Go).unwrap(),
        "func Compute(a int, divergentMarker int) error {"
    );
}

#[test]
fn candidate_source_filter_skips_tests_and_generated() {
    assert!(is_candidate_source("src/lib.rs", Language::Rust));
    assert!(!is_candidate_source("tests/it.rs", Language::Rust));
    assert!(!is_candidate_source("benches/b.rs", Language::Rust));
    assert!(!is_candidate_source("build.rs", Language::Rust));
    assert!(!is_candidate_source("src/lib.go", Language::Rust));
    assert!(is_candidate_source("cmd/server/main.go", Language::Go));
    assert!(!is_candidate_source(
        "cmd/server/main_test.go",
        Language::Go
    ));
    assert!(!is_candidate_source("api/v1/types.pb.go", Language::Go));
    assert!(!is_candidate_source("vendor/x/y.go", Language::Go));
}

#[test]
fn setup_creates_four_worktrees_with_expected_mutations() {
    let tmp = tempfile::tempdir().unwrap();
    let setup = setup(None, tmp.path(), WorkspaceMode::Shared, 1).expect("setup should succeed");
    assert_eq!(setup.worktrees.len(), 4);
    assert_eq!(setup.workspace_name, "fixture-divergent-bench");
    assert_eq!(setup.target.symbol, "compute_signal");
    assert_eq!(setup.target.file_rel, PathBuf::from("src/lib.rs"));
    assert!(setup.origin.join("Cargo.toml").exists());

    for wt in &setup.worktrees {
        assert!(wt.root.exists(), "{:?} should exist", wt.root);
        assert_eq!(wt.workspace_name, "fixture-divergent-bench");
        let content = std::fs::read_to_string(&wt.query_file).unwrap();
        match wt.kind {
            WorktreeKind::Master => assert!(content.contains(BASE_SIGNATURE)),
            WorktreeKind::SignatureChange => assert!(content.contains(MUTATED_SIGNATURE)),
            WorktreeKind::DependencyChange => assert!(content.contains(BASE_SIGNATURE)),
            WorktreeKind::UntrackedFile => {
                assert!(content.contains("divergent_untracked_symbol"));
                assert!(wt.query_file.ends_with("src/divergent_untracked.rs"));
                let owner = std::fs::read_to_string(wt.root.join("src/lib.rs")).unwrap();
                assert!(owner.contains("mod divergent_untracked;"));
                assert!(owner.contains(BASE_SIGNATURE));
            }
        }
    }

    let manifest_of = |kind: WorktreeKind| {
        let root = &setup
            .worktrees
            .iter()
            .find(|w| w.kind == kind)
            .unwrap()
            .root;
        std::fs::read_to_string(root.join("Cargo.toml")).unwrap()
    };
    assert!(manifest_of(WorktreeKind::DependencyChange).contains("manifest touched"));
    assert!(!manifest_of(WorktreeKind::Master).contains("manifest touched"));
}

#[test]
fn setup_isolated_mode_names_each_worktree_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let setup = setup(None, tmp.path(), WorkspaceMode::Isolated, 1).unwrap();
    let names: Vec<&str> = setup
        .worktrees
        .iter()
        .map(|w| w.workspace_name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "fixture-divergent-bench--wt-master",
            "fixture-divergent-bench--wt-signature",
            "fixture-divergent-bench--wt-dependency",
            "fixture-divergent-bench--wt-untracked",
        ]
    );
}

#[test]
fn setup_forks_every_kind_as_many_times_as_asked() {
    let tmp = tempfile::tempdir().unwrap();
    let setup = setup(None, tmp.path(), WorkspaceMode::Isolated, 2).unwrap();
    assert_eq!(setup.worktrees.len(), 8);
    let names: std::collections::BTreeSet<&str> = setup
        .worktrees
        .iter()
        .map(|w| w.workspace_name.as_str())
        .collect();
    assert_eq!(names.len(), 8, "{names:?}");
    assert!(names.contains("fixture-divergent-bench--wt-signature-2"));
    let second = setup
        .worktrees
        .iter()
        .find(|w| w.root.ends_with("wt-signature-2"))
        .unwrap();
    assert_eq!(second.kind, WorktreeKind::SignatureChange);
    let content = std::fs::read_to_string(&second.query_file).unwrap();
    assert!(content.contains(MUTATED_SIGNATURE));
}

#[tokio::test]
async fn run_rejects_too_few_workers() {
    let config = DivergentBenchConfig {
        workers: MIN_WORKERS - 1,
        ..DivergentBenchConfig::default()
    };
    let err = run(config).await.unwrap_err();
    assert!(err.to_string().contains("at least"));
}

#[tokio::test]
async fn run_rejects_a_worktree_count_that_is_not_a_multiple_of_four() {
    let config = DivergentBenchConfig {
        worktrees: 15,
        ..DivergentBenchConfig::default()
    };
    let err = run(config).await.unwrap_err();
    assert!(err.to_string().contains("multiple of 4"), "{err}");
}

#[test]
fn every_language_names_its_manifest_its_extension_and_its_marker() {
    for language in [Language::Rust, Language::Go] {
        assert!(!language.label().is_empty(), "a label");
        assert!(
            language.manifest().contains('.') || !language.manifest().is_empty(),
            "{} names the file that makes a project: {}",
            language.label(),
            language.manifest()
        );
        assert!(
            language.extension().starts_with('.') || !language.extension().is_empty(),
            "{} names its source extension",
            language.label()
        );
        assert!(
            language.marker().contains("divergent") || !language.marker().is_empty(),
            "{} names the marker the benchmark looks for",
            language.label()
        );
        assert!(
            !language.untracked_symbol().is_empty(),
            "{} names the symbol that exists only in the worktree",
            language.label()
        );
    }
}

#[test]
fn a_worktree_kind_has_a_label_and_a_directory() {
    for kind in [
        WorktreeKind::Master,
        WorktreeKind::SignatureChange,
        WorktreeKind::DependencyChange,
        WorktreeKind::UntrackedFile,
    ] {
        assert!(!kind.label().is_empty(), "{kind:?} has a label");
        assert!(!kind.dir_name().is_empty(), "{kind:?} has a directory");
    }
}

#[test]
fn the_workspace_name_follows_the_repository_it_benchmarks() {
    let named = bench_workspace_name(Some(Path::new("/somewhere/my-repo")));
    assert!(
        named.contains("my-repo"),
        "the repository's name is in it: {named}"
    );
    let anonymous = bench_workspace_name(None);
    assert!(
        !anonymous.trim().is_empty(),
        "and there is still a name without one: {anonymous}"
    );
}

#[test]
fn a_go_package_is_read_from_its_first_line_and_nothing_else() {
    assert_eq!(
        go_package_name("package signal\n\nfunc A() {}\n").as_deref(),
        Some("signal")
    );
    assert_eq!(go_package_name("// only a comment\n"), None);
    assert_eq!(go_package_name(""), None);
}
