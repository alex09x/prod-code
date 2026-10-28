/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use prod_code_protocol::{ARTIFACT_LIST_LIMIT, ArtifactEntry, ArtifactListing};
use std::path::Path;

/// Lists one directory without following its entries, bounded even on large caches.
pub(super) fn list_directory(path: &Path) -> Result<ArtifactListing> {
    let mut entries = Vec::new();
    let mut truncated = false;
    for (index, entry) in std::fs::read_dir(path)?.enumerate() {
        if index >= ARTIFACT_LIST_LIMIT {
            truncated = true;
            break;
        }
        let entry = entry?;
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => {
                truncated = true;
                continue;
            }
        };
        let kind = entry.file_type()?;
        let kind = if kind.is_symlink() {
            "symlink"
        } else if kind.is_dir() {
            "directory"
        } else if kind.is_file() {
            "file"
        } else {
            "other"
        };
        entries.push(ArtifactEntry {
            name,
            kind: kind.into(),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ArtifactListing { entries, truncated })
}
