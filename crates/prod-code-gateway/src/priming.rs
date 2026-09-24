//! Warming rust-analyzer for the files an agent is editing, in the background (#233).
//!
//! The first diagnostics of a large file infer every function in it cold: 47 s for a
//! 4,246-line file on a Linux build node, against 2.3 s once warm. An agent validates the file
//! it just edited, so the gateway warms the Rust files a sync writes, and, when a workspace is
//! loaded, the ones modified most recently. The work runs on analysis snapshots without the
//! engine lock, so queries go on, and a write to the database (the next sync, an overlay)
//! cancels it.

use prod_code_engine_rust::RustEngine;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Instant, SystemTime};
use tokio::sync::Mutex;

/// Files warmed after a workspace loads, the most recently modified first.
pub const RECENT_FILES: usize = 10;
/// Files warmed after one sync; a sync that writes more is a checkout arriving, not an edit.
pub const SYNCED_FILES: usize = 20;

/// Workspaces with a warming job running: one at a time per workspace.
fn running() -> &'static StdMutex<HashSet<PathBuf>> {
    static RUNNING: OnceLock<StdMutex<HashSet<PathBuf>>> = OnceLock::new();
    RUNNING.get_or_init(Default::default)
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

/// Warms `files` of `workspace` in the background, unless a job for it is already running.
/// Returns whether a job was started.
pub fn warm_in_background(
    engine: Arc<Mutex<RustEngine>>,
    workspace: PathBuf,
    files: Vec<PathBuf>,
) -> bool {
    if files.is_empty() {
        return false;
    }
    {
        let mut guard = running().lock().unwrap_or_else(|e| e.into_inner());
        if !guard.insert(workspace.clone()) {
            return false;
        }
    }
    tokio::spawn(async move {
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
        tracing::info!(
            workspace = %workspace.display(),
            files = files.len(),
            functions = done,
            threads,
            ms = started.elapsed().as_millis() as u64,
            "warmed rust-analyzer in the background"
        );
        running()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&workspace);
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

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
