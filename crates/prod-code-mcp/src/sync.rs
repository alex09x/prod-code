//! Workspace file scanner for fast sync over 10G LAN.

use anyhow::Result;
use prod_code_protocol::FileDelta;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB per source file limit
const MAX_JSON_CONFIG_SIZE: u64 = 256 * 1024; // 256 KiB for .json configs (reject datasets)

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncFileEntry {
    pub mtime_sec: u64,
    pub mtime_nsec: u32,
    pub size: u64,
    #[serde(default)]
    pub hash: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncCache {
    #[serde(default)]
    pub base_commit_sha: Option<String>,
    #[serde(default)]
    pub last_sync_timestamp_ms: u64,
    #[serde(default)]
    pub files: HashMap<String, SyncFileEntry>,
}

fn cache_file_path(root: &Path) -> PathBuf {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let worktree_id = stable_hash(canonical_root.to_string_lossy().as_bytes());
    let cache_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".local/share/prod_code/sync"))
        .unwrap_or_else(|| std::env::temp_dir().join("prod_code_sync_cache"));
    let _ = std::fs::create_dir_all(&cache_dir);
    cache_dir.join(format!("{worktree_id:016x}.json"))
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
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&temporary, data).is_ok() {
            let _ = std::fs::rename(temporary, path);
        }
    }
}

pub fn clear_sync_cache(root: &Path) {
    let path = cache_file_path(root);
    let _ = std::fs::remove_file(path);
}

/// A prepared incremental sync. The state is committed only after the remote accepts the files.
#[derive(Debug)]
pub struct SyncPlan {
    pub files: Vec<FileDelta>,
    state: SyncCache,
}

/// Build a sync plan from the last acknowledged git base plus the current working tree.
///
/// The first plan for a worktree includes tracked source/manifest files. Later plans use both
/// `git diff <base>` and `git status --porcelain -uall`, then verify candidates against the
/// persisted mtime/size/hash watermark before reading them.
pub fn prepare_workspace_sync(root: &Path, subpath: Option<&Path>) -> Result<SyncPlan> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut state = load_sync_cache(&canonical_root);
    let current_base = git_head(&canonical_root)?;
    let changes = changed_paths(&canonical_root, state.base_commit_sha.as_deref())?;
    let filter = SyncPathFilter::new(&canonical_root, subpath)?;
    let mut files = Vec::new();

    for (relative_path, deleted) in changes {
        if !filter.includes(&relative_path) || !is_relevant_code_or_manifest_file(&relative_path) {
            continue;
        }

        if deleted {
            files.push(FileDelta {
                relative_path: relative_path.clone(),
                content: None,
                is_executable: false,
            });
            state.files.remove(&relative_path);
            continue;
        }

        let full_path = canonical_root.join(&relative_path);
        let Ok(metadata) = full_path.metadata() else {
            continue;
        };
        if !metadata.is_file()
            || metadata.len() > MAX_FILE_SIZE
            || (relative_path.ends_with(".json") && metadata.len() > MAX_JSON_CONFIG_SIZE)
        {
            continue;
        }

        let content = std::fs::read(&full_path)?;
        let entry = sync_file_entry(&metadata, &content);
        if state.files.get(&relative_path) == Some(&entry) {
            continue;
        }

        #[cfg(unix)]
        let is_executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let is_executable = false;

        files.push(FileDelta {
            relative_path: relative_path.clone(),
            content: Some(content),
            is_executable,
        });
        state.files.insert(relative_path, entry);
    }

    // A partial sync cannot advance the workspace-wide base: changes outside the selected path
    // still need to be included by the next full sync.
    if subpath.is_none() {
        state.base_commit_sha = Some(current_base);
    }
    Ok(SyncPlan { files, state })
}

/// Persist the watermarks for a sync plan after its files have been accepted by the gateway.
pub fn commit_workspace_sync(root: &Path, plan: &SyncPlan) {
    let mut state = plan.state.clone();
    state.last_sync_timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    save_sync_cache(root, &state);
}

fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn sync_file_entry(metadata: &std::fs::Metadata, content: &[u8]) -> SyncFileEntry {
    let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
    let duration = modified.duration_since(UNIX_EPOCH).unwrap_or_default();
    SyncFileEntry {
        mtime_sec: duration.as_secs(),
        mtime_nsec: duration.subsec_nanos(),
        size: metadata.len(),
        hash: stable_hash(content),
    }
}

fn git_head(root: &Path) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("git rev-parse HEAD non-zero exit");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn changed_paths(root: &Path, base: Option<&str>) -> Result<BTreeMap<String, bool>> {
    let mut paths = BTreeMap::new();
    // A recorded base can disappear after a rebase, amend or gc; fall back to the full tracked
    // tree instead of failing the sync, since the watermarks still filter unchanged files.
    let diff_from_base = match base {
        Some(base) if !base.is_empty() => {
            git_output(root, ["diff", "--name-status", "-z", base]).ok()
        }
        _ => None,
    };
    match diff_from_base {
        Some(output) => parse_name_status(&output, &mut paths),
        None => {
            let output = git_output(root, ["ls-files", "-z"])?;
            for path in output
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
            {
                paths.insert(String::from_utf8_lossy(path).to_string(), false);
            }
        }
    }

    let output = git_output(root, ["status", "--porcelain=v1", "-z", "-uall"])?;
    parse_porcelain_status(&output, &mut paths);
    Ok(paths)
}

fn git_output<const N: usize>(root: &Path, args: [&str; N]) -> Result<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        anyhow::bail!("git command non-zero exit");
    }
    Ok(output.stdout)
}

fn parse_name_status(output: &[u8], paths: &mut BTreeMap<String, bool>) {
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(status) = fields.next() {
        let status = String::from_utf8_lossy(status);
        let Some(path) = fields.next() else { break };
        if status.starts_with('R') || status.starts_with('C') {
            let Some(new_path) = fields.next() else { break };
            paths.insert(String::from_utf8_lossy(path).to_string(), true);
            paths.insert(String::from_utf8_lossy(new_path).to_string(), false);
        } else {
            paths.insert(
                String::from_utf8_lossy(path).to_string(),
                status.starts_with('D'),
            );
        }
    }
}

fn parse_porcelain_status(output: &[u8], paths: &mut BTreeMap<String, bool>) {
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let status = &entry[..2];
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        if status.contains(&b'R') || status.contains(&b'C') {
            let Some(old_path) = fields.next() else { break };
            paths.insert(String::from_utf8_lossy(old_path).to_string(), true);
        }
        paths.insert(path, status.contains(&b'D'));
    }
}

struct SyncPathFilter {
    relative: Option<PathBuf>,
}

impl SyncPathFilter {
    fn new(root: &Path, subpath: Option<&Path>) -> Result<Self> {
        let relative = match subpath {
            Some(path) => {
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    root.join(path)
                };
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                Some(path.strip_prefix(root)?.to_path_buf())
            }
            None => None,
        };
        Ok(Self { relative })
    }

    fn includes(&self, relative_path: &str) -> bool {
        self.relative.as_ref().is_none_or(|filter| {
            Path::new(relative_path) == filter || Path::new(relative_path).starts_with(filter)
        })
    }
}

/// Returns true if the relative path represents a code or configuration file relevant to language servers.
pub fn is_relevant_code_or_manifest_file(rel_path: &str) -> bool {
    let path = Path::new(rel_path);

    // 1. Check directory components for non-code / build / data trees
    let mut under_code_dir = false;
    for component in path.components() {
        if let std::path::Component::Normal(comp) = component {
            let s = comp.to_string_lossy();
            if s.starts_with('.') && s != ".cargo" {
                return false;
            }
            if matches!(s.as_ref(), "crates" | "packages" | "src") {
                under_code_dir = true;
            }
            if !under_code_dir
                && matches!(
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
                )
            {
                return false;
            }
            if under_code_dir && matches!(s.as_ref(), "target" | "node_modules" | "__pycache__") {
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

                #[cfg(unix)]
                let is_executable = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let is_executable = false;

                if let Ok(content) = std::fs::read(&full_path) {
                    let entry = sync_file_entry(&metadata, &content);
                    if cache.files.get(rel_path) == Some(&entry) {
                        // File was already synced and has not changed.
                        continue;
                    }
                    deltas.push(FileDelta {
                        relative_path: rel_path.to_string(),
                        content: Some(content),
                        is_executable,
                    });
                    cache.files.insert(rel_path.to_string(), entry);
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
            let path = entry.path();
            let is_under_code = path.components().any(|c| {
                if let std::path::Component::Normal(p) = c {
                    matches!(p.to_string_lossy().as_ref(), "crates" | "packages" | "src")
                } else {
                    false
                }
            });

            if !is_under_code
                && matches!(
                    name.as_ref(),
                    "research"
                        | "benchmarks"
                        | "benchmark"
                        | "data"
                        | "dataset"
                        | "datasets"
                        | "corpus"
                        | "traces"
                )
            {
                return false;
            }

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
                || name == "state"
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

    #[test]
    fn test_workspace_sync_state_uses_base_commit_and_watermarks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        clear_sync_cache(root);

        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        if !init.success() {
            return;
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            let configured = std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(root)
                .status()
                .unwrap();
            assert!(configured.success());
        }

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 1 }").unwrap();
        for args in [
            &["add", "src/lib.rs"][..],
            &["commit", "-qm", "initial"][..],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap();
            assert!(status.success());
        }

        let first = prepare_workspace_sync(root, None).unwrap();
        assert_eq!(first.files.len(), 1);
        commit_workspace_sync(root, &first);
        let state = load_sync_cache(root);
        assert!(state.base_commit_sha.is_some());
        assert!(state.last_sync_timestamp_ms > 0);
        assert!(
            state
                .files
                .get("src/lib.rs")
                .is_some_and(|entry| entry.hash != 0)
        );

        let unchanged = prepare_workspace_sync(root, None).unwrap();
        assert!(unchanged.files.is_empty());

        std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 2 }").unwrap();
        for args in [
            &["add", "src/lib.rs"][..],
            &["commit", "-qm", "changed"][..],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap();
            assert!(status.success());
        }

        let changed = prepare_workspace_sync(root, None).unwrap();
        assert_eq!(changed.files.len(), 1);
        assert_eq!(changed.files[0].relative_path, "src/lib.rs");
        clear_sync_cache(root);
    }

    #[test]
    fn test_unreachable_base_commit_falls_back_to_full_tree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        clear_sync_cache(root);

        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        if !init.success() {
            return;
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            let configured = std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(root)
                .status()
                .unwrap();
            assert!(configured.success());
        }

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 1 }").unwrap();
        for args in [&["add", "src"][..], &["commit", "-qm", "initial"][..]] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let initial = prepare_workspace_sync(root, None).unwrap();
        commit_workspace_sync(root, &initial);

        // Simulate history rewritten underneath the persisted watermark.
        let canonical = std::fs::canonicalize(root).unwrap();
        let mut state = load_sync_cache(&canonical);
        state.base_commit_sha = Some("0123456789abcdef0123456789abcdef01234567".to_string());
        save_sync_cache(&canonical, &state);

        // Unchanged file: fallback scans the full tree but the watermark still filters it.
        let unchanged = prepare_workspace_sync(root, None).unwrap();
        assert!(unchanged.files.is_empty());

        // Changed file: fallback still finds it.
        std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 3 }").unwrap();
        let changed = prepare_workspace_sync(root, None).unwrap();
        assert_eq!(changed.files.len(), 1);
        assert_eq!(changed.files[0].relative_path, "src/lib.rs");
        clear_sync_cache(root);
    }

    #[test]
    fn test_partial_sync_does_not_advance_workspace_base() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        clear_sync_cache(root);

        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        if !init.success() {
            return;
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            let configured = std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(root)
                .status()
                .unwrap();
            assert!(configured.success());
        }

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "pub const A: u8 = 1;").unwrap();
        std::fs::write(root.join("src/b.rs"), "pub const B: u8 = 1;").unwrap();
        for args in [&["add", "src"][..], &["commit", "-qm", "initial"][..]] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let initial = prepare_workspace_sync(root, None).unwrap();
        commit_workspace_sync(root, &initial);
        let initial_base = load_sync_cache(root).base_commit_sha;

        std::fs::write(root.join("src/a.rs"), "pub const A: u8 = 2;").unwrap();
        std::fs::write(root.join("src/b.rs"), "pub const B: u8 = 2;").unwrap();
        for args in [&["add", "src"][..], &["commit", "-qm", "both changed"][..]] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap();
            assert!(status.success());
        }

        let partial = prepare_workspace_sync(root, Some(Path::new("src/a.rs"))).unwrap();
        assert_eq!(partial.files.len(), 1);
        commit_workspace_sync(root, &partial);
        assert_eq!(load_sync_cache(root).base_commit_sha, initial_base);

        let remaining = prepare_workspace_sync(root, None).unwrap();
        assert_eq!(remaining.files.len(), 1);
        assert_eq!(remaining.files[0].relative_path, "src/b.rs");
        clear_sync_cache(root);
    }
}
