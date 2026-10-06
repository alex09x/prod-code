/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::io;
use std::path::{Path, PathBuf};

use super::relocate::{is_under_root, relocate_compile_commands_content};
use super::shard::{
    clangd_path_digest, parse_shard_filename, relocate_shard, shard_filename_for_path,
};

/// Identifies the primary translation unit path for a shard.
///
/// In clangd, background index shards contain strings for the primary translation unit,
/// but also for all directly and transitively included headers. Because the string table
/// is sorted alphabetically, the first source/header path is often an included header
/// (e.g. `include/a.h`) rather than the primary file (e.g. `src/z_main.cpp`).
///
/// To prevent writing the relocated shard under the wrong filename:
/// 1. If `orig_shard_name` is provided, we parse its `<filename>.<16-HEX-DIGEST>.idx` identity.
///    We compute `clangd_path_digest(orig_path)` for all workspace paths in the string table
///    and match the exact 16-hex digest.
/// 2. If no exact digest matched (e.g. symlinks/canonicalization), we match candidates by `file_name`.
///    If exactly one matches, we use it; if ambiguous or 0, we reject the shard.
/// 3. If no `orig_shard_name` is provided, we look for unique source implementation files (`.cpp`, `.cc`, etc.),
///    then unique header files, rejecting ambiguous candidates.
pub(crate) fn identify_primary_source(
    orig_strings: &[String],
    relocated_strings: &[String],
    from_str: &str,
    orig_shard_name: Option<&str>,
) -> Option<PathBuf> {
    let shard_id = orig_shard_name.and_then(parse_shard_filename);

    if let Some(id) = shard_id {
        // Priority 1: Exact digest match on an origin path.
        for (orig, relocated) in orig_strings.iter().zip(relocated_strings.iter()) {
            let orig_path = orig.strip_prefix("file://").unwrap_or(orig);
            if is_under_root(orig_path, from_str) {
                let digest = clangd_path_digest(orig_path);
                if digest == id.digest_hex {
                    let p = Path::new(orig_path);
                    if p.file_name().and_then(|f| f.to_str()) == Some(id.file_name.as_str()) {
                        let rel_path = relocated.strip_prefix("file://").unwrap_or(relocated);
                        return Some(PathBuf::from(rel_path));
                    }
                }
            }
        }

        // Priority 2: Candidates matching id.file_name exactly.
        let mut candidates = Vec::new();
        for (orig, relocated) in orig_strings.iter().zip(relocated_strings.iter()) {
            let orig_path = orig.strip_prefix("file://").unwrap_or(orig);
            if is_under_root(orig_path, from_str) {
                let p = Path::new(orig_path);
                if p.file_name().and_then(|f| f.to_str()) == Some(id.file_name.as_str()) {
                    let rel_path = relocated.strip_prefix("file://").unwrap_or(relocated);
                    candidates.push(PathBuf::from(rel_path));
                }
            }
        }

        candidates.sort();
        candidates.dedup();
        if candidates.len() == 1 {
            return Some(candidates.remove(0));
        }

        // Multiple different paths match id.file_name or 0 match: reject ambiguous shard.
        return None;
    }

    // Fallback when no shard name was supplied (e.g. synthetic test calls without filename):
    let mut source_candidates = Vec::new();
    let mut header_candidates = Vec::new();

    for (orig, relocated) in orig_strings.iter().zip(relocated_strings.iter()) {
        let orig_path = orig.strip_prefix("file://").unwrap_or(orig);
        if is_under_root(orig_path, from_str) {
            let p = Path::new(orig_path);
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                let rel_path =
                    PathBuf::from(relocated.strip_prefix("file://").unwrap_or(relocated));
                match ext {
                    "c" | "cc" | "cpp" | "cxx" => source_candidates.push(rel_path),
                    "h" | "hh" | "hpp" | "hxx" => header_candidates.push(rel_path),
                    _ => {}
                }
            }
        }
    }

    source_candidates.sort();
    source_candidates.dedup();
    header_candidates.sort();
    header_candidates.dedup();

    if source_candidates.len() == 1 {
        Some(source_candidates.remove(0))
    } else if source_candidates.is_empty() && header_candidates.len() == 1 {
        Some(header_candidates.remove(0))
    } else {
        None
    }
}

/// Seeds and relocates clangd background index shards from `from` to `to`.
///
/// Searches the index directory next to `compile_commands.json` first, then the conventional
/// `<from>/.cache/clangd/index/` and `<from>/.clangd/index/` locations.
/// For each shard:
/// 1. Rewrites string table paths from `from` to `to`.
/// 2. Derives the target source file path and computes its new LLVM xxh3 digest.
/// 3. Emits each shard under the matching target index directory.
/// 4. Generates an index `.gitignore` with `*\n`.
///
/// Returns total bytes written, or `None` if no shards were present.
pub fn seed_clangd_index(from: &Path, to: &Path) -> io::Result<Option<u64>> {
    let candidate_dirs = [
        (
            from.join("build")
                .join(".cache")
                .join("clangd")
                .join("index"),
            to.join("build").join(".cache").join("clangd").join("index"),
        ),
        (
            from.join(".cache").join("clangd").join("index"),
            to.join(".cache").join("clangd").join("index"),
        ),
        (
            from.join(".clangd").join("index"),
            to.join(".clangd").join("index"),
        ),
    ];

    let Some((index_dir, target_index_dir)) = candidate_dirs
        .iter()
        .find(|(candidate, _)| candidate.is_dir())
    else {
        return Ok(None);
    };
    std::fs::create_dir_all(target_index_dir)?;

    let mut total_bytes = 0u64;
    let mut seeded_shards = 0usize;

    for entry in std::fs::read_dir(index_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if parse_shard_filename(name).is_none() {
            continue;
        }

        let shard_bytes = std::fs::read(&path)?;
        let (relocated, primary_source) = match relocate_shard(&shard_bytes, from, to, Some(name)) {
            Ok(res) => res,
            Err(e) => {
                tracing::debug!(error = %e, shard = %name, "skipping non-RIFF or invalid clangd shard");
                continue;
            }
        };

        // Determine target shard filename from the resolved primary TU.
        // Reject ambiguous shards that cannot be mapped to a known source file.
        let Some(source_path) = primary_source else {
            tracing::warn!(shard = %name, "skipping ambiguous clangd shard whose primary TU cannot be determined");
            continue;
        };

        let new_shard_name = match shard_filename_for_path(&source_path) {
            Some(n) => n,
            None => {
                tracing::warn!(shard = %name, source = ?source_path, "failed to derive target shard filename");
                continue;
            }
        };

        let target_shard_path = target_index_dir.join(new_shard_name);
        std::fs::write(&target_shard_path, &relocated)?;
        total_bytes += relocated.len() as u64;
        seeded_shards += 1;
    }

    if seeded_shards > 0 {
        let gitignore = target_index_dir.join(".gitignore");
        if !gitignore.exists() {
            let _ = std::fs::write(gitignore, "# Autogenerated by prod-code clangd seed\n*\n");
        }
        Ok(Some(total_bytes))
    } else {
        Ok(None)
    }
}

/// Seeds `compile_commands.json` from `from` into `to/build/compile_commands.json`
/// with paths relocated.
pub fn seed_compile_commands(from: &Path, to: &Path) -> io::Result<Option<u64>> {
    let candidate_files = [
        from.join("build").join("compile_commands.json"),
        from.join("compile_commands.json"),
    ];

    let mut found_file = None;
    for cand in &candidate_files {
        if cand.is_file() {
            found_file = Some(cand);
            break;
        }
    }

    let Some(source_cdb) = found_file else {
        return Ok(None);
    };

    let from_str = from
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "from is not valid UTF-8"))?;
    let to_str = to
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "to is not valid UTF-8"))?;

    let content = std::fs::read_to_string(source_cdb)?;
    let relocated = relocate_compile_commands_content(&content, from_str, to_str)?;

    let target_dir = to.join("build");
    std::fs::create_dir_all(&target_dir)?;
    let target_file = target_dir.join("compile_commands.json");
    std::fs::write(&target_file, relocated.as_bytes())?;

    Ok(Some(relocated.len() as u64))
}

/// Combined seeding for C/C++ worktrees: relocates both `compile_commands.json` and
/// the clangd background index shards.
pub fn seed_cpp_worktree(from: &Path, to: &Path) -> io::Result<Option<u64>> {
    let mut total = 0u64;
    let mut any = false;

    if let Some(cdb_bytes) = seed_compile_commands(from, to)? {
        total += cdb_bytes;
        any = true;
    }

    if let Some(idx_bytes) = seed_clangd_index(from, to)? {
        total += idx_bytes;
        any = true;
    }

    if any { Ok(Some(total)) } else { Ok(None) }
}
