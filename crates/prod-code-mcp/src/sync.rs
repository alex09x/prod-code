//! Workspace file scanner for fast sync over 10G LAN.

use anyhow::Result;
use prod_code_protocol::FileDelta;
use std::path::Path;

const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB per source file limit

/// Scan workspace directory and generate FileDelta list, filtering out build artifacts and VCS.
pub fn scan_workspace_files(root: &Path, subpath: Option<&Path>) -> Result<Vec<FileDelta>> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let target_dir = match subpath {
        Some(sub) => {
            if sub.is_absolute() {
                std::fs::canonicalize(sub).unwrap_or_else(|_| sub.to_path_buf())
            } else {
                let joined = canonical_root.join(sub);
                std::fs::canonicalize(&joined).unwrap_or(joined)
            }
        }
        None => canonical_root.clone(),
    };

    if !target_dir.exists() {
        anyhow::bail!("Path {:?} does not exist", target_dir);
    }

    let mut deltas = Vec::new();

    if target_dir.is_file() {
        let rel_path = target_dir
            .strip_prefix(&canonical_root)
            .unwrap_or(&target_dir)
            .to_string_lossy()
            .to_string();
        let content = std::fs::read(&target_dir)?;
        deltas.push(FileDelta {
            relative_path: rel_path,
            content: Some(content),
            is_executable: false,
        });
        return Ok(deltas);
    }

    walk_dir(&target_dir, &canonical_root, &mut deltas)?;
    Ok(deltas)
}

/// Collect dirty, modified, untracked, and deleted files in a workspace directory.
/// When in a git repository or worktree, uses `git status --porcelain -uall` for sub-10ms discovery.
pub fn collect_dirty_files(root: &Path) -> Result<Vec<FileDelta>> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());

    // Try git status discovery first
    if let Ok(deltas) = collect_git_dirty_files(&canonical_root) {
        return Ok(deltas);
    }

    Ok(Vec::new())
}

fn collect_git_dirty_files(root: &Path) -> Result<Vec<FileDelta>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("status")
        .arg("--porcelain")
        .arg("-uall")
        .output()?;

    if !output.status.success() {
        anyhow::bail!("git status non-zero exit");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut deltas = Vec::new();

    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        let raw_path = line[3..].trim();

        let (is_delete, rel_path, old_path) = if status.contains('D') {
            (true, raw_path, None)
        } else if let Some((old_p, new_p)) = raw_path.split_once(" -> ") {
            (
                false,
                new_p.trim().trim_matches('"'),
                Some(old_p.trim().trim_matches('"')),
            )
        } else {
            (false, raw_path.trim_matches('"'), None)
        };

        if let Some(old) = old_path {
            if !old.starts_with(".git") {
                deltas.push(FileDelta {
                    relative_path: old.to_string(),
                    content: None,
                    is_executable: false,
                });
            }
        }

        if rel_path.starts_with(".git") || is_binary_or_media_file(rel_path) {
            continue;
        }

        let full_path = root.join(rel_path);
        if is_delete {
            deltas.push(FileDelta {
                relative_path: rel_path.to_string(),
                content: None,
                is_executable: false,
            });
        } else if full_path.is_file() {
            if let Ok(metadata) = full_path.metadata() {
                if metadata.len() > MAX_FILE_SIZE {
                    continue;
                }
                #[cfg(unix)]
                let is_executable = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let is_executable = false;

                if let Ok(content) = std::fs::read(&full_path) {
                    deltas.push(FileDelta {
                        relative_path: rel_path.to_string(),
                        content: Some(content),
                        is_executable,
                    });
                }
            }
        }
    }

    Ok(deltas)
}

fn is_binary_or_media_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".tar")
        || lower.ends_with(".gz")
        || lower.ends_with(".bin")
        || lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".pdf")
        || lower.ends_with(".wasm")
        || lower.ends_with(".so")
        || lower.ends_with(".dylib")
        || lower.ends_with(".a")
        || lower.ends_with(".o")
        || lower.ends_with(".exe")
        || lower.ends_with(".hprof")
        || lower.ends_with(".mp4")
        || lower.ends_with(".mov")
        || lower.ends_with(".pyc")
        || lower.ends_with(".db")
        || lower.ends_with(".sqlite")
}

fn walk_dir(target_dir: &Path, canonical_root: &Path, deltas: &mut Vec<FileDelta>) -> Result<()> {
    let mut builder = ignore::WalkBuilder::new(target_dir);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .max_filesize(Some(MAX_FILE_SIZE))
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            if name == ".git"
                || name == "target"
                || name == "node_modules"
                || name == "vendor"
                || name == "dist"
                || name == "build"
                || name == "results"
                || name == "samples"
                || name == "__pycache__"
                || name == "artifacts"
                || name == "dogfood-output"
                || name == "data"
                || name == "state"
                || name == ".DS_Store"
            {
                return false;
            }
            true
        });

    for result in builder.build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let name_str = entry.file_name().to_string_lossy();
        if is_binary_or_media_file(&name_str) {
            continue;
        }

        let rel_path = path
            .strip_prefix(canonical_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        if let Ok(content) = std::fs::read(path) {
            #[cfg(unix)]
            let is_executable = {
                use std::os::unix::fs::PermissionsExt;
                entry
                    .metadata()
                    .ok()
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            };
            #[cfg(not(unix))]
            let is_executable = false;

            deltas.push(FileDelta {
                relative_path: rel_path,
                content: Some(content),
                is_executable,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_workspace_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/app"), "binary").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "git").unwrap();

        let deltas = scan_workspace_files(root, None).unwrap();
        let paths: Vec<_> = deltas.iter().map(|d| d.relative_path.as_str()).collect();

        assert!(paths.contains(&"src/main.rs") || paths.contains(&"src\\main.rs"));
        assert!(paths.contains(&"Cargo.toml"));
        assert!(!paths.iter().any(|p| p.contains("target")));
        assert!(!paths.iter().any(|p| p.contains(".git")));
    }

    #[test]
    fn test_collect_dirty_files_in_git_repo() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // Initialize a git repo in tempdir
        let init_status = std::process::Command::new("git")
            .arg("init")
            .current_dir(root)
            .status();
        if init_status.is_err() || !init_status.unwrap().success() {
            return; // git not available in environment, skip
        }

        // Configure git user for commit
        let _ = std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(root)
            .status();
        let _ = std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(root)
            .status();

        // 1. Initial committed file
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn original() {}").unwrap();
        let _ = std::process::Command::new("git")
            .args(["add", "src/lib.rs"])
            .current_dir(root)
            .status();
        let _ = std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(root)
            .status();

        // 2. Modify existing file
        std::fs::write(root.join("src/lib.rs"), "pub fn modified() {}").unwrap();

        // 3. Create a brand new untracked file (NO git add)
        std::fs::write(root.join("src/untracked.rs"), "pub fn untracked() {}").unwrap();

        // 4. Collect dirty files
        let deltas = collect_dirty_files(root).unwrap();
        let map: std::collections::HashMap<_, _> = deltas
            .into_iter()
            .map(|d| (d.relative_path, d.content))
            .collect();

        assert!(map.contains_key("src/lib.rs") || map.contains_key("src\\lib.rs"));
        assert!(map.contains_key("src/untracked.rs") || map.contains_key("src\\untracked.rs"));

        let untracked_content = map
            .get("src/untracked.rs")
            .or_else(|| map.get("src\\untracked.rs"))
            .unwrap()
            .as_ref()
            .unwrap();
        assert_eq!(
            std::str::from_utf8(untracked_content).unwrap(),
            "pub fn untracked() {}"
        );
    }
}
