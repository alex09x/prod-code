/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

/// If `workspace_root` is a worktree copy (e.g. `<repo>--wt-<hash>` or git worktree with `.git` pointer), finds the base checkout root.
pub fn split_worktree_base(workspace_root: &Path) -> Option<PathBuf> {
    for ancestor in workspace_root.ancestors() {
        let Some(name) = ancestor.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        // 1. Server worktree naming: `<base>--wt-<hash>`
        if let Some((base_name, _)) = name.split_once("--wt-")
            && !base_name.is_empty()
        {
            let base_parent = ancestor.parent()?;
            let base_dir = base_parent.join(base_name);
            let relative = workspace_root.strip_prefix(ancestor).ok()?;
            let candidate = base_dir.join(relative);
            if base_dir.exists() {
                return Some(candidate);
            }
        }

        // 2. Standard Git worktree: ancestor contains a `.git` file with `gitdir:`
        let git_file = ancestor.join(".git");
        if git_file.is_file()
            && let Ok(content) = std::fs::read_to_string(&git_file)
        {
            for line in content.lines() {
                if let Some(gitdir_raw) = line.trim().strip_prefix("gitdir: ") {
                    let mut gitdir_path = PathBuf::from(gitdir_raw.trim());
                    if gitdir_path.is_relative() {
                        gitdir_path = ancestor.join(&gitdir_path);
                    }
                    if let Ok(canon) = gitdir_path.canonicalize() {
                        gitdir_path = canon;
                    }
                    for anc in gitdir_path.ancestors() {
                        if anc.file_name().and_then(|n| n.to_str()) == Some(".git")
                            && let Some(base_repo) = anc.parent()
                            && base_repo.exists()
                            && base_repo != ancestor
                            && let Ok(relative) = workspace_root.strip_prefix(ancestor)
                        {
                            let candidate = base_repo.join(relative);
                            return Some(candidate);
                        }
                    }
                }
            }
        }
    }

    None
}

/// Extract a clean, generic workspace identifier from any client workspace or worktree path.
/// Short stable hash of a client root, used to give every worktree its own server workspace.
pub fn worktree_suffix(client_root: &str) -> String {
    let hash = client_root
        .bytes()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        }) as u32;
    format!("--wt-{hash:08x}")
}

pub fn extract_workspace_identifier(client_root: &str) -> String {
    let path = Path::new(client_root);

    // Normalize path components
    let components: Vec<&str> = path
        .iter()
        .filter_map(|c| c.to_str())
        .filter(|&c| c != "/" && c != "\\" && !c.is_empty())
        .collect();

    // 1. Check for standard runner worktree container:
    // e.g. ".../worktrees/<workspace_name>/task-<id>/..."
    for (i, &seg) in components.iter().enumerate() {
        if seg == "worktrees" && i + 1 < components.len() {
            let next_seg = components[i + 1];
            // If followed by task-* or attempt-*, next_seg is the workspace identifier
            if i + 2 < components.len()
                && (components[i + 2].starts_with("task-")
                    || components[i + 2].starts_with("attempt-"))
            {
                // Each worktree owns an isolated workspace named after its origin repository.
                return sanitize_identifier(next_seg) + &worktree_suffix(client_root);
            }
        }
    }

    // 2. Check for local worktrees inside repository:
    // e.g. ".../<project_name>/.worktrees/..." or ".../<project_name>/worktrees/..."
    for (i, &seg) in components.iter().enumerate() {
        if (seg == ".worktrees" || seg == "worktrees") && i > 0 {
            let prev_seg = components[i - 1];
            // Disregard generic container prefixes
            if prev_seg != "Volumes"
                && prev_seg != "mnt"
                && prev_seg != "srv"
                && prev_seg != "home"
                && prev_seg != "var"
            {
                return sanitize_identifier(prev_seg) + &worktree_suffix(client_root);
            }
        }
    }

    // 3. Fallback: nearest ancestor that is not a task-* or attempt-* runner directory
    if let Some(pos) = components.iter().rposition(|&c| {
        !c.starts_with("task-") && !c.starts_with("attempt-") && c != "worktree" && c != "worktrees"
    }) {
        let name = components[pos];
        if !name.is_empty() {
            return sanitize_identifier(name);
        }
    }

    // 4. Fallback: folder name of client_root
    let fallback = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("default");

    sanitize_identifier(fallback)
}

pub fn sanitize_identifier(s: &str) -> String {
    let sanitized: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Go tools skip a directory whose name begins with `.` or `_`: a copy named so hid its own
    // module from gopls, whose first `workspace/symbol` found nothing (#391).
    let sanitized = match sanitized.chars().next() {
        Some('.') => format!("dot-{}", sanitized.trim_start_matches('.')),
        Some('_') => format!("under-{}", sanitized.trim_start_matches('_')),
        _ => sanitized,
    };
    if sanitized.is_empty() {
        "workspace".to_string()
    } else {
        sanitized
    }
}

/// Resolve client workspace path or worktree path to the canonical server workspace root.
pub fn resolve_server_workspace(
    storage_root: &Path,
    client_root: &str,
    explicit_base_name: Option<&str>,
) -> PathBuf {
    let target_dir = server_workspace_path(storage_root, client_root, explicit_base_name);
    let _ = std::fs::create_dir_all(&target_dir);
    std::fs::canonicalize(&target_dir).unwrap_or(target_dir)
}

/// The server workspace directory for a client, without creating it.
pub fn server_workspace_path(
    storage_root: &Path,
    client_root: &str,
    explicit_base_name: Option<&str>,
) -> PathBuf {
    let candidate_name = match explicit_base_name {
        Some(name) if !name.trim().is_empty() => sanitize_identifier(name.trim()),
        _ => extract_workspace_identifier(client_root),
    };
    storage_root.join(candidate_name)
}
