//! Cross-worktree Swift engine support: shared Swift/Clang ModuleCache,
//! SwiftPM package checkout and artifact seeding, and compilation workspace
//! state relocation (Roadmap 3.7).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::cpp_index::relocate_path_or_uri;
use prod_code_protocol::transport::ScrubSecrets;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(1);

/// The environment variable name used to explicitly configure the shared Swift module cache directory.
pub const SWIFT_MODULE_CACHE_ENV: &str = "PROD_CODE_SWIFT_MODULE_CACHE";

/// Returns the path to the shared Swift and Clang module cache directory.
///
/// Precedence:
/// 1. `PROD_CODE_SWIFT_MODULE_CACHE` environment variable if set.
/// 2. `SWIFTPM_MODULECACHE_OVERRIDE` environment variable if already configured.
/// 3. `$HOME/.cache/prod-code/swift-module-cache`.
/// 4. A UID-specific cache directory under `/var/tmp`.
/// 5. A process-specific fallback under the system temporary directory.
///
/// Ensures the directory exists with mode `0700` on Unix systems.
fn resolve_cache_dir(path: PathBuf) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    if path.file_name().is_none() || std::env::current_dir().ok().as_deref() == Some(path.as_path())
    {
        return None;
    }
    match ensure_cache_dir(&path) {
        Ok(()) => Some(path),
        Err(error) => {
            tracing::warn!(cache = %path.display(), %error, "rejecting insecure Swift module cache directory");
            None
        }
    }
}

pub fn swift_module_cache_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os(SWIFT_MODULE_CACHE_ENV) {
        if let Some(dir) = resolve_cache_dir(PathBuf::from(custom)) {
            return dir;
        }
    }

    if let Some(override_path) = std::env::var_os("SWIFTPM_MODULECACHE_OVERRIDE") {
        if let Some(dir) = resolve_cache_dir(PathBuf::from(override_path)) {
            return dir;
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home)
            .join(".cache")
            .join("prod-code")
            .join("swift-module-cache");
        if let Some(dir) = resolve_cache_dir(p) {
            return dir;
        }
    }

    let user = cache_user_suffix();
    let var_tmp = PathBuf::from("/var/tmp").join(format!("prod-code-swift-module-cache-{user}"));
    if let Some(dir) = resolve_cache_dir(var_tmp) {
        return dir;
    }

    let temp = std::env::temp_dir().join(format!(
        "prod-code-swift-module-cache-{user}-{}",
        std::process::id()
    ));
    resolve_cache_dir(temp).expect("no private Swift module cache directory could be created")
}

fn cache_user_suffix() -> String {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() }.to_string()
    }
    #[cfg(not(unix))]
    {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "default".to_string())
    }
}

/// Ensures `dir` exists and has secure permissions (`0700` on Unix).
pub fn ensure_cache_dir(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() || dir.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid cache directory path",
        ));
    }
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::MetadataExt;
        let c_path = std::ffi::CString::new(dir.as_os_str().as_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd was returned by open and ownership is transferred to File.
        let directory = unsafe { std::fs::File::from_raw_fd(fd) };
        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Swift module cache directory is not owned by the current user",
            ));
        }
        if metadata.mode() & 0o077 != 0
            && unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let secured = directory.metadata()?;
        if !secured.is_dir()
            || secured.uid() != unsafe { libc::geteuid() }
            || secured.mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Swift module cache directory could not be secured to mode 0700",
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let metadata = fs::symlink_metadata(dir)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Swift module cache path is not a real directory",
            ));
        }
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
        serde_json::to_string_pretty(&val)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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

        if name_str == "Package.swift" && entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
        {
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

fn real_module_cache_size(path: &Path) -> u64 {
    fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .map(|_| tree_size(path))
        .unwrap_or(0)
}

fn module_cache_size_in_build(build: &Path) -> u64 {
    let mut total = real_module_cache_size(&build.join("ModuleCache"));
    let Ok(entries) = fs::read_dir(build) else {
        return total;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        for profile in ["debug", "release"] {
            total += real_module_cache_size(&path.join(profile).join("ModuleCache"));
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

    let from_str = from
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "from is not valid UTF-8"))?;
    let to_str = to
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "to is not valid UTF-8"))?;

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

    let module_cache_size: u64 = packages
        .iter()
        .map(|rel| {
            module_cache_size_in_build(&from.join(rel).join(".build"))
                + module_cache_size_in_build(&to.join(rel).join(".build"))
        })
        .sum();
    let total_seed_size = heavy_size.saturating_add(module_cache_size);
    let cache_seed_fits = total_seed_size == 0
        || crate::seed_fits(
            "SwiftPM dependency and module caches",
            total_seed_size,
            space,
        );
    let shared_cache_fits = module_cache_size == 0
        || crate::seed_fits(
            "shared Swift module cache",
            module_cache_size,
            crate::disk_space(&shared_module_cache),
        );
    let heavy_fits = cache_seed_fits;
    let module_cache_fits = cache_seed_fits && shared_cache_fits;

    for rel in packages {
        let from_pkg = from.join(&rel);
        let to_pkg = to.join(&rel);

        let from_build = from_pkg.join(".build");
        let to_build = to_pkg.join(".build");

        // Ensure target .build directory exists
        fs::create_dir_all(&to_build)?;

        // 1. Link module caches only when the complete source/target cache set fits the budget.
        if module_cache_fits {
            let to_module_cache = to_build.join("ModuleCache");
            link_shared_module_cache(&from_build, &to_module_cache, &shared_module_cache)?;
            any_seeded = true;

            // Also link triple-specific caches such as debug/ModuleCache.
            link_triple_module_caches(&from_build, &to_build, &shared_module_cache)?;
        } else {
            tracing::info!(
                workspace = %to_pkg.display(),
                cache_bytes = module_cache_size,
                "Swift module cache exceeds disk budget; leaving local module caches in place"
            );
        }

        // 3. Seed package checkouts, bare repositories, and binary artifacts if from_build exists and fits
        if from_build.is_dir() {
            if heavy_fits {
                for &sub_name in heavy_names {
                    let from_sub = from_build.join(sub_name);
                    let to_sub = to_build.join(sub_name);

                    if from_sub.is_dir() && !to_sub.exists() {
                        let sub_bytes = copy_dir_preserving(&from_sub, &to_sub)?;
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
            let bytes = copy_dir_preserving(&from_swiftpm, &to_swiftpm)?;
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
                merge_cache_files(&from_module_cache, shared_module_cache)?;
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
            merge_cache_files(to_module_cache, shared_module_cache)?;
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
        if name_str.starts_with('.')
            || matches!(
                name_str.as_ref(),
                "checkouts" | "repositories" | "artifacts" | "ModuleCache"
            )
        {
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
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let target = dst_dir.join(&name);
        let target_metadata = fs::symlink_metadata(&target).ok();
        if target_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            continue;
        }
        if metadata.is_file() && target_metadata.is_none() {
            copied += fs::copy(&path, &target)?;
        } else if metadata.is_dir() {
            fs::create_dir_all(&target)?;
            copied += merge_cache_files(&path, &target)?;
        }
    }
    Ok(copied)
}

/// Copies a directory tree preserving modification times and symbolic links.
/// Uses a unique per-attempt staging holder directory (created with exclusive mkdir)
/// and atomic rename to ensure concurrent seeding attempts never clobber, nest into,
/// or delete each other's destinations (#834).
fn copy_dir_preserving(src: &Path, dst: &Path) -> io::Result<u64> {
    if dst.exists() {
        return Ok(tree_size(dst));
    }

    let parent = dst
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"))?;
    fs::create_dir_all(parent)?;

    let dst_name = dst.file_name().and_then(|n| n.to_str()).unwrap_or("dir");

    // Exclusively create a unique staging holder directory.
    // If the directory already exists (e.g. from an earlier crashed process),
    // loop and retry with a new unique nonce.
    let mut attempts = 0;
    let (holder, staging_dst) = loop {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            ^ ((NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed) as u128) << 64)
            ^ ((std::process::id() as u128) << 32);
        let holder_path = parent.join(format!(".staging-holder-{dst_name}-{nonce:032x}"));
        match fs::create_dir(&holder_path) {
            Ok(()) => {
                let staging_path = holder_path.join("content");
                break (holder_path, staging_path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                attempts += 1;
                if attempts > 1000 {
                    return Err(io::Error::other(
                        "exhausted attempts to create exclusive staging directory",
                    ));
                }
            }
            Err(e) => return Err(e),
        }
    };

    #[cfg(unix)]
    let copy_result = {
        let mut cmd = std::process::Command::new("cp");
        cmd.scrub_cluster_secrets();
        let status = cmd.arg("-a").arg(src).arg(&staging_dst).status();
        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(io::Error::other(format!(
                "copying {} to {} failed: {s}",
                src.display(),
                staging_dst.display()
            ))),
            Err(e) => Err(e),
        }
    };

    #[cfg(not(unix))]
    let copy_result = copy_dir_fallback(src, &staging_dst).map(|_| ());

    if let Err(e) = copy_result {
        let _ = fs::remove_dir_all(&holder);
        return Err(e);
    }

    // Try atomic rename from staging_dst to dst (same filesystem, since holder is in parent)
    if let Err(e) = fs::rename(&staging_dst, dst) {
        // If dst was already populated concurrently, clean up holder and accept dst
        let _ = fs::remove_dir_all(&holder);
        if dst.exists() {
            return Ok(tree_size(dst));
        }
        return Err(e);
    }

    // Clean up empty holder directory
    let _ = fs::remove_dir(&holder);

    Ok(tree_size(dst))
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
/// Uses directory-handle-relative traversal and removal with `O_NOFOLLOW` / `AT_SYMLINK_NOFOLLOW`
/// to eliminate symlink TOCTOU races and guarantee no files outside `cache_dir` can ever be
/// traversed or unlinked (#834).
/// Returns the number of files removed.
pub fn prune_stale_module_cache_in(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
) -> io::Result<usize> {
    if !cache_dir.is_dir() {
        return Ok(0);
    }

    #[cfg(unix)]
    {
        unix_pruner::prune_unix(cache_dir, max_age, max_size_bytes)
    }

    #[cfg(not(unix))]
    {
        prune_fallback(cache_dir, max_age, max_size_bytes)
    }
}

#[cfg(unix)]
mod unix_pruner {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    pub struct CacheEntry {
        pub rel_components: Vec<std::ffi::CString>,
        pub file_name: std::ffi::CString,
        pub size: u64,
        pub modified: SystemTime,
    }

    pub fn prune_unix(
        cache_dir: &Path,
        max_age: Duration,
        max_size_bytes: u64,
    ) -> io::Result<usize> {
        let parent = cache_dir
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let Some(name) = cache_dir.file_name() else {
            return Ok(0);
        };
        let canonical_parent = match fs::canonicalize(parent) {
            Ok(parent) => parent,
            Err(_) => return Ok(0),
        };
        let root_path = canonical_parent.join(name);
        let expected_meta = match fs::symlink_metadata(&root_path) {
            Ok(m) => m,
            Err(_) => return Ok(0),
        };
        if expected_meta.file_type().is_symlink() || !expected_meta.file_type().is_dir() {
            return Ok(0);
        }

        let c_root = std::ffi::CString::new(root_path.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let root_fd = unsafe {
            libc::open(
                c_root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if root_fd < 0 {
            return Ok(0);
        }

        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(root_fd, &mut st) } != 0 {
            unsafe {
                libc::close(root_fd);
            }
            return Ok(0);
        }

        use std::os::unix::fs::MetadataExt;
        if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR
            || (st.st_dev as u64) != expected_meta.dev()
            || (st.st_ino as u64) != expected_meta.ino()
        {
            unsafe {
                libc::close(root_fd);
            }
            return Ok(0);
        }

        let mut files = Vec::new();
        let mut total_size = 0u64;
        let mut rel_components = Vec::new();

        unsafe {
            collect_dir(root_fd, &mut rel_components, &mut files, &mut total_size);
        }

        let now = SystemTime::now();
        let mut removed = 0;

        // 1. Remove files older than max_age
        files.retain(|f| {
            if let Ok(age) = now.duration_since(f.modified) {
                if age > max_age {
                    if remove_entry(root_fd, f) {
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
            for f in &files {
                if total_size <= max_size_bytes {
                    break;
                }
                if remove_entry(root_fd, f) {
                    total_size = total_size.saturating_sub(f.size);
                    removed += 1;
                }
            }
        }

        unsafe {
            libc::close(root_fd);
        }

        Ok(removed)
    }

    unsafe fn collect_dir(
        current_fd: libc::c_int,
        rel_components: &mut Vec<std::ffi::CString>,
        files: &mut Vec<CacheEntry>,
        total_size: &mut u64,
    ) {
        unsafe {
            let dup_fd = libc::dup(current_fd);
            if dup_fd < 0 {
                return;
            }
            let dir_stream = libc::fdopendir(dup_fd);
            if dir_stream.is_null() {
                libc::close(dup_fd);
                return;
            }

            loop {
                let entry = libc::readdir(dir_stream);
                if entry.is_null() {
                    break;
                }
                let name_ptr = (*entry).d_name.as_ptr();
                let name = std::ffi::CStr::from_ptr(name_ptr);
                let bytes = name.to_bytes();
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                if bytes.starts_with(b".") || bytes.ends_with(b".lock") {
                    continue;
                }

                let mut st: libc::stat = std::mem::zeroed();
                if libc::fstatat(
                    current_fd,
                    name.as_ptr(),
                    &mut st,
                    libc::AT_SYMLINK_NOFOLLOW,
                ) != 0
                {
                    continue;
                }

                let mode = st.st_mode & libc::S_IFMT;
                if mode == libc::S_IFLNK {
                    continue;
                }
                if mode == libc::S_IFDIR {
                    let child_fd = libc::openat(
                        current_fd,
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    );
                    if child_fd >= 0 {
                        rel_components.push(name.to_owned());
                        collect_dir(child_fd, rel_components, files, total_size);
                        rel_components.pop();
                        libc::close(child_fd);
                    }
                } else if mode == libc::S_IFREG {
                    let modified =
                        SystemTime::UNIX_EPOCH + Duration::from_secs(st.st_mtime.max(0) as u64);
                    let size = st.st_size as u64;
                    *total_size += size;
                    files.push(CacheEntry {
                        rel_components: rel_components.clone(),
                        file_name: name.to_owned(),
                        size,
                        modified,
                    });
                }
            }

            libc::closedir(dir_stream);
        }
    }

    fn remove_entry(root_fd: libc::c_int, entry: &CacheEntry) -> bool {
        let mut current_fd = root_fd;
        let mut fds_to_close = Vec::new();

        for comp in &entry.rel_components {
            let next_fd = unsafe {
                libc::openat(
                    current_fd,
                    comp.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if next_fd < 0 {
                for fd in fds_to_close {
                    unsafe {
                        libc::close(fd);
                    }
                }
                return false;
            }
            fds_to_close.push(next_fd);
            current_fd = next_fd;
        }

        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let is_reg = unsafe {
            libc::fstatat(
                current_fd,
                entry.file_name.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            ) == 0
                && (st.st_mode & libc::S_IFMT) == libc::S_IFREG
        };

        let removed = if is_reg {
            unsafe { libc::unlinkat(current_fd, entry.file_name.as_ptr(), 0) == 0 }
        } else {
            false
        };

        for fd in fds_to_close {
            unsafe {
                libc::close(fd);
            }
        }

        removed
    }
}

#[cfg(not(unix))]
fn prune_fallback(
    _cache_dir: &Path,
    _max_age: Duration,
    _max_size_bytes: u64,
) -> io::Result<usize> {
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(unix)]
    #[test]
    fn prune_rejects_a_symlink_root_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        let keep = outside.join("keep.module");
        fs::write(&keep, b"module cache").unwrap();
        let cache_link = temp.path().join("cache-link");
        symlink(&outside, &cache_link).unwrap();

        assert_eq!(
            prune_stale_module_cache_in(&cache_link, Duration::ZERO, 0).unwrap(),
            0
        );
        assert!(keep.exists());
    }

    #[cfg(unix)]
    #[test]
    fn cache_directory_rejects_symlinks_and_secures_owned_directories() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temp = tempfile::tempdir().unwrap();
        let owned = temp.path().join("owned-cache");
        fs::create_dir(&owned).unwrap();
        fs::set_permissions(&owned, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_cache_dir(&owned).unwrap();
        assert_eq!(
            fs::metadata(&owned).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let target = temp.path().join("external");
        fs::create_dir(&target).unwrap();
        let target_mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        let link = temp.path().join("cache-link");
        symlink(&target, &link).unwrap();
        assert!(ensure_cache_dir(&link).is_err());
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            target_mode
        );
    }

    #[cfg(unix)]
    #[test]
    fn merging_module_cache_skips_symlink_files_and_directories() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        let dst = temp.path().join("dst");
        let external = temp.path().join("external");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(external.join("private.pcm"), b"private").unwrap();
        symlink(&external, src.join("linked-dir")).unwrap();
        symlink(external.join("private.pcm"), src.join("linked-file.pcm")).unwrap();

        merge_cache_files(&src, &dst).unwrap();

        assert!(!dst.join("linked-dir").exists());
        assert!(!dst.join("linked-file.pcm").exists());
        assert!(external.join("private.pcm").exists());
    }

    #[test]
    fn test_swift_module_cache_env_and_path() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let custom_cache = temp.path().join("my-custom-swift-cache");

        // Use custom env
        unsafe {
            std::env::set_var(SWIFT_MODULE_CACHE_ENV, &custom_cache);
        }
        let dir = swift_module_cache_dir();
        assert_eq!(dir, custom_cache);
        assert!(dir.is_dir());

        let envs = swift_module_cache_env();
        assert!(envs.iter().any(
            |(k, v)| k == "SWIFTPM_MODULECACHE_OVERRIDE" && v == custom_cache.to_str().unwrap()
        ));
        assert!(envs
            .iter()
            .any(|(k, v)| k == "SWIFT_MODULE_CACHE_PATH" && v == custom_cache.to_str().unwrap()));
        assert!(envs
            .iter()
            .any(|(k, v)| k == "CLANG_MODULE_CACHE_PATH" && v == custom_cache.to_str().unwrap()));

        unsafe {
            std::env::remove_var(SWIFT_MODULE_CACHE_ENV);
        }
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

        let relocated = relocate_swiftpm_workspace_state(sample_json, from_root, to_root).unwrap();

        // Target path under from_root should be relocated
        assert!(relocated.contains("/tmp/ram-disk/prod-code-worktree-1/packages/myswiftpkg"));
        assert!(
            relocated.contains("/tmp/ram-disk/prod-code-worktree-1/.build/checkouts/myswiftpkg")
        );

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
        fs::write(
            macos_pkg.join("Package.swift"),
            "// swift-tools-version:5.9\n",
        )
        .unwrap();

        // Ignored package inside .build or target
        let ignored_pkg = root.join(".build").join("checkouts").join("ignored");
        fs::create_dir_all(&ignored_pkg).unwrap();
        fs::write(
            ignored_pkg.join("Package.swift"),
            "// swift-tools-version:5.9\n",
        )
        .unwrap();

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
            fs::read_to_string(to_build.join("checkouts").join("MyLib").join("MyLib.swift"))
                .unwrap(),
            "public let x = 42;\n"
        );

        // Verify repositories copied
        assert!(
            to_build
                .join("repositories")
                .join("MyLib-hash")
                .join("config")
                .is_file()
        );

        // Verify workspace-state.json relocated
        let to_state = fs::read_to_string(to_build.join("workspace-state.json")).unwrap();
        let to_str = to.to_str().unwrap();
        assert!(to_state.contains(&format!("{to_str}/.build/checkouts/MyLib")));
        assert!(!to_state.contains(&format!("{from_str}/.build/checkouts/MyLib")));

        // Verify ModuleCache symlink
        let to_module_cache = to_build.join("ModuleCache");
        assert!(
            fs::symlink_metadata(&to_module_cache)
                .unwrap()
                .file_type()
                .is_symlink()
        );
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
        unsafe {
            std::env::set_var(SWIFT_MODULE_CACHE_ENV, &custom_cache);
        }

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
        let pruned_in =
            prune_stale_module_cache_in(&custom_cache, Duration::from_secs(3600), 1500).unwrap();
        assert_eq!(pruned_in, 1);

        unsafe {
            std::env::remove_var(SWIFT_MODULE_CACHE_ENV);
        }
    }
}
