/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Warming rust-analyzer for the files an agent is editing, in the background (#233).
//!
//! The first diagnostics of a large file infer every function in it cold: 47 s for a
//! 4,246-line file on a Linux build node, against 2.3 s once warm. An agent validates the file
//! it just edited, so the gateway warms the Rust files a sync writes, and, when the validation
//! engine is loaded, the ones modified most recently. Validation runs on an engine of its own
//! (`SharedWorkspace::validation_view`), and that is the one warmed. The work runs on analysis
//! snapshots without the engine lock, so queries go on, and a write to the database (the next
//! sync, an overlay) cancels it.

use prod_code_engine_rust::RustEngine;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Instant, SystemTime};
use tokio::sync::Mutex;

/// Files warmed after the validation engine loads, the most recently modified first.
pub const RECENT_FILES: usize = 10;
/// Files whose diagnostics are computed through the engine after the snapshots are done.
pub const DIAGNOSED_FILES: usize = 3;
/// Files warmed after one sync; a sync that writes more is a checkout arriving, not an edit.
pub const SYNCED_FILES: usize = 20;

/// Workspaces with a warming job running, one at a time per workspace, each with the files
/// asked for while it runs: the job warms them next (#1426).
fn running() -> &'static StdMutex<HashMap<PathBuf, Vec<PathBuf>>> {
    static RUNNING: OnceLock<StdMutex<HashMap<PathBuf, Vec<PathBuf>>>> = OnceLock::new();
    RUNNING.get_or_init(Default::default)
}

/// Asks for `files` of `workspace` to be warmed: `true` when no job runs for it and the caller
/// starts one, `false` when the running job takes them after its current files. The queue
/// holds at most [`SYNCED_FILES`], the latest asked kept.
fn enqueue(workspace: &Path, files: &[PathBuf]) -> bool {
    let mut guard = running().lock().unwrap_or_else(|e| e.into_inner());
    let Some(queued) = guard.get_mut(workspace) else {
        guard.insert(workspace.to_path_buf(), Vec::new());
        return true;
    };
    for file in files {
        queued.retain(|q| q != file);
        queued.push(file.clone());
    }
    let excess = queued.len().saturating_sub(SYNCED_FILES);
    queued.drain(..excess);
    false
}

/// The files asked for while `workspace`'s job ran, for it to warm next; `None`, and the job
/// is over, when there are none.
fn next_batch(workspace: &Path) -> Option<Vec<PathBuf>> {
    let mut guard = running().lock().unwrap_or_else(|e| e.into_inner());
    let queued = std::mem::take(guard.get_mut(workspace)?);
    if queued.is_empty() {
        guard.remove(workspace);
        return None;
    }
    Some(queued)
}

/// The Rust files among `touched` (workspace-relative paths a sync wrote) that exist, at most
/// [`SYNCED_FILES`] of them; none when the sync wrote more than that.
pub fn synced_rust_files(workspace: &Path, touched: &[String]) -> Vec<PathBuf> {
    let files: Vec<PathBuf> = touched
        .iter()
        .filter(|rel| rel.ends_with(".rs"))
        .map(|rel| workspace.join(rel))
        .filter(|path| path.is_file())
        .collect();
    if files.len() > SYNCED_FILES {
        return Vec::new();
    }
    files
}

/// The `n` Rust files under `workspace` modified most recently, skipping build output and
/// version control.
pub fn recent_rust_files(workspace: &Path, n: usize) -> Vec<PathBuf> {
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut stack = vec![workspace.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                if !matches!(
                    name.to_str(),
                    Some("target" | ".git" | ".prod" | "node_modules")
                ) {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(modified) = meta.modified()
            {
                found.push((modified, path));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().take(n).map(|(_, path)| path).collect()
}

/// Warms `files` of `workspace` in the background. When a job for it is already running, the
/// files are queued and that job warms them after its current ones. Returns whether a job was
/// started.
pub fn warm_in_background(
    engine: Arc<Mutex<RustEngine>>,
    workspace: PathBuf,
    files: Vec<PathBuf>,
) -> bool {
    if files.is_empty() || !enqueue(&workspace, &files) {
        return false;
    }
    tokio::spawn(async move {
        let mut files = files;
        loop {
            warm(&engine, &workspace, &files).await;
            match next_batch(&workspace) {
                Some(next) => files = next,
                None => break,
            }
        }
    });
    true
}

/// One round of warming `files`: the priming job on snapshots, then the first files'
/// diagnostics.
async fn warm(engine: &Arc<Mutex<RustEngine>>, workspace: &Path, files: &[PathBuf]) {
    let started = Instant::now();
    // The job is made under the lock, which takes milliseconds, and runs after it is let go.
    let job = {
        let engine = engine.lock().await;
        let paths: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();
        engine.priming_job(&paths)
    };
    let threads = job.threads();
    let done = tokio::task::spawn_blocking(move || job.run())
        .await
        .unwrap_or(0);
    // Then the diagnostics a validation asks for, computed on a snapshot without holding the
    // engine lock, so concurrent queries, syncs, and refreshes never stall (#1118).
    for path in files.iter().take(DIAGNOSED_FILES) {
        let snap = engine.lock().await.snapshot_for_path(path);
        let path = path.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let _ = snap.diagnostics(&path);
        })
        .await;
    }
    tracing::info!(
        workspace = %workspace.display(),
        files = files.len(),
        functions = done,
        threads,
        ms = started.elapsed().as_millis() as u64,
        "warmed rust-analyzer in the background"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files asked for while a job runs are not dropped: the job takes the latest set after its
    /// current files, and the workspace is free again once none are left (#1426).
    #[test]
    fn files_asked_for_during_a_job_are_warmed_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("queued-ws");
        let (a, b, c) = (ws.join("a.rs"), ws.join("b.rs"), ws.join("c.rs"));
        assert!(
            enqueue(&ws, std::slice::from_ref(&a)),
            "no job runs: start one"
        );
        // Two more syncs while the first job is still running.
        assert!(!enqueue(&ws, &[b.clone(), c.clone()]));
        assert!(!enqueue(&ws, std::slice::from_ref(&b)));
        assert_eq!(next_batch(&ws), Some(vec![c.clone(), b.clone()]));
        assert_eq!(next_batch(&ws), None, "the job ends when nothing is left");
        assert!(enqueue(&ws, &[a]), "the workspace is free for a new job");
        let many: Vec<PathBuf> = (0..SYNCED_FILES + 5)
            .map(|i| ws.join(format!("f{i}.rs")))
            .collect();
        assert!(!enqueue(&ws, &many));
        let queued = next_batch(&ws).unwrap();
        assert_eq!(queued.len(), SYNCED_FILES, "the queue is bounded");
        assert_eq!(queued.last(), many.last(), "the latest asked are kept");
        assert_eq!(next_batch(&ws), None);
    }

    #[test]
    fn the_files_to_warm_are_the_edited_ones_or_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("src/old.rs"), "fn a() {}\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(root.join("src/new.rs"), "fn b() {}\n").unwrap();
        std::fs::write(root.join("target/debug/build.rs"), "fn c() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "text\n").unwrap();

        assert_eq!(
            recent_rust_files(root, 10),
            vec![root.join("src/new.rs"), root.join("src/old.rs")]
        );
        assert_eq!(recent_rust_files(root, 1), vec![root.join("src/new.rs")]);

        let touched = vec![
            "src/new.rs".to_string(),
            "README.md".to_string(),
            "src/gone.rs".to_string(),
        ];
        assert_eq!(
            synced_rust_files(root, &touched),
            vec![root.join("src/new.rs")]
        );
        let many: Vec<String> = (0..=SYNCED_FILES)
            .map(|_| "src/new.rs".to_string())
            .collect();
        assert!(
            synced_rust_files(root, &many).is_empty(),
            "a checkout arriving is not an edit"
        );
    }
}
