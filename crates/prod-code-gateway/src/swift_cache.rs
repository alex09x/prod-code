//! Cross-worktree Swift engine support: shared Swift/Clang ModuleCache,
//! SwiftPM package checkout and artifact seeding, and compilation workspace
//! state relocation (Roadmap 3.7).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::cpp_index::relocate_path_or_uri;
use prod_code_protocol::transport::ScrubSecrets;

/// The environment variable name used to explicitly configure the shared Swift module cache directory.
pub const SWIFT_MODULE_CACHE_ENV: &str = "PROD_CODE_SWIFT_MODULE_CACHE";

/// Returns the path to the shared Swift and Clang module cache directory.
///
/// Precedence:
/// 1. `PROD_CODE_SWIFT_MODULE_CACHE` environment variable if set.
/// 2. `SWIFTPM_MODULECACHE_OVERRIDE` environment variable if already configured.
/// 3. `$HOME/.cache/prod-code/swift-module-cache`.
/// 4. `/var/tmp/prod-code/swift-module-cache` (or `/tmp/prod-code/swift-module-cache`).
/// 5. Temporary directory fallback (`std::env::temp_dir().join("prod-code-swift-module-cache")`).
///
/// Ensures the directory exists with mode `0700` on Unix systems.
pub fn swift_module_cache_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os(SWIFT_MODULE_CACHE_ENV) {
        let p = PathBuf::from(custom);
        let _ = ensure_cache_dir(&p);
        return p;
    }

    if let Some(override_path) = std::env::var_os("SWIFTPM_MODULECACHE_OVERRIDE") {
        let p = PathBuf::from(override_path);
        let _ = ensure_cache_dir(&p);
        return p;
    }

    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home)
            .join(".cache")
            .join("prod-code")
            .join("swift-module-cache");
        if ensure_cache_dir(&p).is_ok() {
            return p;
        }
    }

    let var_tmp = PathBuf::from("/var/tmp/prod-code/swift-module-cache");
    if ensure_cache_dir(&var_tmp).is_ok() {
        return var_tmp;
    }

    let temp = std::env::temp_dir().join("prod-code-swift-module-cache");
    let _ = ensure_cache_dir(&temp);
    temp
}

/// Ensures `dir` exists and has secure permissions (`0700` on Unix).
pub fn ensure_cache_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Returns the environment variables that configure the shared Swift and Clang module cache
/// across SwiftPM (`swift build`, `swift test`, `swift run`), `swiftc`, ClangImporter,
/// and `sourcekit-lsp` (Roadmap 3.7).
pub fn swift_module_cache_env() -> Vec<(String, String)> {
    let dir = swift_module_cache_dir();
    let dir_str = dir.to_string_lossy().into_owned();
    vec![
        // SwiftPM module cache override (passes -module-cache-path to swiftc and -fmodules-cache-path to clang)
        ("SWIFTPM_MODULECACHE_OVERRIDE".to_string(), dir_str.clone()),
        // Direct Swift compiler module cache path
        ("SWIFT_MODULE_CACHE_PATH".to_string(), dir_str.clone()),
        // Clang importer / C module cache path
        ("CLANG_MODULE_CACHE_PATH".to_string(), dir_str),
    ]
}

/// Relocates paths and `file://` URIs in a SwiftPM `workspace-state.json` file.
///
/// In SwiftPM, `.build/workspace-state.json` stores the resolved package dependency graph,
/// checkouts subpaths, local source control repository locations, and binary artifacts.
///
/// Preserves path-component boundaries: sibling paths such as `/work/repo-deps` when relocating
/// `/work/repo` are left completely untouched.
pub fn relocate_swiftpm_workspace_state(
    content: &str,
    from_str: &str,
    to_str: &str,
) -> io::Result<String> {
    if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(content) {
        relocate_json_value(&mut val, from_str, to_str);
        serde_json::to_string_pretty(&val).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    } else {
        // Fallback: line-by-line token relocation if json is not strictly standard
        let mut lines = Vec::new();
        for line in content.lines() {
            lines.push(relocate_path_or_uri(line, from_str, to_str));
        }
        Ok(lines.join("\n"))
    }
}

fn relocate_json_value(val: &mut serde_json::Value, from_str: &str, to_str: &str) {
    match val {
        serde_json::Value::String(s) => {
            let replaced = relocate_path_or_uri(s, from_str, to_str);
            if replaced != *s {
                *s = replaced;
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                relocate_json_value(item, from_str, to_str);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, item) in map {
                relocate_json_value(item, from_str, to_str);
            }
        }
        _ => {}
    }
}

/// Finds all directories containing a `Package.swift` manifest in `root`.
///
/// Returns relative paths from `root`. If `root` itself contains `Package.swift`,
/// `PathBuf::new()` is included. Recursively discovers nested SwiftPM packages
/// (e.g. `clients/macos/ProdUI`), while skipping known cache/node directories.
pub fn find_swift_packages(root: &Path) -> Vec<PathBuf> {
    let mut packages = Vec::new();
    walk_for_packages(root, root, &mut packages);
    packages.sort();
    packages.dedup();
    packages
}

fn walk_for_packages(root: &Path, dir: &Path, packages: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    let mut has_package = false;
    let mut subdirs = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str == "Package.swift" && entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            has_package = true;
        } else if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            if is_cache_or_ignored_dir(&name_str) {
                continue;
            }
            subdirs.push(entry.path());
        }
    }

    if has_package {
        if let Ok(rel) = dir.strip_prefix(root) {
            packages.push(rel.to_path_buf());
        }
    }

    for subdir in subdirs {
        walk_for_packages(root, &subdir, packages);
    }
}

fn is_cache_or_ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "target"
            | "node_modules"
            | ".venv"
            | "venv"
            | "__pycache__"
            | "build"
            | ".build"
            | ".cache"
            | "DerivedData"
            | ".swiftpm"
            | ".gradle"
    )
}

/// Recursively calculates the total size in bytes of a directory tree.
pub fn tree_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.file_type().is_symlink() {
        return metadata.len();
    }
    if !metadata.is_dir() {
        return metadata.len();
    }
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            total += tree_size(&entry.path());
        }
    }
    total
}

/// Seeds SwiftPM package checkouts, bare repositories, binary artifacts, workspace state,
/// and links `.build/ModuleCache` to the node's shared Swift module cache directory (Roadmap 3.7).
///
/// Returns `Ok(Some(bytes_seeded))` if any Swift packages were found and initialized,
/// or `Ok(None)` if no Swift packages exist in `from`.
pub fn seed_swift_worktree(from: &Path, to: &Path) -> io::Result<Option<u64>> {
    seed_swift_worktree_within(from, to, crate::disk_space(to))
}

/// Seeds SwiftPM package checkouts, bare repositories, binary artifacts, workspace state,
/// and links `.build/ModuleCache` to the node's shared Swift module cache directory (Roadmap 3.7),
/// respecting the filesystem disk space budget `space`.
pub fn seed_swift_worktree_within(
    from: &Path,
    to: &Path,
    space: Option<crate::DiskSpace>,
) -> io::Result<Option<u64>> {
    let packages = find_swift_packages(from);
    if packages.is_empty() {
        return Ok(None);
    }

    let from_str = from.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "from is not valid UTF-8")
    })?;
    let to_str = to.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "to is not valid UTF-8")
    })?;

    let shared_module_cache = swift_module_cache_dir();
    let mut total_bytes = 0u64;
    let mut any_seeded = false;

    let heavy_names = &["checkouts", "repositories", "artifacts"];
    let mut heavy_size = 0u64;

    for rel in &packages {
        let from_pkg = from.join(rel);
        let to_pkg = to.join(rel);

        let from_build = from_pkg.join(".build");
        let to_build = to_pkg.join(".build");

        if from_build.is_dir() {
            for &sub_name in heavy_names {
                let from_sub = from_build.join(sub_name);
                let to_sub = to_build.join(sub_name);
                if from_sub.is_dir() && !to_sub.exists() {
                    heavy_size += tree_size(&from_sub);
                }
            }
        }
    }

    let heavy_fits = if heavy_size > 0 {
        crate::seed_fits("SwiftPM checkouts and artifacts", heavy_size, space)
    } else {
        true
    };

    for rel in packages {
        let from_pkg = from.join(&rel);
        let to_pkg = to.join(&rel);

        let from_build = from_pkg.join(".build");
        let to_build = to_pkg.join(".build");

        // Ensure target .build directory exists
        fs::create_dir_all(&to_build)?;

        // 1. Establish shared ModuleCache symlink: .build/ModuleCache -> shared_module_cache
        let to_module_cache = to_build.join("ModuleCache");
        link_shared_module_cache(&from_build, &to_module_cache, &shared_module_cache)?;
        any_seeded = true;

        // 2. Also ensure any architecture-specific ModuleCache links to the shared cache
        // e.g. .build/arm64-apple-macosx/debug/ModuleCache or .build/x86_64-apple-macosx/debug/ModuleCache
        link_triple_module_caches(&from_build, &to_build, &shared_module_cache)?;

        // 3. Seed package checkouts, bare repositories, and binary artifacts if from_build exists and fits
        if from_build.is_dir() {
            if heavy_fits {
                for &sub_name in heavy_names {
                    let from_sub = from_build.join(sub_name);
                    let to_sub = to_build.join(sub_name);

                    if from_sub.is_dir() && !to_sub.exists() {
                        let sub_bytes = match copy_dir_preserving(&from_sub, &to_sub) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                if to_sub.exists() {
                                    let _ = fs::remove_dir_all(&to_sub);
                                }
                                return Err(e);
                            }
                        };
                        total_bytes += sub_bytes;
                        any_seeded = true;
                    }
                }
            }

            // 4. Relocate and seed workspace-state.json
            let from_state = from_build.join("workspace-state.json");
            let to_state = to_build.join("workspace-state.json");
            if from_state.is_file() {
                if let Ok(state_content) = fs::read_to_string(&from_state) {
                    if let Ok(relocated_state) =
                        relocate_swiftpm_workspace_state(&state_content, from_str, to_str)
                    {
                        fs::write(&to_state, relocated_state.as_bytes())?;
                        total_bytes += relocated_state.len() as u64;
                        any_seeded = true;
                    }
                }
            }
        }

        // 5. Seed .swiftpm configuration if present
        let from_swiftpm = from_pkg.join(".swiftpm");
        let to_swiftpm = to_pkg.join(".swiftpm");
        if from_swiftpm.is_dir() && !to_swiftpm.exists() {
            let bytes = match copy_dir_preserving(&from_swiftpm, &to_swiftpm) {
                Ok(b) => b,
                Err(e) => {
                    if to_swiftpm.exists() {
                        let _ = fs::remove_dir_all(&to_swiftpm);
                    }
                    return Err(e);
                }
            };
            total_bytes += bytes;
            any_seeded = true;
        }
    }

    if any_seeded {
        Ok(Some(total_bytes))
    } else {
        Ok(None)
    }
}

/// Links `to_module_cache` to `shared_module_cache`.
///
/// If `from_build/ModuleCache` exists and is a regular directory (not a symlink to shared cache),
/// copies any precompiled `.pcm` or `.swiftmodule` cache files into `shared_module_cache`
/// before establishing the link, preserving existing warm compilation products.
fn link_shared_module_cache(
    from_build: &Path,
    to_module_cache: &Path,
    shared_module_cache: &Path,
) -> io::Result<()> {
    let from_module_cache = from_build.join("ModuleCache");
    if from_module_cache.is_dir() {
        // If from has a real directory with cached modules, merge them into shared_module_cache
        if let Ok(meta) = fs::symlink_metadata(&from_module_cache) {
            if !meta.file_type().is_symlink() {
                let _ = merge_cache_files(&from_module_cache, shared_module_cache);
            }
        }
    }

    // If to_module_cache is already a symlink pointing to shared_module_cache, keep it
    if let Ok(meta) = fs::symlink_metadata(to_module_cache) {
        if meta.file_type().is_symlink() {
            if let Ok(target) = fs::read_link(to_module_cache) {
                if target == shared_module_cache {
                    return Ok(());
                }
            }
            let _ = fs::remove_file(to_module_cache);
        } else if meta.is_dir() {
            let _ = merge_cache_files(to_module_cache, shared_module_cache);
            let _ = fs::remove_dir_all(to_module_cache);
        }
    }

    // Create symlink
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(shared_module_cache, to_module_cache)?;
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(shared_module_cache, to_module_cache)?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        fs::create_dir_all(to_module_cache)?;
    }

    Ok(())
}

/// Discovers any triple-specific ModuleCache directories (e.g. `<triple>/debug/ModuleCache`)
/// and establishes symlinks to `shared_module_cache`.
fn link_triple_module_caches(
    from_build: &Path,
    to_build: &Path,
    shared_module_cache: &Path,
) -> io::Result<()> {
    if !from_build.is_dir() {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(from_build) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || matches!(name_str.as_ref(), "checkouts" | "repositories" | "artifacts" | "ModuleCache") {
            continue;
        }
        let from_triple = entry.path();
        if !from_triple.is_dir() {
            continue;
        }
        for profile in &["debug", "release"] {
            let from_profile = from_triple.join(profile);
            let from_cache = from_profile.join("ModuleCache");
            if from_cache.is_dir() {
                let to_profile = to_build.join(name.as_os_str()).join(profile);
                fs::create_dir_all(&to_profile)?;
                let to_cache = to_profile.join("ModuleCache");
                let _ = link_shared_module_cache(&from_profile, &to_cache, shared_module_cache);
            }
        }
    }
    Ok(())
}

/// Merges non-hidden cache files from `src_dir` into `dst_dir`.
fn merge_cache_files(src_dir: &Path, dst_dir: &Path) -> io::Result<u64> {
    let mut copied = 0u64;
    let Ok(entries) = fs::read_dir(src_dir) else {
        return Ok(0);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let target = dst_dir.join(&name);
        if path.is_file() && !target.exists() {
            if let Ok(bytes) = fs::copy(&path, &target) {
                copied += bytes;
            }
        } else if path.is_dir() {
            fs::create_dir_all(&target)?;
            copied += merge_cache_files(&path, &target)?;
        }
    }
    Ok(copied)
}

/// Copies a directory tree preserving modification times and symbolic links.
fn copy_dir_preserving(src: &Path, dst: &Path) -> io::Result<u64> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        let mut cmd = std::process::Command::new("cp");
        cmd.scrub_cluster_secrets();
        let status = cmd
            .arg("-a")
            .arg(src)
            .arg(dst)
            .status();
        match status {
            Ok(s) if s.success() => Ok(tree_size(dst)),
            Ok(s) => {
                if dst.exists() {
                    let _ = fs::remove_dir_all(dst);
                }
                Err(io::Error::other(format!(
                    "copying {} to {} failed: {s}",
                    src.display(),
                    dst.display()
                )))
            }
            Err(e) => {
                if dst.exists() {
                    let _ = fs::remove_dir_all(dst);
                }
                Err(e)
            }
        }
    }

    #[cfg(not(unix))]
    {
        match copy_dir_fallback(src, dst) {
            Ok(size) => Ok(size),
            Err(e) => {
                if dst.exists() {
                    let _ = fs::remove_dir_all(dst);
                }
                Err(e)
            }
        }
    }
}

#[cfg(not(unix))]
fn copy_dir_fallback(src: &Path, dst: &Path) -> io::Result<u64> {
    let mut total = 0u64;
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            total += copy_dir_fallback(&from, &to)?;
        } else if from.is_file() {
            total += fs::copy(&from, &to)?;
        }
    }
    Ok(total)
}

/// Evicts stale `.pcm` and `.swiftmodule` cache files from `swift_module_cache_dir()`
/// that are older than `max_age`, or when total cache size exceeds `max_size_bytes`
/// (least recently modified files evicted first).
///
/// Returns the number of files removed.
pub fn prune_stale_module_cache(max_age: Duration, max_size_bytes: u64) -> io::Result<usize> {
    prune_stale_module_cache_in(&swift_module_cache_dir(), max_age, max_size_bytes)
}

/// Evicts stale `.pcm` and `.swiftmodule` cache files from `cache_dir`
/// that are older than `max_age`, or when total cache size exceeds `max_size_bytes`
/// (least recently modified files evicted first).
///
/// Returns the number of files removed.
pub fn prune_stale_module_cache_in(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
) -> io::Result<usize> {
    if !cache_dir.is_dir() {
        return Ok(0);
    }

    struct CacheFile {
        path: PathBuf,
        size: u64,
        modified: SystemTime,
    }

    let now = SystemTime::now();
    let mut files = Vec::new();
    let mut total_size = 0u64;

    fn collect_files(
        dir: &Path,
        files: &mut Vec<CacheFile>,
        total_size: &mut u64,
    ) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let meta = entry.metadata()?;
            if meta.is_dir() {
                collect_files(&path, files, total_size)?;
            } else if meta.is_file() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                // Never remove lock files or active sentinel files
                if name.ends_with(".lock") || name.starts_with('.') {
                    continue;
                }
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let size = meta.len();
                *total_size += size;
                files.push(CacheFile { path, size, modified });
            }
        }
        Ok(())
    }

    let _ = collect_files(cache_dir, &mut files, &mut total_size);

    let mut removed = 0;

    // 1. Remove files older than max_age
    files.retain(|f| {
        if let Ok(age) = now.duration_since(f.modified) {
            if age > max_age {
                if fs::remove_file(&f.path).is_ok() {
                    total_size = total_size.saturating_sub(f.size);
                    removed += 1;
                    return false;
                }
            }
        }
        true
    });

    // 2. If total size still exceeds max_size_bytes, evict oldest first
    if total_size > max_size_bytes {
        files.sort_by_key(|f| f.modified);
        for f in files {
            if total_size <= max_size_bytes {
                break;
            }
            if fs::remove_file(&f.path).is_ok() {
                total_size = total_size.saturating_sub(f.size);
                removed += 1;
            }
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_swift_module_cache_env_and_path() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let custom_cache = temp.path().join("my-custom-swift-cache");

        // Use custom env
        unsafe { std::env::set_var(SWIFT_MODULE_CACHE_ENV, &custom_cache); }
        let dir = swift_module_cache_dir();
        assert_eq!(dir, custom_cache);
        assert!(dir.is_dir());

        let envs = swift_module_cache_env();
        assert!(envs
            .iter()
            .any(|(k, v)| k == "SWIFTPM_MODULECACHE_OVERRIDE" && v == custom_cache.to_str().unwrap()));
        assert!(envs
            .iter()
            .any(|(k, v)| k == "SWIFT_MODULE_CACHE_PATH" && v == custom_cache.to_str().unwrap()));
        assert!(envs
            .iter()
            .any(|(k, v)| k == "CLANG_MODULE_CACHE_PATH" && v == custom_cache.to_str().unwrap()));

        unsafe { std::env::remove_var(SWIFT_MODULE_CACHE_ENV); }
    }

    #[test]
    fn test_relocate_swiftpm_workspace_state_component_boundaries() {
        let from_root = "/Users/developer/prod-code";
        let to_root = "/tmp/ram-disk/prod-code-worktree-1";

        let sample_json = r#"{
  "object": {
    "artifacts": [],
    "dependencies": [
      {
        "packageRef": {
          "identity": "myswiftpkg",
          "kind": "localSourceControl",
          "location": "/Users/developer/prod-code/packages/myswiftpkg",
          "name": "MySwiftPkg"
        },
        "state": {
          "name": "localSourceControl",
          "path": "/Users/developer/prod-code/.build/checkouts/myswiftpkg"
        },
        "subpath": "myswiftpkg"
      },
      {
        "packageRef": {
          "identity": "external-dep",
          "kind": "localSourceControl",
          "location": "/Users/developer/prod-code-external-deps/external-dep",
          "name": "ExternalDep"
        },
        "state": {
          "name": "localSourceControl",
          "path": "/Users/developer/prod-code-sibling/external-dep"
        },
        "subpath": "external-dep"
      }
    ]
  },
  "version": 1
}"#;

        let relocated =
            relocate_swiftpm_workspace_state(sample_json, from_root, to_root).unwrap();

        // Target path under from_root should be relocated
        assert!(relocated.contains("/tmp/ram-disk/prod-code-worktree-1/packages/myswiftpkg"));
        assert!(relocated.contains("/tmp/ram-disk/prod-code-worktree-1/.build/checkouts/myswiftpkg"));

        // Sibling paths starting with prefix substrings MUST NOT be changed
        assert!(relocated.contains("/Users/developer/prod-code-external-deps/external-dep"));
        assert!(relocated.contains("/Users/developer/prod-code-sibling/external-dep"));
        assert!(!relocated.contains("/tmp/ram-disk/prod-code-worktree-1-external-deps"));
        assert!(!relocated.contains("/tmp/ram-disk/prod-code-worktree-1-sibling"));
    }

    #[test]
    fn test_find_swift_packages_root_and_nested() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // Root package
        fs::write(root.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();

        // Nested package in clients/macos/ProdUI
        let macos_pkg = root.join("clients").join("macos").join("ProdUI");
        fs::create_dir_all(&macos_pkg).unwrap();
        fs::write(macos_pkg.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();

        // Ignored package inside .build or target
        let ignored_pkg = root.join(".build").join("checkouts").join("ignored");
        fs::create_dir_all(&ignored_pkg).unwrap();
        fs::write(ignored_pkg.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();

        let found = find_swift_packages(root);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], PathBuf::from(""));
        assert_eq!(found[1], PathBuf::from("clients/macos/ProdUI"));
    }

    #[test]
    fn test_seed_swift_worktree_basic() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("seed-workspace");
        let to = temp.path().join("new-worktree");

        fs::create_dir_all(&from).unwrap();
        fs::write(from.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();

        let from_build = from.join(".build");
        fs::create_dir_all(&from_build).unwrap();

        // Mock checkouts
        let checkout_dir = from_build.join("checkouts").join("MyLib");
        fs::create_dir_all(&checkout_dir).unwrap();
        fs::write(checkout_dir.join("MyLib.swift"), "public let x = 42;\n").unwrap();

        // Mock repositories
        let repo_dir = from_build.join("repositories").join("MyLib-hash");
        fs::create_dir_all(&repo_dir).unwrap();
        fs::write(repo_dir.join("config"), "bare git repo mock\n").unwrap();

        // Mock workspace-state.json
        let from_str = from.to_str().unwrap();
        let state_content = format!(
            r#"{{"object":{{"artifacts":[],"dependencies":[{{"state":{{"path":"{from_str}/.build/checkouts/MyLib"}}}}]}}}}"#
        );
        fs::write(from_build.join("workspace-state.json"), state_content).unwrap();

        // Run seed
        let result = seed_swift_worktree(&from, &to).unwrap();
        assert!(result.is_some());

        let to_build = to.join(".build");
        assert!(to_build.is_dir());

        // Verify checkouts copied
        assert_eq!(
            fs::read_to_string(to_build.join("checkouts").join("MyLib").join("MyLib.swift")).unwrap(),
            "public let x = 42;\n"
        );

        // Verify repositories copied
        assert!(to_build.join("repositories").join("MyLib-hash").join("config").is_file());

        // Verify workspace-state.json relocated
        let to_state = fs::read_to_string(to_build.join("workspace-state.json")).unwrap();
        let to_str = to.to_str().unwrap();
        assert!(to_state.contains(&format!("{to_str}/.build/checkouts/MyLib")));
        assert!(!to_state.contains(&format!("{from_str}/.build/checkouts/MyLib")));

        // Verify ModuleCache symlink
        let to_module_cache = to_build.join("ModuleCache");
        assert!(fs::symlink_metadata(&to_module_cache).unwrap().file_type().is_symlink());
        let link_target = fs::read_link(&to_module_cache).unwrap();
        assert_eq!(link_target, swift_module_cache_dir());
    }

    #[test]
    fn test_seed_swift_worktree_skips_non_swift_projects() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("rust-workspace");
        let to = temp.path().join("new-worktree");

        fs::create_dir_all(&from).unwrap();
        fs::write(from.join("Cargo.toml"), "[package]\nname=\"foo\"\n").unwrap();

        let result = seed_swift_worktree(&from, &to).unwrap();
        assert!(result.is_none());
        assert!(!to.exists());
    }

    #[test]
    fn test_prune_stale_module_cache() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let custom_cache = temp.path().join("prune-cache");
        unsafe { std::env::set_var(SWIFT_MODULE_CACHE_ENV, &custom_cache); }

        ensure_cache_dir(&custom_cache).unwrap();

        let old_file = custom_cache.join("old-module.pcm");
        fs::write(&old_file, vec![0u8; 1024]).unwrap();

        let new_file = custom_cache.join("new-module.pcm");
        fs::write(&new_file, vec![0u8; 1024]).unwrap();

        // Pruning with max_size_bytes=1500 will evict one file
        let pruned = prune_stale_module_cache(Duration::from_secs(3600), 1500).unwrap();
        assert_eq!(pruned, 1);

        // Direct directory pruning
        fs::write(&old_file, vec![0u8; 1024]).unwrap();
        let pruned_in = prune_stale_module_cache_in(&custom_cache, Duration::from_secs(3600), 1500).unwrap();
        assert_eq!(pruned_in, 1);

        unsafe { std::env::remove_var(SWIFT_MODULE_CACHE_ENV); }
    }
}
