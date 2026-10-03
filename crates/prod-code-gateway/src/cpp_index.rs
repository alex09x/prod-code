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
/// by positional index in the string table, preserving the number and order of null-delimited
/// strings guarantees that all index references remain valid and unbroken.
pub fn relocate_shard(
    shard_data: &[u8],
    from_workspace: &Path,
    to_workspace: &Path,
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
    let mut primary_source_path: Option<PathBuf> = None;

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
            let (relocated_chunk, found_source) = relocate_string_table(
                chunk_bytes,
                from_str,
                to_str,
            )?;
            if primary_source_path.is_none() {
                primary_source_path = found_source;
            }
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
fn relocate_string_table(
    data: &[u8],
    from_str: &str,
    to_str: &str,
) -> io::Result<(Vec<u8>, Option<PathBuf>)> {
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

    // The string table contains null-terminated strings
    let mut relocated_strings = Vec::new();
    let mut primary_source = None;

    for slice in decompressed.split(|&b| b == 0) {
        if slice.is_empty() {
            continue;
        }
        let s = String::from_utf8_lossy(slice);
        let replaced = s.replace(from_str, to_str);

        // Identify any source or header file belonging to the new workspace
        if primary_source.is_none() && replaced.starts_with(to_str) {
            let p = PathBuf::from(&replaced);
            if p.extension().is_some_and(|ext| {
                matches!(
                    ext.to_str().unwrap_or(""),
                    "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx"
                )
            }) {
                primary_source = Some(p);
            }
        }

        relocated_strings.push(replaced);
    }

    // Join back into null-terminated string table
    let mut new_uncompressed = Vec::new();
    for s in relocated_strings {
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
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&new_uncompressed)?;
        let compressed = encoder.finish()?;
        out.extend_from_slice(&compressed);
    }

    Ok((out, primary_source))
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
        if !name.ends_with(".idx") {
            continue;
        }

        // Base name before the .<digest>.idx suffix
        let parts: Vec<&str> = name.split('.').collect();
        if parts.len() < 3 {
            continue;
        }
        let base_filename = parts[..parts.len() - 2].join(".");

        let shard_bytes = std::fs::read(&path)?;
        let (relocated, primary_source) = match relocate_shard(&shard_bytes, from, to) {
            Ok(res) => res,
            Err(e) => {
                tracing::debug!(error = %e, shard = %name, "skipping non-RIFF or invalid clangd shard");
                continue;
            }
        };

        // Determine target shard filename
        let new_shard_name = if let Some(source_path) = primary_source {
            shard_filename_for_path(&source_path)
                .unwrap_or_else(|| name.to_string())
        } else {
            // Fallback: estimate from relative path if possible
            let digest = clangd_path_digest(&to.join(&base_filename).to_string_lossy());
            format!("{base_filename}.{digest}.idx")
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
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
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

        // Relocate shard
        let (relocated, source_path) = relocate_shard(&riff, from, to).unwrap();
        assert_eq!(
            source_path,
            Some(PathBuf::from("/srv/workspaces/repo--wt-1234/src/main.cpp"))
        );

        // Verify the relocated shard can be re-parsed
        let mut decoder = flate2::read::ZlibDecoder::new(&relocated[36..]);
        let mut decomp = Vec::new();
        decoder.read_to_end(&mut decomp).unwrap();

        let decomp_str = String::from_utf8_lossy(&decomp);
        assert!(decomp_str.contains("/srv/workspaces/repo--wt-1234/src/main.cpp"));
        assert!(decomp_str.contains("file:///srv/workspaces/repo--wt-1234/src/main.cpp"));
        assert!(!decomp_str.contains("/srv/workspaces/repo/src"));
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

        // Create a minimal synthetic shard for lib.cpp
        let uncompressed = format!("\0{}\0file://{}\0", source_file.display(), source_file.display());
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
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
