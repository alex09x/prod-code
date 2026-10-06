/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use super::relocate::relocate_arg_token;
use super::seed::identify_primary_source;

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
pub(crate) struct RiffChunk {
    pub(crate) tag: [u8; 4],
    pub(crate) data: Vec<u8>,
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
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "from_workspace is not valid UTF-8",
        )
    })?;
    let to_str = to_workspace.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "to_workspace is not valid UTF-8",
        )
    })?;

    let mut chunks = Vec::new();
    let mut offset = 12;
    let mut orig_strings_table = Vec::new();
    let mut relocated_strings_table = Vec::new();

    while offset + 8 <= shard_data.len() {
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&shard_data[offset..offset + 4]);
        let length =
            u32::from_le_bytes(shard_data[offset + 4..offset + 8].try_into().unwrap()) as usize;
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
            let (relocated_chunk, orig_strings, relocated_strings) =
                relocate_string_table(chunk_bytes, from_str, to_str)?;
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
pub(crate) fn relocate_string_table(
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
        let replaced = relocate_arg_token(&orig, from_str, to_str);
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
