/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use anyhow::{Context, Result};

use super::types::Pruned;

struct TempIndexGuard<'a>(&'a Path);

impl<'a> Drop for TempIndexGuard<'a> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// Creates a Git commit for the pruned changes using a temporary index file.
pub fn create_prune_commit(
    root: &Path,
    git_dir: &Path,
    pruned: &mut Pruned,
    patch: &str,
) -> Result<()> {
    let head_out = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .context("failed to execute git rev-parse HEAD")?;
    anyhow::ensure!(
        head_out.status.success(),
        "cannot create git commit: repository has no commits on HEAD"
    );
    let head_sha = String::from_utf8_lossy(&head_out.stdout).trim().to_string();

    let temp_index_path = git_dir.join(format!(
        "prod_code_prune_index_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    let _guard = TempIndexGuard(&temp_index_path);

    let mut read_tree = std::process::Command::new("git");
    read_tree
        .current_dir(root)
        .args(["read-tree", "HEAD"])
        .env("GIT_INDEX_FILE", &temp_index_path);
    let read_out = read_tree
        .output()
        .context("failed to execute git read-tree")?;
    anyhow::ensure!(
        read_out.status.success(),
        "git read-tree failed: {}",
        String::from_utf8_lossy(&read_out.stderr)
    );

    let mut apply_cmd = std::process::Command::new("git");
    apply_cmd
        .current_dir(root)
        .args(["apply", "--cached", "-"])
        .env("GIT_INDEX_FILE", &temp_index_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = apply_cmd
        .spawn()
        .context("failed to spawn git apply --cached")?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(patch.as_bytes())
            .context("failed to write patch to git apply")?;
    }
    let apply_out = child
        .wait_with_output()
        .context("failed to wait on git apply")?;
    anyhow::ensure!(
        apply_out.status.success(),
        "git apply --cached failed: {}",
        String::from_utf8_lossy(&apply_out.stderr)
    );

    let mut write_tree = std::process::Command::new("git");
    write_tree
        .current_dir(root)
        .arg("write-tree")
        .env("GIT_INDEX_FILE", &temp_index_path);
    let write_out = write_tree
        .output()
        .context("failed to execute git write-tree")?;
    anyhow::ensure!(
        write_out.status.success(),
        "git write-tree failed: {}",
        String::from_utf8_lossy(&write_out.stderr)
    );
    let tree_sha = String::from_utf8_lossy(&write_out.stdout)
        .trim()
        .to_string();

    let commit_title = format!(
        "refactor(prune): remove {} unreferenced orphan(s)",
        pruned.removed.len()
    );
    let mut commit_body = format!(
        "Pruned {} orphan(s) of {} symbol(s) checked:\n",
        pruned.removed.len(),
        pruned.symbols_checked
    );
    for d in &pruned.removed {
        commit_body.push_str(&format!(
            "  - {} {} ({}:{}\n)",
            d.kind, d.name, d.file, d.line
        ));
    }
    let commit_msg = format!("{commit_title}\n\n{commit_body}");

    let commit_tree_out = std::process::Command::new("git")
        .current_dir(root)
        .args(["commit-tree", &tree_sha, "-p", &head_sha, "-m", &commit_msg])
        .env("GIT_AUTHOR_NAME", "Alexander Panasenko")
        .env("GIT_AUTHOR_EMAIL", "alex@prod.codes")
        .env("GIT_COMMITTER_NAME", "Alexander Panasenko")
        .env("GIT_COMMITTER_EMAIL", "alex@prod.codes")
        .output()
        .context("failed to execute git commit-tree")?;
    anyhow::ensure!(
        commit_tree_out.status.success(),
        "git commit-tree failed: {}",
        String::from_utf8_lossy(&commit_tree_out.stderr)
    );
    let commit_sha = String::from_utf8_lossy(&commit_tree_out.stdout)
        .trim()
        .to_string();

    let update_ref_out = std::process::Command::new("git")
        .current_dir(root)
        .args(["update-ref", "HEAD", &commit_sha, &head_sha])
        .output()
        .context("failed to execute git update-ref")?;
    anyhow::ensure!(
        update_ref_out.status.success(),
        "git update-ref failed: {}",
        String::from_utf8_lossy(&update_ref_out.stderr)
    );

    // Synchronize ambient repository index for the touched files only,
    // leaving any other unrelated staged changes in the ambient index untouched.
    let mut reset_cmd = std::process::Command::new("git");
    reset_cmd.current_dir(root).args(["reset", "HEAD", "--"]);
    for (path, _) in &pruned.rewritten {
        let p = Path::new(path);
        let rel = p.strip_prefix(root).unwrap_or(p);
        reset_cmd.arg(rel);
    }
    let reset_out = reset_cmd
        .output()
        .context("failed to execute git reset HEAD")?;
    anyhow::ensure!(
        reset_out.status.success(),
        "git reset HEAD failed: {}",
        String::from_utf8_lossy(&reset_out.stderr)
    );

    pruned.git_commit = Some(commit_sha);
    Ok(())
}
