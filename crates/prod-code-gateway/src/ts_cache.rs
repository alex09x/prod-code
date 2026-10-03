//! Cross-worktree TypeScript & JavaScript engine support: shared global `@types/*`
//! and declaration cache, automated worktree type resolution, and vtsls/tsc coordination (Roadmap 3.5).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::{disk_space, seed_fits, DiskSpace};

/// The environment variable name used to explicitly configure the shared TypeScript `@types` cache directory.
pub const TS_TYPES_CACHE_ENV: &str = "PROD_CODE_TS_TYPES_CACHE";

/// Returns the path to the shared TypeScript `@types` cache directory.
///
/// Precedence:
/// 1. `PROD_CODE_TS_TYPES_CACHE` environment variable if set.
/// 2. `$HOME/.cache/prod-code/typescript-types`.
/// 3. `/var/tmp/prod-code/typescript-types` (or `/tmp/prod-code/typescript-types`).
/// 4. Temporary directory fallback (`std::env::temp_dir().join("prod-code-typescript-types")`).
///
/// Ensures the directory exists with mode `0700` on Unix systems.
pub fn ts_types_cache_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os(TS_TYPES_CACHE_ENV) {
        let p = PathBuf::from(custom);
        let _ = ensure_cache_dir(&p);
        return p;
    }

    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home)
            .join(".cache")
            .join("prod-code")
            .join("typescript-types");
        if ensure_cache_dir(&p).is_ok() {
            return p;
        }
    }

    let var_tmp = PathBuf::from("/var/tmp/prod-code/typescript-types");
    if ensure_cache_dir(&var_tmp).is_ok() {
        return var_tmp;
    }

    let temp = std::env::temp_dir().join("prod-code-typescript-types");
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

/// Returns the environment variables that configure the shared TypeScript types cache
/// across vtsls, tsserver, tsc, and TypeScript/Node tools (Roadmap 3.5).
pub fn ts_types_cache_env() -> Vec<(String, String)> {
    let dir = ts_types_cache_dir();
    let dir_str = dir.to_string_lossy().into_owned();
    vec![
        (TS_TYPES_CACHE_ENV.to_string(), dir_str.clone()),
        ("TS_TYPES_CACHE".to_string(), dir_str),
    ]
}

/// Checks whether `root` represents or contains a TypeScript or JavaScript project.
pub fn is_typescript_project(root: &Path) -> bool {
    const TS_MARKERS: &[&str] = &[
        "tsconfig.json",
        "jsconfig.json",
        "package.json",
        "deno.json",
        "deno.jsonc",
        "bunfig.toml",
    ];

    for marker in TS_MARKERS {
        if root.join(marker).is_file() {
            return true;
        }
    }

    // Check top-level or immediate subfolder source files
    let subdirs = ["src", "lib", "test", "tests", "packages", "apps", "."];
    for sub in &subdirs {
        let check_dir = if *sub == "." {
            root.to_path_buf()
        } else {
            root.join(sub)
        };
        if let Ok(entries) = fs::read_dir(&check_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts") {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Discovers candidate type declaration roots in a project.
///
/// Discovers:
/// 1. `root/node_modules/@types`
/// 2. Monorepo subpackage `node_modules/@types` (e.g. `packages/*/node_modules/@types`)
/// 3. Project-level custom `types/`, `@types/`, `typings/` containing declaration files.
/// Constrains candidate discovery to approved roots and avoids symlink cycles (#836).
pub fn find_project_types(root: &Path) -> Vec<PathBuf> {
    let project_root = find_enclosing_project_root(root);
    find_project_types_within(root, &[&project_root])
}

/// Discovers candidate type declaration roots in a project constraining candidate paths
/// and recursive declaration file checks to explicit approved roots (#836).
pub fn find_project_types_within(root: &Path, approved_roots: &[&Path]) -> Vec<PathBuf> {
    let approved = build_approved_roots(approved_roots);
    let mut type_dirs = Vec::new();

    // 1. Root node_modules/@types
    let root_types = root.join("node_modules").join("@types");
    if root_types.is_dir() && is_target_approved(&root_types, &approved) {
        type_dirs.push(root_types);
    }

    // 2. Custom local types directories
    for custom_name in &["types", "@types", "typings"] {
        let custom_dir = root.join(custom_name);
        if custom_dir.is_dir()
            && is_target_approved(&custom_dir, &approved)
            && has_declaration_files_inner(&custom_dir, &approved, &mut VisitedDirs::default())
        {
            type_dirs.push(custom_dir);
        }
    }

    // 3. Monorepo subpackages: packages/*, apps/*, libs/*
    for mono_parent in &["packages", "apps", "libs", "modules"] {
        let parent_dir = root.join(mono_parent);
        if let Ok(entries) = fs::read_dir(&parent_dir) {
            for entry in entries.flatten() {
                let pkg_dir = entry.path();
                if pkg_dir.is_dir() && is_target_approved(&pkg_dir, &approved) {
                    let sub_at_types = pkg_dir.join("node_modules").join("@types");
                    if sub_at_types.is_dir() && is_target_approved(&sub_at_types, &approved) {
                        type_dirs.push(sub_at_types);
                    }
                    for custom_name in &["types", "@types", "typings"] {
                        let sub_custom = pkg_dir.join(custom_name);
                        if sub_custom.is_dir()
                            && is_target_approved(&sub_custom, &approved)
                            && has_declaration_files_inner(
                                &sub_custom,
                                &approved,
                                &mut VisitedDirs::default(),
                            )
                        {
                            type_dirs.push(sub_custom);
                        }
                    }
                }
            }
        }
    }

    type_dirs
}

/// Checks if a directory contains any `.d.ts`, `.d.mts`, or `.d.cts` files,
/// safely dereferencing symlinks only within approved roots and tracking visited directory inodes to prevent cycles (#836).
pub fn has_declaration_files(dir: &Path) -> bool {
    let project_root = find_enclosing_project_root(dir);
    has_declaration_files_within(dir, &[&project_root])
}

/// Checks if a directory contains declaration files, constraining symlinks to explicit approved roots.
pub fn has_declaration_files_within(dir: &Path, approved_roots: &[&Path]) -> bool {
    let approved = build_approved_roots(approved_roots);
    let mut visited = VisitedDirs::default();
    has_declaration_files_inner(dir, &approved, &mut visited)
}

fn has_declaration_files_inner(
    dir: &Path,
    approved_roots: &[PathBuf],
    visited: &mut VisitedDirs,
) -> bool {
    let Some(canonical_dir) = approved_target(dir, approved_roots) else {
        return false;
    };
    if !canonical_dir.is_dir() || !visited.insert(&canonical_dir) {
        return false;
    }
    let Ok(entries) = fs::read_dir(&canonical_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }

        // Safely dereference symlinks within approved roots, using the validated canonical target
        // for subsequent operations to prevent TOCTOU symlink swaps (#836).
        let (effective_path, is_dir, is_file) = if file_type.is_symlink() {
            let Some(canon) = approved_target(&path, approved_roots) else {
                continue;
            };
            match fs::metadata(&canon) {
                Ok(meta) => (canon, meta.is_dir(), meta.is_file()),
                Err(_) => continue,
            }
        } else {
            (path, file_type.is_dir(), file_type.is_file())
        };

        if is_file {
            if name_str.ends_with(".d.ts")
                || name_str.ends_with(".d.mts")
                || name_str.ends_with(".d.cts")
            {
                return true;
            }
        } else if is_dir && has_declaration_files_inner(&effective_path, approved_roots, visited) {
            return true;
        }
    }
    false
}

struct TargetLock {
    #[cfg(unix)]
    _file: std::fs::File,
}

impl TargetLock {
    fn acquire(lock_path: &Path) -> io::Result<Self> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if ret != 0 {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(TargetLock {
            #[cfg(unix)]
            _file: file,
        })
    }
}

#[cfg(unix)]
fn sync_mtime_from_meta(meta: &fs::Metadata, dst: &Path) {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let times = [
        libc::timespec {
            tv_sec: meta.atime() as libc::time_t,
            tv_nsec: meta.atime_nsec() as libc::c_long,
        },
        libc::timespec {
            tv_sec: meta.mtime() as libc::time_t,
            tv_nsec: meta.mtime_nsec() as libc::c_long,
        },
    ];
    if let Ok(c_path) = std::ffi::CString::new(dst.as_os_str().as_bytes()) {
        unsafe {
            libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0);
        }
    }
}

#[cfg(not(unix))]
fn sync_mtime_from_meta(_meta: &fs::Metadata, _dst: &Path) {}

static TYPE_FILE_NONCE: AtomicU64 = AtomicU64::new(1);

/// Copies a type declaration file to `dst` atomically under a per-target lock,
/// ensuring concurrent seeders do not race and that older files never overwrite newer files.
/// Operates on the open file handle directly to prevent symlink TOCTOU races (#836).
fn copy_and_publish_type_file(src: &Path, dst: &Path) -> io::Result<u64> {
    let parent = dst.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination file has no parent directory")
    })?;
    ensure_cache_dir(parent)?;

    let file_name = dst.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination file has no name")
    })?;
    let lock_path = parent.join(format!(".lock-{}", file_name.to_string_lossy()));
    let _lock = TargetLock::acquire(&lock_path)?;

    // Open source file first; all subsequent metadata and copy operations use the open file handle (#836)
    let mut src_file = fs::File::open(src)?;
    let src_meta = src_file.metadata()?;
    let src_mod = src_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

    if let Ok(dst_meta) = fs::metadata(dst) {
        let dst_mod = dst_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if dst_mod > src_mod || (dst_mod == src_mod && dst_meta.len() == src_meta.len()) {
            return Ok(0);
        }
    }

    let nonce = TYPE_FILE_NONCE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_name = format!(".tmp-ts-{pid}-{nonce}-{ts:x}");
    let tmp_path = parent.join(tmp_name);

    let mut tmp_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&tmp_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let ret = unsafe { libc::flock(tmp_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            let _ = fs::remove_file(&tmp_path);
            return Err(io::Error::last_os_error());
        }
    }

    let bytes = match io::copy(&mut src_file, &mut tmp_file) {
        Ok(b) => b,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }
    };

    if let Err(e) = tmp_file.sync_all() {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    if let Err(e) = fs::rename(&tmp_path, dst) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    drop(tmp_file);

    sync_mtime_from_meta(&src_meta, dst);

    Ok(bytes)
}

#[derive(Default)]
struct VisitedDirs {
    #[cfg(unix)]
    dev_ino: std::collections::HashSet<(u64, u64)>,
    #[cfg(not(unix))]
    canonical: std::collections::HashSet<PathBuf>,
}

impl VisitedDirs {
    fn insert(&mut self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(meta) = fs::metadata(path) {
                if !meta.is_dir() {
                    return false;
                }
                return self.dev_ino.insert((meta.dev(), meta.ino()));
            }
            false
        }
        #[cfg(not(unix))]
        {
            if let Ok(canon) = path.canonicalize() {
                if canon.is_dir() {
                    return self.canonical.insert(canon);
                }
            }
            false
        }
    }
}

/// Builds the set of canonical approved roots from which symlinks may be safely dereferenced.
/// Includes the project root(s) and any validated pnpm virtual/global stores.
pub fn build_approved_roots(roots: &[&Path]) -> Vec<PathBuf> {
    let mut approved = Vec::new();
    for r in roots {
        if let Ok(c) = r.canonicalize() {
            approved.push(c);
        } else {
            approved.push(r.to_path_buf());
        }
    }

    // Include PNPM_HOME / global pnpm store if valid
    if let Some(pnpm_home) = std::env::var_os("PNPM_HOME") {
        let p = PathBuf::from(pnpm_home);
        if let Ok(c) = p.canonicalize() {
            approved.push(c);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home_path = PathBuf::from(home);
        for pnpm_sub in &[".local/share/pnpm", ".pnpm-store", "Library/pnpm"] {
            let candidate = home_path.join(pnpm_sub);
            if candidate.is_dir() {
                if let Ok(c) = candidate.canonicalize() {
                    approved.push(c);
                }
            }
        }
    }

    approved
}

/// Finds the enclosing project root by searching parent directories for standard manifests.
pub fn find_enclosing_project_root(path: &Path) -> PathBuf {
    let mut current = if path.is_file() {
        path.parent()
    } else {
        Some(path)
    };
    let mut candidate = None;
    while let Some(dir) = current {
        if dir.join("package.json").is_file()
            || dir.join("tsconfig.json").is_file()
            || dir.join("pnpm-workspace.yaml").is_file()
            || dir.join(".git").exists()
        {
            candidate = Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    candidate.unwrap_or_else(|| {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.to_path_buf())
    })
}

/// Resolves a path to its canonical target and returns it if it resides within an approved root (#836).
pub fn approved_target(target: &Path, approved_roots: &[PathBuf]) -> Option<PathBuf> {
    let canon = target.canonicalize().ok()?;
    if approved_roots.iter().any(|root| canon.starts_with(root)) {
        Some(canon)
    } else {
        None
    }
}

/// Checks whether a symlink's target canonical path is inside an approved project or package root.
pub fn is_target_approved(target: &Path, approved_roots: &[PathBuf]) -> bool {
    approved_target(target, approved_roots).is_some()
}

/// Recursively copies and merges type declarations from `src_dir` into `dst_dir`.
///
/// Only declaration files (`.d.ts`, `.d.mts`, `.d.cts`, `.d.ts.map`, `.json`, etc.)
/// and subdirectories containing them are indexed.
/// Safely dereferences valid package symlinks (such as pnpm package symlinks into virtual stores)
/// while strictly rejecting out-of-root symlinks and tracking visited directory inodes to prevent cycles (#835, #836).
/// Returns total bytes written or updated.
pub fn merge_types(src_dir: &Path, dst_dir: &Path) -> io::Result<u64> {
    let project_root = find_enclosing_project_root(src_dir);
    merge_types_within(src_dir, dst_dir, &[&project_root])
}

/// Recursively copies and merges type declarations from `src_dir` into `dst_dir` constraining
/// symlinks to explicit approved roots (such as `from` and `to` project worktrees).
pub fn merge_types_within(
    src_dir: &Path,
    dst_dir: &Path,
    approved_roots: &[&Path],
) -> io::Result<u64> {
    let approved = build_approved_roots(approved_roots);
    let mut visited = VisitedDirs::default();
    merge_types_inner(src_dir, dst_dir, &approved, &mut visited)
}

fn is_type_declaration_file(name: &str) -> bool {
    name.ends_with(".d.ts")
        || name.ends_with(".d.mts")
        || name.ends_with(".d.cts")
        || name.ends_with(".d.ts.map")
        || name.ends_with(".d.mts.map")
        || name.ends_with(".d.cts.map")
        || name == "package.json"
        || name == "tsconfig.json"
        || name.ends_with(".json")
}

fn merge_types_inner(
    src_dir: &Path,
    dst_dir: &Path,
    approved_roots: &[PathBuf],
    visited: &mut VisitedDirs,
) -> io::Result<u64> {
    let Some(canonical_src) = approved_target(src_dir, approved_roots) else {
        tracing::debug!(src_dir = %src_dir.display(), "skipping traversal root outside approved roots");
        return Ok(0);
    };
    if !canonical_src.is_dir() || !visited.insert(&canonical_src) {
        return Ok(0);
    }
    ensure_cache_dir(dst_dir)?;

    let mut bytes_written = 0u64;
    let Ok(entries) = fs::read_dir(&canonical_src) else {
        return Ok(0);
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        if name_str.starts_with('.') {
            continue;
        }

        // Safely dereference symlinks (e.g. pnpm package symlinks into virtual stores).
        // Resolves the canonical target once, verifies it is inside approved roots, and uses
        // that validated canonical target for all subsequent metadata, traversal, and copy operations
        // to prevent TOCTOU symlink swaps (#836).
        let (effective_path, is_dir, is_file) = if file_type.is_symlink() {
            let Some(canon) = approved_target(&path, approved_roots) else {
                tracing::debug!(path = %path.display(), "skipping symlink pointing outside approved roots");
                continue;
            };
            match fs::metadata(&canon) {
                Ok(meta) => (canon, meta.is_dir(), meta.is_file()),
                Err(_) => continue, // dangling symlink, skip safely
            }
        } else {
            (path, file_type.is_dir(), file_type.is_file())
        };

        if is_dir {
            let sub_dst = dst_dir.join(&file_name);
            let sub_bytes = merge_types_inner(&effective_path, &sub_dst, approved_roots, visited)?;
            bytes_written += sub_bytes;
        } else if is_file && is_type_declaration_file(&name_str) {
            let target_file = dst_dir.join(&file_name);
            let written = copy_and_publish_type_file(&effective_path, &target_file)?;
            bytes_written += written;
        }
    }

    Ok(bytes_written)
}

/// Recursively computes total size of all regular declaration files in a directory,
/// safely dereferencing valid symlinks within approved roots and tracking visited inodes to prevent cycles.
pub fn tree_size(dir: &Path) -> u64 {
    let project_root = find_enclosing_project_root(dir);
    tree_size_within(dir, &[&project_root])
}

/// Computes total size of declaration files in a directory, constraining symlinks to explicit approved roots.
pub fn tree_size_within(dir: &Path, approved_roots: &[&Path]) -> u64 {
    let approved = build_approved_roots(approved_roots);
    let mut visited = VisitedDirs::default();
    tree_size_inner(dir, &approved, &mut visited)
}

fn tree_size_inner(
    dir: &Path,
    approved_roots: &[PathBuf],
    visited: &mut VisitedDirs,
) -> u64 {
    let Some(canonical_dir) = approved_target(dir, approved_roots) else {
        return 0;
    };
    if !canonical_dir.is_dir() || !visited.insert(&canonical_dir) {
        return 0;
    }
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(&canonical_dir) {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };

            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            if name_str.starts_with('.') {
                continue;
            }

            // Safely dereference symlinks for sizing using validated canonical target (#836).
            let (effective_path, is_dir, is_file, file_len) = if file_type.is_symlink() {
                let Some(canon) = approved_target(&path, approved_roots) else {
                    continue;
                };
                match fs::metadata(&canon) {
                    Ok(meta) => {
                        let len = if meta.is_file() { meta.len() } else { 0 };
                        (canon, meta.is_dir(), meta.is_file(), len)
                    }
                    Err(_) => continue,
                }
            } else {
                let len = if file_type.is_file() {
                    entry.metadata().map(|m| m.len()).unwrap_or(0)
                } else {
                    0
                };
                (path, file_type.is_dir(), file_type.is_file(), len)
            };

            if is_dir {
                total += tree_size_inner(&effective_path, approved_roots, visited);
            } else if is_file && is_type_declaration_file(&name_str) {
                total += file_len;
            }
        }
    }
    total
}

/// Seeds TypeScript type declarations and `@types` cache across worktrees.
pub fn seed_typescript_worktree(from: &Path, to: &Path) -> io::Result<Option<u64>> {
    seed_typescript_worktree_within(from, to, disk_space(to))
}

/// Seeds TypeScript type declarations respecting a provided disk space budget.
pub fn seed_typescript_worktree_within(
    from: &Path,
    to: &Path,
    space: Option<DiskSpace>,
) -> io::Result<Option<u64>> {
    if !is_typescript_project(from) {
        return Ok(None);
    }

    let cache_dir = ts_types_cache_dir();
    let mut total_bytes = 0u64;

    // 1. Gather all candidate type declaration directories
    let discovered_types = find_project_types_within(from, &[from, to]);
    let aggregate_size: u64 = discovered_types
        .iter()
        .map(|d| tree_size_within(d, &[from, to]))
        .sum();

    // 2. Enforce disk budget before merging into shared cache
    if aggregate_size > 0 && seed_fits("typescript types cache", aggregate_size, space) {
        for type_dir in &discovered_types {
            let dir_name = type_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let dst_target = if dir_name == "@types" {
                cache_dir.clone()
            } else {
                cache_dir.join(dir_name)
            };
            let merged = merge_types_within(type_dir, &dst_target, &[from, to])?;
            total_bytes += merged;
        }
    }

    // 3. Establish `to/node_modules/@types` symlink pointing to the shared types cache
    let to_node_modules = to.join("node_modules");
    if let Err(e) = fs::create_dir_all(&to_node_modules) {
        tracing::debug!(error = %e, "creating to/node_modules failed");
    }

    let to_at_types = to_node_modules.join("@types");
    let symlink_meta = fs::symlink_metadata(&to_at_types);
    match symlink_meta {
        Err(_) => {
            #[cfg(unix)]
            {
                if std::os::unix::fs::symlink(&cache_dir, &to_at_types).is_ok() {
                    total_bytes += 1;
                }
            }
            #[cfg(not(unix))]
            {
                let _ = merge_types(&cache_dir, &to_at_types);
            }
        }
        Ok(m) if m.file_type().is_symlink() => {
            // Already a symlink. If dangling, replace with shared cache symlink.
            if !to_at_types.exists() {
                let _ = fs::remove_file(&to_at_types);
                #[cfg(unix)]
                {
                    if std::os::unix::fs::symlink(&cache_dir, &to_at_types).is_ok() {
                        total_bytes += 1;
                    }
                }
            }
        }
        Ok(m) if m.is_dir() => {
            // If it's a real directory (e.g. copied by dependency trees or created by pnpm),
            // safely dereference valid package links and merge into shared cache before deduplicating.
            let to_size = tree_size_within(&to_at_types, &[from, to]);
            if to_size == 0 || seed_fits("typescript types cache", to_size, space) {
                let merged = merge_types_within(&to_at_types, &cache_dir, &[from, to])?;
                total_bytes += merged;

                // Deduplicate: replace real directory with symlink to shared cache to eliminate duplicate gigabytes
                #[cfg(unix)]
                {
                    let backup = to_node_modules.join(".old-at-types");
                    if fs::rename(&to_at_types, &backup).is_ok() {
                        let to_had_types = tree_size_within(&backup, &[from, to]) > 0;
                        let cache_has_types = tree_size(&cache_dir) > 0;
                        // If backup had valid types, ensure cache has types before committing to symlink
                        if (!to_had_types || cache_has_types)
                            && std::os::unix::fs::symlink(&cache_dir, &to_at_types).is_ok()
                        {
                            let _ = fs::remove_dir_all(&backup);
                        } else {
                            let _ = fs::remove_file(&to_at_types);
                            let _ = fs::rename(&backup, &to_at_types);
                        }
                    }
                }
            }
        }
        _ => {}
    }

    // 4. Coordinate tsconfig.json / jsconfig.json in `to`
    coordinate_tsconfig(&to.join("tsconfig.json"));
    coordinate_tsconfig(&to.join("jsconfig.json"));

    Ok(Some(total_bytes))
}

/// Ensures `typeRoots` in `tsconfig.json` or `jsconfig.json` includes `"node_modules/@types"`
/// if custom `typeRoots` are configured, preventing custom typeRoots from hiding shared types.
fn coordinate_tsconfig(config_path: &Path) {
    if !config_path.is_file() {
        return;
    }
    let Ok(content) = fs::read_to_string(config_path) else {
        return;
    };
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };

    let Some(obj) = val.as_object_mut() else {
        return;
    };

    let compiler_options = obj
        .entry("compilerOptions")
        .or_insert_with(|| serde_json::json!({}));

    if let Some(opts) = compiler_options.as_object_mut() {
        if let Some(type_roots) = opts.get_mut("typeRoots").and_then(|tr| tr.as_array_mut()) {
            let has_node_modules_types = type_roots.iter().any(|v| {
                v.as_str().map_or(false, |s| {
                    s == "node_modules/@types"
                        || s == "./node_modules/@types"
                        || s.ends_with("/node_modules/@types")
                })
            });
            if !has_node_modules_types {
                type_roots.push(serde_json::Value::String("node_modules/@types".to_string()));
                if let Ok(updated) = serde_json::to_string_pretty(&val) {
                    let _ = fs::write(config_path, updated);
                }
            }
        }
    }
}

/// Parses the timestamp from a `.tmp-ts-{pid}-{nonce}-{ts:x}` filename.
/// Returns `None` if the name does not match the expected format or if the timestamp
/// is prior to the year 2020.
pub fn parse_tmp_ts_timestamp(name: &str) -> Option<SystemTime> {
    let parts: Vec<&str> = name.split('-').collect();
    if parts.len() == 5 && parts[0] == ".tmp" && parts[1] == "ts" {
        let _pid: u32 = parts[2].parse().ok()?;
        let _nonce: u64 = parts[3].parse().ok()?;
        let ts_str = parts[4];
        if let Ok(nanos) = u128::from_str_radix(ts_str, 16) {
            // Must be a reasonable epoch timestamp (after year 2020: ~1.57e18 nanos)
            const MIN_VALID_NANOS: u128 = 1_500_000_000_000_000_000;
            if nanos >= MIN_VALID_NANOS {
                let secs = (nanos / 1_000_000_000) as u64;
                let subsec = (nanos % 1_000_000_000) as u32;
                return Some(SystemTime::UNIX_EPOCH + Duration::new(secs, subsec));
            }
        }
    }
    None
}

/// Grace period for temporary files created during atomic copy (1 hour).
pub const TMP_TS_GRACE_PERIOD: Duration = Duration::from_secs(3600);

/// Evicts stale type files from the shared TypeScript types cache based on age and max capacity.
/// Uses secure directory-handle-relative traversal and O_NOFOLLOW to eliminate symlink TOCTOU.
pub fn prune_stale_types_cache(max_age: Duration, max_size_bytes: u64) -> io::Result<usize> {
    let cache_dir = ts_types_cache_dir();
    prune_stale_types_cache_in(&cache_dir, max_age, max_size_bytes)
}

/// Prunes stale type cache files within the specified directory using the default 1-hour grace period for temporary files.
pub fn prune_stale_types_cache_in(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
) -> io::Result<usize> {
    prune_stale_types_cache_with_grace(cache_dir, max_age, max_size_bytes, TMP_TS_GRACE_PERIOD)
}

/// Prunes stale type cache files with a configurable grace period for abandoned temporary files.
pub fn prune_stale_types_cache_with_grace(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
    tmp_grace_period: Duration,
) -> io::Result<usize> {
    #[cfg(unix)]
    {
        unix_pruner::prune_unix(cache_dir, max_age, max_size_bytes, tmp_grace_period)
    }

    #[cfg(not(unix))]
    {
        prune_fallback(cache_dir, max_age, max_size_bytes, tmp_grace_period)
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
        tmp_grace_period: Duration,
    ) -> io::Result<usize> {
        let canonical_root = match fs::canonicalize(cache_dir) {
            Ok(c) => c,
            Err(_) => return Ok(0),
        };
        let expected_meta = match fs::symlink_metadata(&canonical_root) {
            Ok(m) => m,
            Err(_) => return Ok(0),
        };
        if !expected_meta.file_type().is_dir() {
            return Ok(0);
        }

        let c_root = std::ffi::CString::new(canonical_root.as_os_str().as_bytes())
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
            unsafe { libc::close(root_fd); }
            return Ok(0);
        }

        use std::os::unix::fs::MetadataExt;
        if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR
            || (st.st_dev as u64) != expected_meta.dev()
            || (st.st_ino as u64) != expected_meta.ino()
        {
            unsafe { libc::close(root_fd); }
            return Ok(0);
        }

        let mut files = Vec::new();
        let mut total_size = 0u64;
        let mut rel_components = Vec::new();

        let now = SystemTime::now();
        let mut removed = 0;

        unsafe {
            collect_dir(
                root_fd,
                &mut rel_components,
                &mut files,
                &mut total_size,
                now,
                tmp_grace_period,
                &mut removed,
            );
        }

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
        now: SystemTime,
        tmp_grace_period: Duration,
        removed: &mut usize,
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
                if bytes.ends_with(b".lock") {
                    continue;
                }
                let is_tmp_ts = bytes.starts_with(b".tmp-ts-");
                if bytes.starts_with(b".") && !is_tmp_ts {
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

                if is_tmp_ts {
                    if mode == libc::S_IFREG {
                        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(st.st_mtime.max(0) as u64);
                        let file_size = st.st_size as u64;
                        let name_str = name.to_str().unwrap_or("");
                        let created_at = parse_tmp_ts_timestamp(name_str);
                        let age = match created_at {
                            Some(ts) => now.duration_since(ts).unwrap_or_else(|_| {
                                now.duration_since(modified).unwrap_or(Duration::ZERO)
                            }),
                            None => now.duration_since(modified).unwrap_or(Duration::ZERO),
                        };

                        // Check whether an active publisher holds an OS lock on this temporary file
                        let is_locked = is_temp_file_locked(current_fd, name.as_ptr());

                        if !is_locked && age > tmp_grace_period {
                            // Abandoned temporary file: unlink immediately
                            if libc::unlinkat(current_fd, name.as_ptr(), 0) == 0 {
                                *removed += 1;
                            }
                        } else {
                            // Active publisher holding lock or recent temporary file: account for disk space
                            *total_size += file_size;
                        }
                    }
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
                        collect_dir(
                            child_fd,
                            rel_components,
                            files,
                            total_size,
                            now,
                            tmp_grace_period,
                            removed,
                        );
                        rel_components.pop();
                        libc::close(child_fd);
                    }
                } else if mode == libc::S_IFREG {
                    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(st.st_mtime.max(0) as u64);
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
                    unsafe { libc::close(fd); }
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
            unsafe { libc::close(fd); }
        }

        removed
    }

    unsafe fn is_temp_file_locked(dir_fd: libc::c_int, name: *const libc::c_char) -> bool {
        unsafe {
            let fd = libc::openat(
                dir_fd,
                name,
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            );
            if fd < 0 {
                return true;
            }

            let ret = libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB);
            if ret == 0 {
                libc::flock(fd, libc::LOCK_UN);
                libc::close(fd);
                false
            } else {
                libc::close(fd);
                true
            }
        }
    }
}

#[cfg(not(unix))]
fn prune_fallback(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
    tmp_grace_period: Duration,
) -> io::Result<usize> {
    if !cache_dir.is_dir() {
        return Ok(0);
    }
    let now = SystemTime::now();
    let mut removed = 0;
    let mut files = Vec::new();
    let mut total_size = 0u64;

    fn walk(
        dir: &Path,
        files: &mut Vec<(PathBuf, u64, SystemTime)>,
        total_size: &mut u64,
        now: SystemTime,
        tmp_grace_period: Duration,
        removed: &mut usize,
    ) {
        let Ok(entries) = fs::read_dir(dir) else { return; };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else { continue; };
            if file_type.is_symlink() { continue; }
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".lock") { continue; }
            let is_tmp_ts = name_str.starts_with(".tmp-ts-");
            if name_str.starts_with('.') && !is_tmp_ts { continue; }

            if file_type.is_dir() {
                walk(&path, files, total_size, now, tmp_grace_period, removed);
            } else if file_type.is_file() {
                let Ok(meta) = fs::metadata(&path) else { continue; };
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let size = meta.len();
                let created_at = parse_tmp_ts_timestamp(&name_str);
                let age = match created_at {
                    Some(ts) => now.duration_since(ts).unwrap_or_else(|_| {
                        now.duration_since(modified).unwrap_or(Duration::ZERO)
                    }),
                    None => now.duration_since(modified).unwrap_or(Duration::ZERO),
                };

                if is_tmp_ts {
                    let is_locked = fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .is_err();

                    if !is_locked && age > tmp_grace_period {
                        if fs::remove_file(&path).is_ok() {
                            *removed += 1;
                        }
                    } else {
                        *total_size += size;
                    }
                } else {
                    *total_size += size;
                    files.push((path, size, modified));
                }
            }
        }
    }

    walk(cache_dir, &mut files, &mut total_size, now, tmp_grace_period, &mut removed);

    // Evict files older than max_age
    files.retain(|(path, size, modified)| {
        if let Ok(age) = now.duration_since(*modified) {
            if age > max_age {
                if fs::remove_file(path).is_ok() {
                    *total_size = total_size.saturating_sub(*size);
                    removed += 1;
                    return false;
                }
            }
        }
        true
    });

    // Evict oldest if still over budget
    if total_size > max_size_bytes {
        files.sort_by_key(|(_, _, modified)| *modified);
        for (path, size, _) in files {
            if total_size <= max_size_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total_size = total_size.saturating_sub(size);
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
    fn test_ts_types_cache_dir_and_env() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let custom = temp.path().join("custom-ts-types");

        unsafe {
            std::env::set_var(TS_TYPES_CACHE_ENV, &custom);
        }

        let dir = ts_types_cache_dir();
        assert_eq!(dir, custom);
        assert!(dir.is_dir());

        let envs = ts_types_cache_env();
        let custom_str = custom.to_str().unwrap();
        assert!(envs.iter().any(|(k, v)| k == TS_TYPES_CACHE_ENV && v == custom_str));
        assert!(envs.iter().any(|(k, v)| k == "TS_TYPES_CACHE" && v == custom_str));

        unsafe {
            std::env::remove_var(TS_TYPES_CACHE_ENV);
        }
    }

    #[test]
    fn test_is_typescript_project() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!is_typescript_project(temp.path()));

        fs::write(temp.path().join("package.json"), "{}").unwrap();
        assert!(is_typescript_project(temp.path()));

        let temp2 = tempfile::tempdir().unwrap();
        fs::write(temp2.path().join("tsconfig.json"), "{}").unwrap();
        assert!(is_typescript_project(temp2.path()));

        let temp3 = tempfile::tempdir().unwrap();
        fs::write(temp3.path().join("index.ts"), "export const x = 1;").unwrap();
        assert!(is_typescript_project(temp3.path()));
    }

    #[test]
    fn test_parse_tmp_ts_timestamp() {
        let now_nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!(".tmp-ts-12345-1-{now_nanos:x}");
        let parsed = parse_tmp_ts_timestamp(&name);
        assert!(parsed.is_some());

        // Invalid timestamp: prior to 2020
        let old_name = ".tmp-ts-12345-1-100";
        assert!(parse_tmp_ts_timestamp(old_name).is_none());

        // Malformed name
        assert!(parse_tmp_ts_timestamp("other.tmp").is_none());
        assert!(parse_tmp_ts_timestamp(".tmp-ts-notapid-1-123456").is_none());
    }

    #[test]
    fn test_coordinate_tsconfig() {
        let temp = tempfile::tempdir().unwrap();
        let tsconfig = temp.path().join("tsconfig.json");

        // Custom typeRoots without node_modules/@types
        let content = serde_json::json!({
            "compilerOptions": {
                "typeRoots": ["custom_types"]
            }
        });
        fs::write(&tsconfig, serde_json::to_string(&content).unwrap()).unwrap();

        coordinate_tsconfig(&tsconfig);

        let updated: serde_json::Value = serde_json::from_str(&fs::read_to_string(&tsconfig).unwrap()).unwrap();
        let roots = updated["compilerOptions"]["typeRoots"].as_array().unwrap();
        assert!(roots.iter().any(|r| r.as_str() == Some("node_modules/@types")));
    }
}
