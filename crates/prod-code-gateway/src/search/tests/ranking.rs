/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::graph::WorkspaceIndex;
use super::super::index::refresh;
use super::super::scoring::{dense, rank, rank_weighted, rank_with};
use super::super::types::{Declaration, EMBED_BATCH, FileEntry, Indexed};
use super::fixtures::index_decls;
use crate::embed::Embed;
use prod_code_protocol::SearchHit;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::time::Instant;

#[test]
fn a_question_finds_the_declaration_whose_prose_answers_it() {
    let mut index = WorkspaceIndex::default();
    index.files.insert(
        "src/place.rs".to_string(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: index_decls(
                "src/place.rs",
                "/// Decides which node runs a workspace: the one already holding it, else the quietest.\npub fn place(name: &str) -> Node { todo!() }\n",
            ),
        },
    );
    index.files.insert(
        "src/sync.rs".to_string(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: index_decls(
                "src/sync.rs",
                "/// Uploads changed files to the gateway.\npub fn push_sync(files: Vec<File>) {}\n",
            ),
        },
    );
    let hits = rank(
        &index,
        "where do we decide which node runs a workspace",
        5,
        None,
    );
    assert!(!hits.is_empty(), "the question should match something");
    assert_eq!(hits[0].name, "place", "got {:?}", hits[0]);
    assert!(
        hits[0]
            .doc
            .starts_with("Decides which node runs a workspace")
    );

    let scoped = rank(&index, "decide node workspace", 5, Some("src/sync.rs"));
    assert!(scoped.iter().all(|h| h.file == "src/sync.rs"));
}

#[test]
fn the_implementation_outranks_the_test_that_names_it() {
    let mut index = WorkspaceIndex::default();
    index.files.insert(
        "src/shadow.rs".to_string(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: [
                Declaration {
                    file: "src/shadow.rs".into(),
                    line: 10,
                    kind: "function".into(),
                    name: "run_overlay".into(),
                    container: None,
                    signature: "pub async fn run_overlay(job: Job)".into(),
                    doc: "Runs one hypothesis as an overlay shadow.".into(),
                    is_test: false,
                },
                Declaration {
                    file: "src/shadow.rs".into(),
                    line: 200,
                    kind: "function".into(),
                    name: "overlay_hypotheses_run_in_parallel_and_leave_the_workspace_untouched"
                        .into(),
                    container: Some("tests".into()),
                    signature:
                        "fn overlay_hypotheses_run_in_parallel_and_leave_the_workspace_untouched()"
                            .into(),
                    doc: String::new(),
                    is_test: true,
                },
            ]
            .into_iter()
            .map(Indexed::new)
            .collect(),
        },
    );
    let hits = rank(&index, "where are hypotheses run in an overlay", 5, None);
    assert_eq!(
        hits[0].name,
        "run_overlay",
        "got {:?}",
        hits.iter().map(|h| &h.name).collect::<Vec<_>>()
    );
    assert!(
        hits.iter()
            .all(|h| !h.name.starts_with("overlay_hypotheses")),
        "a test should not answer a question about the implementation"
    );
    // Asking about the test finds it.
    let about_tests = rank(
        &index,
        "test that overlay hypotheses leave the workspace untouched",
        5,
        None,
    );
    assert!(
        about_tests
            .iter()
            .any(|h| h.name.starts_with("overlay_hypotheses")),
        "got {:?}",
        about_tests.iter().map(|h| &h.name).collect::<Vec<_>>()
    );
}

#[test]
fn a_query_sharing_no_words_finds_nothing() {
    let mut index = WorkspaceIndex::default();
    index.files.insert(
        "a.rs".to_string(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: index_decls(
                "a.rs",
                "/// Adds two numbers.\npub fn add(a: i32, b: i32) -> i32 { a + b }\n",
            ),
        },
    );
    assert!(rank(&index, "kubernetes ingress certificate rotation", 5, None).is_empty());
    assert!(rank(&index, "", 5, None).is_empty());
}

#[test]
#[ignore]
fn eval_ranking_on_this_repository() {
    const QUESTIONS: &[(&str, &[&str])] = &[
        (
            "which machine should host this checkout",
            &["pick_node", "rendezvous_order", "place"],
        ),
        (
            "put the files back when writing fails halfway",
            &["restore", "apply_workspace_edit"],
        ),
        (
            "kill the command when it takes too long",
            &["kill_exec_group", "kill_exec_tree"],
        ),
        (
            "split an identifier into its words whatever its casing",
            &["words", "split_humps", "tokenize"],
        ),
        (
            "apply a patch without touching the disk",
            &["apply", "apply_hunks"],
        ),
        (
            "keep only the last few kilobytes of output",
            &["TailBuffer"],
        ),
        (
            "make compiler paths relative to the project",
            &["relativize_diagnostics"],
        ),
        (
            "which language a nested folder is written in",
            &["engine_project", "language_of"],
        ),
        (
            "the fixes rustc says are safe to apply automatically",
            &["parse_fixes", "machine_applicable"],
        ),
        (
            "walk up the directories to find the repository top",
            &["find_workspace_root"],
        ),
        (
            "cpu time and peak memory of a finished command",
            &["ExecUsage"],
        ),
        (
            "the first sentence of a comment for a one-line summary",
            &["first_sentence"],
        ),
        (
            "where a command runs on the server relative to the checkout root",
            &["subdir_of"],
        ),
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let storage = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("prod-code-storage/workspaces");
    let mut model = crate::embed::OnnxEmbedder::load(&crate::embed::model_dir(&storage))
        .expect("the model is installed on this node");
    let mut index = WorkspaceIndex::default();
    refresh(&root, &mut index, &AtomicU64::new(0));
    let started = Instant::now();
    let mut count = 0;
    for entry in index.files.values_mut() {
        for chunk in entry.decls.chunks_mut(EMBED_BATCH) {
            let texts: Vec<String> = chunk.iter().map(|d| d.passage()).collect();
            for (d, v) in chunk.iter_mut().zip(model.passages(&texts).unwrap()) {
                d.vector = Some(v);
                count += 1;
            }
        }
    }
    let took = started.elapsed().as_secs_f64();
    println!(
        "embedded {count} declarations of {} files in {took:.1}s ({:.0}/s)",
        index.files.len(),
        count as f64 / took
    );
    let mut vectors = Vec::new();
    let started = Instant::now();
    for (question, _) in QUESTIONS {
        vectors.push(model.query(question).unwrap());
    }
    println!(
        "a query vector takes {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0 / QUESTIONS.len() as f64
    );
    let hit = |hits: &[SearchHit], expected: &[&str]| {
        hits.iter()
            .take(3)
            .any(|h| expected.contains(&h.name.as_str()))
    };
    let mut rows: Vec<(String, usize)> = Vec::new();
    let lexical_only = QUESTIONS
        .iter()
        .filter(|(q, e)| hit(&rank(&index, q, 3, None), e))
        .count();
    rows.push(("lexical".into(), lexical_only));
    let docs_all: Vec<&Indexed> = index.declarations().filter(|d| !d.decl.is_test).collect();
    let dense_only = QUESTIONS
        .iter()
        .zip(&vectors)
        .filter(|((_, e), v)| {
            dense(&docs_all, v)
                .iter()
                .take(3)
                .any(|(d, _)| e.contains(&d.name.as_str()))
        })
        .count();
    rows.push(("dense".into(), dense_only));
    for weight in [1.0, 1.5, 2.0, 3.0] {
        let fused = QUESTIONS
            .iter()
            .zip(&vectors)
            .filter(|((q, e), v)| hit(&rank_weighted(&index, q, Some(v), 3, None, weight), e))
            .count();
        rows.push((format!("fused, dense weight {weight}"), fused));
    }
    for (name, n) in &rows {
        println!("{name:<24} top-3 {n}/{}", QUESTIONS.len());
    }
    for ((q, e), v) in QUESTIONS.iter().zip(&vectors) {
        let fused = rank_with(&index, q, Some(v), 3, None);
        println!(
            "{} {q}: {}",
            if hit(&fused, e) { "ok  " } else { "MISS" },
            fused
                .iter()
                .map(|h| h.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

#[test]
fn multi_term_query_ranks_promptly_without_hanging() {
    let mut index = WorkspaceIndex::default();
    index.files.insert(
        "crates/prod-code-client/src/sync/pull.rs".to_string(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: index_decls(
                "crates/prod-code-client/src/sync/pull.rs",
                "/// Pulls remote files and rejects truncated response before building or applying workspace delta.\npub fn pull_remote_files() {}\n",
            ),
        },
    );
    index.files.insert(
        "crates/prod-code-client/src/sync/delta.rs".to_string(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: index_decls(
                "crates/prod-code-client/src/sync/delta.rs",
                "/// Applies workspace delta.\npub fn apply_delta() {}\n",
            ),
        },
    );
    index.rebuild_graph();

    let started = Instant::now();
    let hits = rank(
        &index,
        "pull remote files reject truncated response before building or applying workspace delta",
        10,
        Some("crates"),
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(!hits.is_empty(), "expected hits for multi-term query");
    assert_eq!(hits[0].name, "pull_remote_files");
}
