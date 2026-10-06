/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::cpp_index::shard::{clangd_path_digest, relocate_shard, shard_filename_for_path};

#[test]
fn test_clangd_digest_format_and_properties() {
    let path = "/srv/workspaces/repo/src/main.cpp";
    let digest = clangd_path_digest(path);
    assert_eq!(digest.len(), 16);
    assert!(
        digest
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
    );

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
        "/srv/workspaces/repository-deps/include".to_string(),
        "file:///srv/workspaces/repo-deps/include".to_string(),
        "-I/srv/workspaces/repo-deps/include".to_string(),
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
    assert_eq!(
        decomp_strings[2],
        "/srv/workspaces/repo--wt-1234/src/main.cpp"
    );
    assert_eq!(
        decomp_strings[3],
        "file:///srv/workspaces/repo--wt-1234/src/main.cpp"
    );
    assert_eq!(decomp_strings[4], "/srv/workspaces/repository-deps/include");
    assert_eq!(
        decomp_strings[5],
        "file:///srv/workspaces/repo-deps/include"
    );
    assert_eq!(decomp_strings[6], "-I/srv/workspaces/repo-deps/include");
    assert_eq!(decomp_strings[7], "compute_magic");
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

    let (relocated, resolved_source) = relocate_shard(&riff, from, to, Some(&shard_name)).unwrap();

    // Must resolve to z_main.cpp in the new worktree, NOT a.h!
    assert_eq!(
        resolved_source,
        Some(PathBuf::from(
            "/srv/workspaces/project--wt-1/src/z_main.cpp"
        ))
    );

    // Derive target shard filename
    let target_shard = shard_filename_for_path(&resolved_source.unwrap()).unwrap();
    let expected_target_digest = clangd_path_digest("/srv/workspaces/project--wt-1/src/z_main.cpp");
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
    assert_eq!(
        decomp_strings[1],
        "/srv/workspaces/project--wt-1/include/a.h"
    );
    assert_eq!(
        decomp_strings[3],
        "/srv/workspaces/project--wt-1/src/z_main.cpp"
    );
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
