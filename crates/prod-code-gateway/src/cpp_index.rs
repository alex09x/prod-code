//! Cross-worktree C/C++ engine support: clangd background index seeding, compilation
//! database relocation, and shared precompiled header (PCH) compiler caching (Roadmap 3.4).

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Computes the 16-character uppercase hexadecimal digest of a file path as expected
/// by clangd's `BackgroundIndexStorage` (`llvm::toHex(digest(FilePath))`).
///
/// In LLVM clangd, `digest` calculates `xxh3_64bits` and copies the 64-bit integer
/// into a little-endian 8-byte array (`FileDigest`), which `llvm::toHex` then formats
/// as two uppercase hex characters per byte.
pub fn clangd_path_digest(path: &str) -> String {
    let hash = xxhash_rust::xxh3::xxh3_64(path.as_bytes());
    let le = hash.to_le_bytes();
    let mut hex = String::with_capacity(16);
    for b in le {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{:02X}", b);
    }
    hex
}

/// Parsed identity of a clangd index shard filename `<filename>.<16-HEX-DIGEST>.idx`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardIdentity {
    pub file_name: String,
    pub digest_hex: String,
}

/// Parses `<filename>.<16-HEX-DIGEST>.idx` into its file basename and uppercase hex digest.
pub fn parse_shard_filename(shard_filename: &str) -> Option<ShardIdentity> {
    let name = shard_filename.strip_suffix(".idx")?;
    let (base, digest) = name.rsplit_once('.')?;
    if digest.len() == 16 && digest.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(ShardIdentity {
            file_name: base.to_string(),
            digest_hex: digest.to_ascii_uppercase(),
        })
    } else {
        None
    }
}

/// Computes the clangd shard filename for a source or header file on disk.
///
/// Format: `<filename>.<16-HEX-DIGEST>.idx`
pub fn shard_filename_for_path(path: &Path) -> Option<String> {
    let filename = path.file_name()?.to_str()?;
    let path_str = path.to_str()?;
    let digest = clangd_path_digest(path_str);
    Some(format!("{filename}.{digest}.idx"))
}

/// A parsed RIFF chunk within a clangd index file.
#[derive(Debug, Clone)]
struct RiffChunk {
    tag: [u8; 4],
    data: Vec<u8>,
}

/// Relocates a RIFF `CdIx` clangd index shard from `from_workspace` to `to_workspace`.
///
/// This parses the RIFF container, locates the string table (`stri`), decompresses it
/// via zlib, substitutes all occurrences of `from_workspace` with `to_workspace` (both
/// filesystem paths and `file://` URIs), recompresses the string table, and rebuilds
/// the RIFF container with updated chunk and header lengths.
///
/// Because all symbols, references, relations, and include-graph nodes address strings
/// by ordinal index in the string table, preserving the number and order of null-delimited
/// strings — including the index 0 empty-string sentinel — guarantees that all index references
/// remain valid and unbroken.
///
/// When `orig_shard_name` is provided, the primary translation unit is derived from the shard's
/// original basename and LLVM path digest, ensuring shards containing included headers
/// are never written under the wrong filename. Ambiguous shards are rejected.
pub fn relocate_shard(
    shard_data: &[u8],
    from_workspace: &Path,
    to_workspace: &Path,
    orig_shard_name: Option<&str>,
) -> io::Result<(Vec<u8>, Option<PathBuf>)> {
    if shard_data.len() < 12 || &shard_data[0..4] != b"RIFF" || &shard_data[8..12] != b"CdIx" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a valid clangd RIFF CdIx index file",
        ));
    }

    let from_str = from_workspace.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "from_workspace is not valid UTF-8")
    })?;
    let to_str = to_workspace.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "to_workspace is not valid UTF-8")
    })?;

    let mut chunks = Vec::new();
    let mut offset = 12;
    let mut orig_strings_table = Vec::new();
    let mut relocated_strings_table = Vec::new();

    while offset + 8 <= shard_data.len() {
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&shard_data[offset..offset + 4]);
        let length = u32::from_le_bytes(shard_data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        offset += 8;

        if offset + length > shard_data.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated RIFF chunk",
            ));
        }

        let chunk_bytes = &shard_data[offset..offset + length];
        offset += length;
        if length % 2 != 0 && offset < shard_data.len() {
            offset += 1; // 1-byte padding for odd chunk lengths
        }

        if &tag == b"stri" {
            let (relocated_chunk, orig_strings, relocated_strings) = relocate_string_table(
                chunk_bytes,
                from_str,
                to_str,
            )?;
            orig_strings_table = orig_strings;
            relocated_strings_table = relocated_strings;
            chunks.push(RiffChunk {
                tag,
                data: relocated_chunk,
            });
        } else {
            chunks.push(RiffChunk {
                tag,
                data: chunk_bytes.to_vec(),
            });
        }
    }

    let primary_source_path = identify_primary_source(
        &orig_strings_table,
        &relocated_strings_table,
        from_str,
        orig_shard_name,
    );

    // Rebuild RIFF container
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    // Placeholder for total length
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(b"CdIx");

    for chunk in &chunks {
        out.extend_from_slice(&chunk.tag);
        let len = chunk.data.len() as u32;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&chunk.data);
        if len % 2 != 0 {
            out.push(0); // Pad to 2-byte boundary
        }
    }

    let total_len = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&total_len.to_le_bytes());

    Ok((out, primary_source_path))
}

/// Relocates paths in the string table (`stri`) chunk.
///
/// Clangd string table serialization begins with an empty string (`""`) at index 0 (sentinel)
/// and emits strings terminated by null bytes (`\0`). Other RIFF chunks address strings by
/// ordinal index. We MUST preserve every single slot in exact ordinal position, including index 0.
fn relocate_string_table(
    data: &[u8],
    from_str: &str,
    to_str: &str,
) -> io::Result<(Vec<u8>, Vec<String>, Vec<String>)> {
    if data.len() < 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "stri chunk too small",
        ));
    }

    let uncompressed_size = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let raw_payload = &data[4..];

    let decompressed = if uncompressed_size == 0 {
        raw_payload.to_vec()
    } else {
        let mut decoder = flate2::read::ZlibDecoder::new(raw_payload);
        let mut buf = Vec::with_capacity(uncompressed_size);
        decoder.read_to_end(&mut buf)?;
        buf
    };

    // The string table contains null-terminated strings: s0 \0 s1 \0 s2 \0 ...
    // If the buffer ends with a trailing \0, strip it before splitting so we don't
    // produce an extra empty element beyond the last terminated string.
    let bytes = decompressed.strip_suffix(&[0]).unwrap_or(&decompressed);
    let slices: Vec<&[u8]> = if bytes.is_empty() && decompressed.is_empty() {
        Vec::new()
    } else {
        bytes.split(|&b| b == 0).collect()
    };

    let mut orig_strings = Vec::with_capacity(slices.len());
    let mut relocated_strings = Vec::with_capacity(slices.len());

    for slice in slices {
        let orig = String::from_utf8_lossy(slice).into_owned();
        let replaced = orig.replace(from_str, to_str);
        orig_strings.push(orig);
        relocated_strings.push(replaced);
    }

    // Join back into null-terminated string table
    let mut new_uncompressed = Vec::new();
    for s in &relocated_strings {
        new_uncompressed.extend_from_slice(s.as_bytes());
        new_uncompressed.push(0);
    }

    let mut out = Vec::new();
    if uncompressed_size == 0 {
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&new_uncompressed);
    } else {
        let new_uncomp_size = new_uncompressed.len() as u32;
        out.extend_from_slice(&new_uncomp_size.to_le_bytes());
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::<u8>::new(), flate2::Compression::default());
        encoder.write_all(&new_uncompressed)?;
        let compressed = encoder.finish()?;
        out.extend_from_slice(&compressed);
    }

    Ok((out, orig_strings, relocated_strings))
}

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
fn identify_primary_source(
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
            if orig_path.starts_with(from_str) {
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
            if orig_path.starts_with(from_str) {
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
        if orig_path.starts_with(from_str) {
            let p = Path::new(orig_path);
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                let rel_path = PathBuf::from(relocated.strip_prefix("file://").unwrap_or(relocated));
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
/// Searches `<from>/.cache/clangd/index/` and `<from>/.clangd/index/` for `.idx` files.
/// For each shard:
/// 1. Rewrites string table paths from `from` to `to`.
/// 2. Derives the target source file path and computes its new LLVM xxh3 digest.
/// 3. Emits `<to>/.cache/clangd/index/<filename>.<new-digest>.idx`.
/// 4. Generates `<to>/.cache/clangd/index/.gitignore` with `*\n`.
///
/// Returns total bytes written, or `None` if no shards were present.
pub fn seed_clangd_index(from: &Path, to: &Path) -> io::Result<Option<u64>> {
    let candidate_dirs = [
        from.join(".cache").join("clangd").join("index"),
        from.join(".clangd").join("index"),
    ];

    let mut found_dir = None;
    for cand in &candidate_dirs {
        if cand.is_dir() {
            found_dir = Some(cand);
            break;
        }
    }

    let Some(index_dir) = found_dir else {
        return Ok(None);
    };

    let target_index_dir = to.join(".cache").join("clangd").join("index");
    std::fs::create_dir_all(&target_index_dir)?;

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

    let from_str = from.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "from is not valid UTF-8")
    })?;
    let to_str = to.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "to is not valid UTF-8")
    })?;

    let content = std::fs::read_to_string(source_cdb)?;
    let relocated = content.replace(from_str, to_str);

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

    if any {
        Ok(Some(total))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clangd_digest_format_and_properties() {
        let path = "/srv/workspaces/repo/src/main.cpp";
        let digest = clangd_path_digest(path);
        assert_eq!(digest.len(), 16);
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));

        let shard_name = shard_filename_for_path(Path::new(path)).unwrap();
        assert_eq!(shard_name, format!("main.cpp.{digest}.idx"));

        // Different paths yield different digests
        let path2 = "/srv/workspaces/repo--wt-1234/src/main.cpp";
        let digest2 = clangd_path_digest(path2);
        assert_ne!(digest, digest2);
    }

    #[test]
    fn test_riff_shard_relocation_and_rebuilding() {
        let from = Path::new("/srv/workspaces/repo");
        let to = Path::new("/srv/workspaces/repo--wt-1234");

        // Construct synthetic uncompressed string table
        let strings = vec![
            "".to_string(),
            "/srv/workspaces/repo".to_string(),
            "/srv/workspaces/repo/src/main.cpp".to_string(),
            "file:///srv/workspaces/repo/src/main.cpp".to_string(),
            "compute_magic".to_string(),
        ];
        let mut uncompressed = Vec::new();
        for s in &strings {
            uncompressed.extend_from_slice(s.as_bytes());
            uncompressed.push(0);
        }

        // Compress string table
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::<u8>::new(), flate2::Compression::default());
        encoder.write_all(&uncompressed).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut stri_data = Vec::new();
        stri_data.extend_from_slice(&(uncompressed.len() as u32).to_le_bytes());
        stri_data.extend_from_slice(&compressed);

        // Build RIFF container
        let mut riff = Vec::new();
        riff.extend_from_slice(b"RIFF");
        let total_len = 4 + 8 + 4 + (8 + stri_data.len()) + (8 + 4);
        riff.extend_from_slice(&(total_len as u32).to_le_bytes());
        riff.extend_from_slice(b"CdIx");

        // meta chunk (version 21)
        riff.extend_from_slice(b"meta");
        riff.extend_from_slice(&4u32.to_le_bytes());
        riff.extend_from_slice(&21u32.to_le_bytes());

        // stri chunk
        riff.extend_from_slice(b"stri");
        let stri_len = stri_data.len() as u32;
        riff.extend_from_slice(&stri_len.to_le_bytes());
        riff.extend_from_slice(&stri_data);
        if stri_len % 2 != 0 {
            riff.push(0);
        }

        // Relocate shard without explicit shard name
        let (relocated, source_path) = relocate_shard(&riff, from, to, None).unwrap();
        assert_eq!(
            source_path,
            Some(PathBuf::from("/srv/workspaces/repo--wt-1234/src/main.cpp"))
        );

        // Verify the relocated shard can be re-parsed and preserves all slots including index 0
        let mut decoder = flate2::read::ZlibDecoder::new(&relocated[36..]);
        let mut decomp = Vec::new();
        decoder.read_to_end(&mut decomp).unwrap();

        let bytes = decomp.strip_suffix(&[0]).unwrap_or(&decomp);
        let decomp_strings: Vec<String> = bytes
            .split(|&b| b == 0)
            .map(|s| String::from_utf8_lossy(s).to_string())
            .collect();

        assert_eq!(decomp_strings.len(), strings.len());
        assert_eq!(decomp_strings[0], "");
        assert_eq!(decomp_strings[1], "/srv/workspaces/repo--wt-1234");
        assert_eq!(decomp_strings[2], "/srv/workspaces/repo--wt-1234/src/main.cpp");
        assert_eq!(decomp_strings[3], "file:///srv/workspaces/repo--wt-1234/src/main.cpp");
        assert_eq!(decomp_strings[4], "compute_magic");
    }

    #[test]
    fn test_clangd_relocation_resolves_primary_tu_not_included_header() {
        let from = Path::new("/srv/workspaces/project");
        let to = Path::new("/srv/workspaces/project--wt-1");

        let header_path = "/srv/workspaces/project/include/a.h";
        let source_path = "/srv/workspaces/project/src/z_main.cpp";

        // a.h is alphabetically before z_main.cpp and appears first in the sorted table
        let strings = vec![
            "".to_string(),
            header_path.to_string(),
            format!("file://{header_path}"),
            source_path.to_string(),
            format!("file://{source_path}"),
            "z_main_func".to_string(),
        ];
        let mut uncompressed = Vec::new();
        for s in &strings {
            uncompressed.extend_from_slice(s.as_bytes());
            uncompressed.push(0);
        }

        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::<u8>::new(), flate2::Compression::default());
        encoder.write_all(&uncompressed).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut stri_data = Vec::new();
        stri_data.extend_from_slice(&(uncompressed.len() as u32).to_le_bytes());
        stri_data.extend_from_slice(&compressed);

        let mut riff = Vec::new();
        riff.extend_from_slice(b"RIFF");
        let total_len = 4 + 8 + 4 + (8 + stri_data.len());
        riff.extend_from_slice(&(total_len as u32).to_le_bytes());
        riff.extend_from_slice(b"CdIx");
        riff.extend_from_slice(b"meta");
        riff.extend_from_slice(&4u32.to_le_bytes());
        riff.extend_from_slice(&21u32.to_le_bytes());
        riff.extend_from_slice(b"stri");
        let stri_len = stri_data.len() as u32;
        riff.extend_from_slice(&stri_len.to_le_bytes());
        riff.extend_from_slice(&stri_data);
        if stri_len % 2 != 0 {
            riff.push(0);
        }

        // Shard filename created by clangd for z_main.cpp
        let source_digest = clangd_path_digest(source_path);
        let shard_name = format!("z_main.cpp.{source_digest}.idx");

        let (relocated, resolved_source) =
            relocate_shard(&riff, from, to, Some(&shard_name)).unwrap();

        // Must resolve to z_main.cpp in the new worktree, NOT a.h!
        assert_eq!(
            resolved_source,
            Some(PathBuf::from("/srv/workspaces/project--wt-1/src/z_main.cpp"))
        );

        // Derive target shard filename
        let target_shard = shard_filename_for_path(&resolved_source.unwrap()).unwrap();
        let expected_target_digest =
            clangd_path_digest("/srv/workspaces/project--wt-1/src/z_main.cpp");
        assert_eq!(
            target_shard,
            format!("z_main.cpp.{expected_target_digest}.idx")
        );
        assert!(!target_shard.starts_with("a.h"));

        // Verify string table contents and index 0 sentinel
        let mut decoder = flate2::read::ZlibDecoder::new(&relocated[36..]);
        let mut decomp = Vec::new();
        decoder.read_to_end(&mut decomp).unwrap();
        let bytes = decomp.strip_suffix(&[0]).unwrap();
        let decomp_strings: Vec<String> = bytes
            .split(|&b| b == 0)
            .map(|s| String::from_utf8_lossy(s).to_string())
            .collect();

        assert_eq!(decomp_strings.len(), strings.len());
        assert_eq!(decomp_strings[0], "");
        assert_eq!(decomp_strings[1], "/srv/workspaces/project--wt-1/include/a.h");
        assert_eq!(decomp_strings[3], "/srv/workspaces/project--wt-1/src/z_main.cpp");
    }

    #[test]
    fn test_clangd_relocation_rejects_ambiguous_shard() {
        let from = Path::new("/srv/workspaces/project");
        let to = Path::new("/srv/workspaces/project--wt-1");

        // Two source files without shard name identity -> ambiguous
        let strings = vec![
            "".to_string(),
            "/srv/workspaces/project/src/first.cpp".to_string(),
            "/srv/workspaces/project/src/second.cpp".to_string(),
        ];
        let mut uncompressed = Vec::new();
        for s in &strings {
            uncompressed.extend_from_slice(s.as_bytes());
            uncompressed.push(0);
        }

        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::<u8>::new(), flate2::Compression::default());
        encoder.write_all(&uncompressed).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut stri_data = Vec::new();
        stri_data.extend_from_slice(&(uncompressed.len() as u32).to_le_bytes());
        stri_data.extend_from_slice(&compressed);

        let mut riff = Vec::new();
        riff.extend_from_slice(b"RIFF");
        let total_len = 4 + 8 + 4 + (8 + stri_data.len());
        riff.extend_from_slice(&(total_len as u32).to_le_bytes());
        riff.extend_from_slice(b"CdIx");
        riff.extend_from_slice(b"meta");
        riff.extend_from_slice(&4u32.to_le_bytes());
        riff.extend_from_slice(&21u32.to_le_bytes());
        riff.extend_from_slice(b"stri");
        let stri_len = stri_data.len() as u32;
        riff.extend_from_slice(&stri_len.to_le_bytes());
        riff.extend_from_slice(&stri_data);
        if stri_len % 2 != 0 {
            riff.push(0);
        }

        // Without shard name, 2 sources are ambiguous
        let (_relocated, resolved_source) = relocate_shard(&riff, from, to, None).unwrap();
        assert_eq!(resolved_source, None);

        // With invalid shard name matching neither, also rejected
        let (_relocated, resolved_source) =
            relocate_shard(&riff, from, to, Some("other.cpp.0123456789ABCDEF.idx")).unwrap();
        assert_eq!(resolved_source, None);
    }

    #[test]
    fn test_seed_cpp_worktree_e2e() {
        let temp_from = tempfile::tempdir().unwrap();
        let temp_to = tempfile::tempdir().unwrap();

        let from_root = temp_from.path();
        let to_root = temp_to.path();

        // 1. Setup mock compile_commands.json
        let from_build = from_root.join("build");
        std::fs::create_dir_all(&from_build).unwrap();
        let cdb_content = format!(
            r#"[{{ "directory": "{}/build", "command": "clang++ -c {}/src/lib.cpp", "file": "{}/src/lib.cpp" }}]"#,
            from_root.display(),
            from_root.display(),
            from_root.display()
        );
        std::fs::write(from_build.join("compile_commands.json"), cdb_content).unwrap();

        // 2. Setup mock .cache/clangd/index
        let from_index = from_root.join(".cache").join("clangd").join("index");
        std::fs::create_dir_all(&from_index).unwrap();

        let source_file = from_root.join("src").join("lib.cpp");
        let shard_name = shard_filename_for_path(&source_file).unwrap();

        // Create a minimal synthetic shard for lib.cpp with index 0 sentinel
        let uncompressed = format!("\0{}\0file://{}\0", source_file.display(), source_file.display());
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::<u8>::new(), flate2::Compression::default());
        encoder.write_all(uncompressed.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut stri_data = Vec::new();
        stri_data.extend_from_slice(&(uncompressed.len() as u32).to_le_bytes());
        stri_data.extend_from_slice(&compressed);

        let mut riff = Vec::new();
        riff.extend_from_slice(b"RIFF");
        let total_len = 4 + 8 + 4 + (8 + stri_data.len());
        riff.extend_from_slice(&(total_len as u32).to_le_bytes());
        riff.extend_from_slice(b"CdIx");
        riff.extend_from_slice(b"meta");
        riff.extend_from_slice(&4u32.to_le_bytes());
        riff.extend_from_slice(&21u32.to_le_bytes());
        riff.extend_from_slice(b"stri");
        let stri_len = stri_data.len() as u32;
        riff.extend_from_slice(&stri_len.to_le_bytes());
        riff.extend_from_slice(&stri_data);
        if stri_len % 2 != 0 {
            riff.push(0);
        }

        std::fs::write(from_index.join(&shard_name), &riff).unwrap();

        // 3. Seed to new worktree
        let seeded = seed_cpp_worktree(from_root, to_root).unwrap();
        assert!(seeded.is_some());

        // Verify compile_commands.json in to
        let to_cdb_path = to_root.join("build").join("compile_commands.json");
        assert!(to_cdb_path.is_file());
        let to_cdb = std::fs::read_to_string(to_cdb_path).unwrap();
        assert!(to_cdb.contains(&to_root.display().to_string()));
        assert!(!to_cdb.contains(&from_root.display().to_string()));

        // Verify index shard in to
        let to_source_file = to_root.join("src").join("lib.cpp");
        let expected_to_shard = shard_filename_for_path(&to_source_file).unwrap();
        let to_shard_path = to_root.join(".cache").join("clangd").join("index").join(&expected_to_shard);
        assert!(to_shard_path.is_file(), "expected shard {expected_to_shard} was not created");

        let to_gitignore = to_root.join(".cache").join("clangd").join("index").join(".gitignore");
        assert!(to_gitignore.is_file());
    }
}
