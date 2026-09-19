//! Workspace file scanner for fast sync over 10G LAN.

use anyhow::Result;
use prod_code_protocol::FileDelta;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB per source file limit
const MAX_JSON_CONFIG_SIZE: u64 = 256 * 1024; // 256 KiB for .json configs (reject datasets)

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncFileEntry {
    pub mtime_sec: u64,
    pub mtime_nsec: u32,
    pub size: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SyncCache {
    pub files: HashMap<String, SyncFileEntry>,
}

fn cache_file_path(root: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    let hash = hasher.finish();

    let folder_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("ws");
    let sanitized: String = folder_name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();

    let cache_dir = std::env::temp_dir().join("prod_code_sync_cache");
    let _ = std::fs::create_dir_all(&cache_dir);
    cache_dir.join(format!("{sanitized}_{hash:016x}.json"))
}

pub fn load_sync_cache(root: &Path) -> SyncCache {
    let path = cache_file_path(root);
    if let Ok(data) = std::fs::read(&path) {
        if let Ok(cache) = serde_json::from_slice::<SyncCache>(&data) {
            return cache;
        }
    }
    SyncCache::default()
}

pub fn save_sync_cache(root: &Path, cache: &SyncCache) {
    let path = cache_file_path(root);
    if let Ok(data) = serde_json::to_vec(cache) {
        let _ = std::fs::write(path, data);
    }
}

pub fn clear_sync_cache(root: &Path) {
    let path = cache_file_path(root);
    let _ = std::fs::remove_file(path);
}

/// Returns true if the relative path represents a code or configuration file relevant to language servers.
pub fn is_relevant_code_or_manifest_file(rel_path: &str) -> bool {
    let path = Path::new(rel_path);

    // 1. Check directory components for non-code / build / data trees
    for component in path.components() {
        if let std::path::Component::Normal(comp) = component {
            let s = comp.to_string_lossy();
            if s.starts_with('.') && s != ".cargo" {
                return false;
            }
            if matches!(
                s.as_ref(),
                "target"
                    | "node_modules"
                    | "vendor"
                    | "dist"
                    | "build"
                    | "results"
                    | "samples"
                    | "__pycache__"
                    | "artifacts"
                    | "dogfood-output"
                    | "data"
                    | "dataset"
                    | "datasets"
                    | "corpus"
                    | "traces"
                    | "state"
                    | "research"
                    | "benchmarks"
                    | "benchmark"
                    | ".idea"
                    | ".vscode"
            ) {
                return false;
            }
        }
    }

    // 2. Binary / media extensions
    if is_binary_or_media_file(rel_path) {
        return false;
    }

    // 3. Known non-code data, dumps, documentation, and log formats
    let lower = rel_path.to_lowercase();
    if lower.ends_with(".jsonl")
        || lower.ends_with(".csv")
        || lower.ends_with(".tsv")
        || lower.ends_with(".parquet")
        || lower.ends_with(".arrow")
        || lower.ends_with(".feather")
        || lower.ends_with(".log")
        || lower.ends_with(".md")
        || lower.ends_with(".txt")
        || lower.ends_with(".rst")
        || lower.ends_with(".pdf")
        || lower.ends_with(".doc")
        || lower.ends_with(".docx")
    {
        return false;
    }

    // 4. Code & manifest extensions
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        matches!(
            ext_lower.as_str(),
            "rs" | "go"
                | "mod"
                | "sum"
                | "work"
                | "py"
                | "pyi"
                | "js"
                | "mjs"
                | "cjs"
                | "jsx"
                | "ts"
                | "mts"
                | "cts"
                | "tsx"
                | "vue"
                | "svelte"
                | "c"
                | "h"
                | "cc"
                | "cpp"
                | "cxx"
                | "hh"
                | "hpp"
                | "hxx"
                | "inl"
                | "java"
                | "kt"
                | "kts"
                | "scala"
                | "cs"
                | "swift"
                | "proto"
                | "thrift"
                | "graphql"
                | "gql"
                | "sql"
                | "sh"
                | "bash"
                | "zsh"
                | "toml"
                | "yaml"
                | "yml"
                | "json"
        )
    } else {
        // Files without extension: manifests and scripts
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        matches!(
            file_name,
            "Makefile"
                | "Dockerfile"
                | "Containerfile"
                | "Procfile"
                | "Gemfile"
                | "Rakefile"
                | "Cargo.lock"
        )
    }
}

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
        if is_relevant_code_or_manifest_file(&rel_path) {
            let content = std::fs::read(&target_dir)?;
            deltas.push(FileDelta {
                relative_path: rel_path,
                content: Some(content),
                is_executable: false,
            });
        }
        return Ok(deltas);
    }

    walk_dir(&target_dir, &canonical_root, &mut deltas)?;
    Ok(deltas)
}

/// Collect dirty, modified, untracked, and deleted files in a workspace directory.
/// When in a git repository or worktree, uses `git status --porcelain -uall` for sub-10ms discovery.
/// Uses lightweight memoization cache so files that were already synced and unchanged are not re-read or re-sent.
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
    let mut cache = load_sync_cache(root);
    let mut cache_modified = false;

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
                if cache.files.remove(old).is_some() {
                    cache_modified = true;
                }
            }
        }

        if !is_relevant_code_or_manifest_file(rel_path) {
            continue;
        }

        let full_path = root.join(rel_path);
        if is_delete {
            deltas.push(FileDelta {
                relative_path: rel_path.to_string(),
                content: None,
                is_executable: false,
            });
            if cache.files.remove(rel_path).is_some() {
                cache_modified = true;
            }
        } else if full_path.is_file() {
            if let Ok(metadata) = full_path.metadata() {
                let size = metadata.len();
                if size > MAX_FILE_SIZE {
                    continue;
                }
                if rel_path.ends_with(".json") && size > MAX_JSON_CONFIG_SIZE {
                    continue;
                }

                let mtime = metadata
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                let dur = mtime
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .unwrap_or_default();
                let mtime_sec = dur.as_secs();
                let mtime_nsec = dur.subsec_nanos();

                if let Some(entry) = cache.files.get(rel_path) {
                    if entry.mtime_sec == mtime_sec
                        && entry.mtime_nsec == mtime_nsec
                        && entry.size == size
                    {
                        // File was already synced and has not changed
                        continue;
                    }
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
                    cache.files.insert(
                        rel_path.to_string(),
                        SyncFileEntry {
                            mtime_sec,
                            mtime_nsec,
                            size,
                        },
                    );
                    cache_modified = true;
                }
            }
        }
    }

    if cache_modified {
        save_sync_cache(root, &cache);
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
                || name == "dataset"
                || name == "datasets"
                || name == "corpus"
                || name == "traces"
                || name == "state"
                || name == "research"
                || name == "benchmarks"
                || name == "benchmark"
                || name == ".idea"
                || name == ".vscode"
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

        let rel_path = path
            .strip_prefix(canonical_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        if !is_relevant_code_or_manifest_file(&rel_path) {
            continue;
        }

        if let Ok(metadata) = entry.metadata() {
            if rel_path.ends_with(".json") && metadata.len() > MAX_JSON_CONFIG_SIZE {
                continue;
            }
        }

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

        // 4. Create non-code / research files that MUST be ignored
        std::fs::create_dir_all(root.join("research")).unwrap();
        std::fs::write(root.join("research/bench.jsonl"), "{\"dump\": true}").unwrap();
        std::fs::write(root.join("notes.md"), "# Research Notes").unwrap();

        // 5. First collection: code files collected, non-code files ignored
        let deltas = collect_dirty_files(root).unwrap();
        let map: std::collections::HashMap<_, _> = deltas
            .into_iter()
            .map(|d| (d.relative_path, d.content))
            .collect();

        assert!(map.contains_key("src/lib.rs") || map.contains_key("src\\lib.rs"));
        assert!(map.contains_key("src/untracked.rs") || map.contains_key("src\\untracked.rs"));
        assert!(!map.contains_key("research/bench.jsonl"));
        assert!(!map.contains_key("notes.md"));

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

        // 6. Second consecutive collection without changes: cache hit, zero deltas!
        let deltas_cached = collect_dirty_files(root).unwrap();
        assert!(
            deltas_cached.is_empty(),
            "Expected 0 deltas on cache hit, got {}",
            deltas_cached.len()
        );

        // 7. Touch one file: only that file is collected again
        std::fs::write(root.join("src/lib.rs"), "pub fn modified_v2() {}").unwrap();
        let deltas_recheck = collect_dirty_files(root).unwrap();
        assert_eq!(deltas_recheck.len(), 1);
        assert!(deltas_recheck[0].relative_path.contains("lib.rs"));
    }
}
