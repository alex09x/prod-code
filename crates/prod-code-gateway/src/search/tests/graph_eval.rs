/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::graph::{WorkspaceIndex, category_weight, centrality_score};
use super::super::patterns::declaration_on;
use super::super::scoring::{first_sentence, rank_with};
use super::super::types::FileEntry;
use super::fixtures::index_decls;
use std::time::Instant;

#[test]
fn first_sentence_is_bounded() {
    assert_eq!(first_sentence("One. Two."), "One.");
    assert_eq!(first_sentence(""), "");
    let long = "x".repeat(400);
    assert_eq!(first_sentence(&long).len(), 200);
}

#[test]
fn typed_graph_builds_in_degree_and_adjacency_across_languages() {
    let mut index = WorkspaceIndex::default();
    // Rust
    index.files.insert(
        "src/cluster.rs".into(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: index_decls(
                "src/cluster.rs",
                "/// Node in a cluster.\npub struct Node {}\n/// Schedules work on a Node.\npub struct Cluster {\n    pub fn schedule(&self, node: &Node) {}\n}\n",
            ),
        },
    );
    // Go
    index.files.insert(
        "pkg/scheduler.go".into(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: index_decls(
                "pkg/scheduler.go",
                "// Context carries deadlines.\ntype Context struct {}\n// JobScheduler dispatches jobs.\ntype JobScheduler struct {}\nfunc (s *JobScheduler) Run(ctx *Context) {}\n",
            ),
        },
    );
    // TypeScript
    index.files.insert(
        "src/engine.ts".into(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: index_decls(
                "src/engine.ts",
                "export interface Task { id: string; }\nexport class Worker {\n    execute(task: Task): void {}\n}\n",
            ),
        },
    );
    // Python
    index.files.insert(
        "model/runner.py".into(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: index_decls(
                "model/runner.py",
                "# Tensor representation.\nclass Tensor:\n    pass\n# ModelRunner executes inference.\nclass ModelRunner:\n    def run(self, input_tensor: Tensor):\n        pass\n",
            ),
        },
    );

    index.rebuild_graph();

    // Node is referenced by Cluster::schedule and doc
    let node_deg = index.graph.in_degree.get("Node").copied().unwrap_or(0);
    assert!(
        node_deg >= 1,
        "Node in-degree should be >= 1, got {node_deg}"
    );

    // Context is referenced by JobScheduler.Run
    let ctx_deg = index.graph.in_degree.get("Context").copied().unwrap_or(0);
    assert!(
        ctx_deg >= 1,
        "Context in-degree should be >= 1, got {ctx_deg}"
    );

    // Task is referenced by Worker.execute
    let task_deg = index.graph.in_degree.get("Task").copied().unwrap_or(0);
    assert!(
        task_deg >= 1,
        "Task in-degree should be >= 1, got {task_deg}"
    );

    // Tensor is referenced by ModelRunner.run
    let tensor_deg = index.graph.in_degree.get("Tensor").copied().unwrap_or(0);
    assert!(
        tensor_deg >= 1,
        "Tensor in-degree should be >= 1, got {tensor_deg}"
    );

    // Verify category weighting
    assert!(category_weight("struct") > category_weight("function"));
    assert!(category_weight("class") > category_weight("constant"));
    assert!(centrality_score("struct", 10) > centrality_score("struct", 0));
}

#[test]
fn typed_graph_fusion_elevates_central_architectural_types() {
    let mut index = WorkspaceIndex::default();
    index.files.insert(
        "src/tail.rs".into(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: index_decls(
                "src/tail.rs",
                r#"
/// Ring buffer keeping the last bytes of process output.
pub struct TailBuffer {
    capacity: usize,
}

impl TailBuffer {
    pub fn new(capacity: usize) -> Self { Self { capacity } }
    pub fn push(&mut self, bytes: &[u8]) {}
}

/// Helper function that logs temporary tail buffer output during debug.
pub fn log_temp_tail_output(buf: &TailBuffer) {}
"#,
            ),
        },
    );
    index.files.insert(
        "src/exec.rs".into(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls: index_decls(
                "src/exec.rs",
                r#"
/// Execution output holding stdout and stderr buffers.
pub struct ExecOutput {
    pub stdout: TailBuffer,
    pub stderr: TailBuffer,
}

pub fn run_exec_command(output: &mut ExecOutput) {}
"#,
            ),
        },
    );

    index.rebuild_graph();

    let query = "keep last bytes in tail buffer output";
    let hits = rank_with(&index, query, None, 5, None);
    assert!(!hits.is_empty(), "expected hits for query");

    // TailBuffer should be the top hit due to structural centrality and in-degree reinforcement
    assert_eq!(
        hits[0].name,
        "TailBuffer",
        "TailBuffer should be top hit: got {:?}",
        hits.iter().map(|h| &h.name).collect::<Vec<_>>()
    );
    assert!(hits[0].score.is_some(), "hit must have attributable score");
    let reasons = hits[0]
        .rank_reasons
        .as_ref()
        .expect("hit must have rank reasons");
    assert!(
        reasons.iter().any(|r| r.contains("graph:")),
        "reasons must contain graph attribution: {reasons:?}"
    );
    assert!(
        reasons.iter().any(|r| r.contains("lexical:")),
        "reasons must contain lexical attribution: {reasons:?}"
    );
}

#[test]
fn search_sub_10ms_latency_and_attribution_evidence() {
    let mut index = WorkspaceIndex::default();
    // Generate a synthetic workspace with 20 files and 420 declarations
    for f in 0..20 {
        let mut text = String::new();
        text.push_str(&format!(
            "/// Module {f} primary manager.\npub struct Manager{f} {{\n    id: u32,\n}}\n\n"
        ));
        for d in 0..20 {
            text.push_str(&format!(
                "/// Operation {d} in module {f} processing requests.\npub fn process_req_{f}_{d}(mgr: &Manager{f}) -> u32 {{ {d} }}\n\n"
            ));
        }
        index.files.insert(
            format!("src/module_{f}.rs"),
            FileEntry {
                stamp: (0, 0),
                generation: 0,
                decls: index_decls(&format!("src/module_{f}.rs"), &text),
            },
        );
    }
    index.rebuild_graph();

    let queries = [
        "process requests in manager",
        "primary manager module",
        "operation processing requests",
        "module manager request handler",
    ];

    let mut latencies_us = Vec::new();
    for _ in 0..50 {
        for q in &queries {
            let start = Instant::now();
            let hits = rank_with(&index, q, None, 10, None);
            let elapsed_us = start.elapsed().as_micros();
            latencies_us.push(elapsed_us);
            assert!(!hits.is_empty());
            assert!(hits[0].score.is_some());
            assert!(hits[0].rank_reasons.is_some());
        }
    }

    latencies_us.sort_unstable();
    let p50_us = latencies_us[latencies_us.len() / 2];
    let p95_us = latencies_us[latencies_us.len() * 95 / 100];
    let max_us = latencies_us[latencies_us.len() - 1];

    println!(
        "Latency evidence (200 searches over 420 declarations): p50={:.2}ms, p95={:.2}ms, max={:.2}ms",
        p50_us as f64 / 1000.0,
        p95_us as f64 / 1000.0,
        max_us as f64 / 1000.0
    );

    // Strict assertion: universal sub-10 ms latency (p50 and p95 under 10 ms)
    assert!(
        p50_us < 10_000,
        "p50 latency was {} us (> 10000 us)",
        p50_us
    );
    assert!(
        p95_us < 10_000,
        "p95 latency was {} us (> 10000 us)",
        p95_us
    );
}

#[test]
fn typescript_method_detection_rejects_calls_but_accepts_declarations() {
    assert_eq!(declaration_on("doWork(arg);", "typescript"), None);
    assert_eq!(declaration_on("console.log(arg);", "typescript"), None);
    assert_eq!(
        declaration_on("doWork(arg): void {", "typescript"),
        Some(("method".to_string(), "doWork".to_string()))
    );
    assert_eq!(
        declaration_on("doWork(arg) {", "typescript"),
        Some(("method".to_string(), "doWork".to_string()))
    );
    assert_eq!(
        declaration_on("doWork(cb: (x: number) => void): void {", "typescript"),
        Some(("method".to_string(), "doWork".to_string()))
    );
    assert_eq!(
        declaration_on("doWork(cb: (x: number) => void) {", "typescript"),
        Some(("method".to_string(), "doWork".to_string()))
    );
}
