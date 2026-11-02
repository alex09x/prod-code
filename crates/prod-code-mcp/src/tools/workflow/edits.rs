/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Guarded preview and journaled application for explicit unique block replacements.
//!
//! Callers of [`apply`] must hold the `prod-code` workspace writer/mutation gates for `root`
//! (such as `workspace_mutation_lock(root).write_owned()`). Path, size, SHA-256 fingerprint,
//! and unique-snippet prechecks detect content or filesystem entries that have already changed
//! before writes start, but cannot eliminate arbitrary external writes or path swaps between
//! validation and replacement. Multi-file writes delegate to
//! [`crate::refactor::apply_workspace_edit`] for in-process journaled rollback on write failure;
//! no crash-safe atomicity is promised.

#[cfg(test)]
#[path = "edits_tests.rs"]
mod tests;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fmt::Write as _,
    fs::OpenOptions,
    io::Read as _,
    path::{Component, Path, PathBuf},
};

const MAX_FILES: usize = 8;
const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_SNIPPET_BYTES: usize = 64 * 1024;

/// A single unique snippet replacement requested in an existing workspace file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockEdit {
    pub path: String,
    pub old: String,
    pub new: String,
}

/// A validated replacement paired with the SHA-256 hex digest of the original file bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedFile {
    pub path: String,
    pub base_sha256: String,
    pub old: String,
    pub new: String,
}

/// Non-mutating preview output containing a unified diff and SHA-256-guarded file entries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    pub diff: String,
    pub files: Vec<PreparedFile>,
}

/// Computes an in-memory unified diff and original-byte SHA-256 preconditions without writing.
pub fn preview(root: &Path, edits: &[BlockEdit]) -> Result<Preview> {
    check_batch_bounds(
        edits.len(),
        edits.iter().map(|e| (e.old.as_str(), e.new.as_str())),
    )?;
    let canonical_root = resolve_root(root)?;
    let mut seen = SeenPaths::default();
    let mut diff = String::new();
    let mut files = Vec::with_capacity(edits.len());

    for edit in edits {
        let checked = seen.check_one(&canonical_root, &edit.path, &edit.old, &edit.new, None)?;
        append_unified_diff(&mut diff, &checked.rel, &checked.source, &checked.updated);
        files.push(PreparedFile {
            path: checked.rel,
            base_sha256: checked.base_sha256,
            old: edit.old.clone(),
            new: edit.new.clone(),
        });
    }
    Ok(Preview { diff, files })
}

/// Validates every path, size, SHA-256 fingerprint, and unique snippet before writing via
/// [`crate::refactor::apply_workspace_edit`].
///
/// The caller must hold the `prod-code` workspace writer/mutation gates for `root`. Prechecks
/// detect content that has already diverged, but cannot prevent concurrent external writes
/// between validation and file replacement; no crash-safe atomicity is promised.
pub fn apply(root: &Path, files: &[PreparedFile]) -> Result<Vec<PathBuf>> {
    check_batch_bounds(
        files.len(),
        files.iter().map(|f| (f.old.as_str(), f.new.as_str())),
    )?;
    let canonical_root = resolve_root(root)?;
    let mut seen = SeenPaths::default();
    let mut rewrites = Vec::with_capacity(files.len());

    for file in files {
        let checked = seen.check_one(
            &canonical_root,
            &file.path,
            &file.old,
            &file.new,
            Some(&file.base_sha256),
        )?;
        let end = utf16_end_position(&checked.source);
        rewrites.push((checked.canonical_path, end, checked.updated));
    }

    let workspace_edit = build_workspace_edit(&rewrites);
    let touched = crate::refactor::apply_workspace_edit(&canonical_root, &workspace_edit)?;
    Ok(touched.into_iter().map(PathBuf::from).collect())
}

#[derive(Default)]
struct SeenPaths {
    rel: HashSet<String>,
    canonical: HashSet<PathBuf>,
}

struct CheckedRewrite {
    rel: String,
    canonical_path: PathBuf,
    base_sha256: String,
    source: String,
    updated: String,
}

impl SeenPaths {
    fn check_one(
        &mut self,
        canonical_root: &Path,
        raw_path: &str,
        old: &str,
        new: &str,
        expected_sha256: Option<&str>,
    ) -> Result<CheckedRewrite> {
        let (rel, canonical_path) = resolve_regular_file(canonical_root, raw_path)?;
        ensure!(
            self.rel.insert(rel.clone()) && self.canonical.insert(canonical_path.clone()),
            "duplicate file path in batch: {rel}"
        );
        ensure!(!old.is_empty(), "old snippet must not be empty for {rel}");
        ensure!(old != new, "replacement is a no-op for {rel}");
        let bytes = read_regular_bytes(&canonical_path, &rel)?;
        let base_sha256 = sha256_hex(&bytes);
        if let Some(expected) = expected_sha256 {
            ensure!(
                base_sha256 == expected,
                "SHA-256 precondition mismatch for {rel}: expected {expected}, found {base_sha256}"
            );
        }
        let source =
            String::from_utf8(bytes).with_context(|| format!("{rel} is not valid UTF-8 text"))?;
        let updated = replace_unique(&rel, &source, old, new)?;
        Ok(CheckedRewrite {
            rel,
            canonical_path,
            base_sha256,
            source,
            updated,
        })
    }
}

fn check_batch_bounds<'a, I>(count: usize, snippets: I) -> Result<()>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    ensure!(
        (1..=MAX_FILES).contains(&count),
        "edits require 1..={MAX_FILES} files, got {count}"
    );
    let total = snippets
        .into_iter()
        .try_fold(0usize, |acc, (old, new)| {
            acc.checked_add(old.len())?.checked_add(new.len())
        })
        .context("total snippet size overflowed")?;
    ensure!(
        total <= MAX_SNIPPET_BYTES,
        "total old/new snippet bytes ({total}) exceed {MAX_SNIPPET_BYTES} byte limit"
    );
    Ok(())
}

fn resolve_root(root: &Path) -> Result<PathBuf> {
    let canonical = root
        .canonicalize()
        .with_context(|| format!("cannot resolve workspace root: {}", root.display()))?;
    ensure!(
        canonical.is_dir(),
        "workspace root is not a directory: {}",
        canonical.display()
    );
    Ok(canonical)
}

fn resolve_regular_file(canonical_root: &Path, raw_path: &str) -> Result<(String, PathBuf)> {
    ensure!(!raw_path.is_empty(), "edit path must not be empty");
    ensure!(
        !raw_path.ends_with('/') && !raw_path.ends_with('\\'),
        "edit path must name a regular file, not a directory: {raw_path}"
    );
    let requested = Path::new(raw_path);
    ensure!(
        !requested.is_absolute(),
        "edit path must be relative to the workspace: {raw_path}"
    );
    let mut segments = Vec::new();
    for component in requested.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => segments.push(
                part.to_str()
                    .with_context(|| format!("edit path is not valid UTF-8: {raw_path}"))?,
            ),
            Component::ParentDir => {
                bail!("edit path must not contain parent traversal ('..'): {raw_path}");
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!("edit path must be relative to the workspace: {raw_path}");
            }
        }
    }
    ensure!(
        !segments.is_empty(),
        "edit path must name a regular file: {raw_path}"
    );
    let rel = segments.join("/");
    let mut cursor = canonical_root.to_path_buf();
    let mut leaf = None;
    for segment in &segments {
        cursor.push(segment);
        let metadata = std::fs::symlink_metadata(&cursor)
            .with_context(|| format!("path does not exist: {rel}"))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "symlink component refused in edit path: {rel}"
        );
        leaf = Some(metadata);
    }
    let leaf = leaf.expect("segments is non-empty");
    ensure!(
        leaf.file_type().is_file(),
        "path is not a regular file: {rel}"
    );
    ensure!(
        leaf.len() <= MAX_FILE_BYTES as u64,
        "file exceeds {MAX_FILE_BYTES} byte limit: {rel}"
    );
    let canonical = cursor
        .canonicalize()
        .with_context(|| format!("cannot resolve path: {rel}"))?;
    ensure!(
        canonical.starts_with(canonical_root) && canonical.as_path() != canonical_root,
        "path escapes workspace root: {rel}"
    );
    Ok((rel, canonical))
}

fn read_regular_bytes(target: &Path, rel: &str) -> Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(target)
        .with_context(|| format!("cannot open {rel}"))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("cannot stat open file {rel}"))?;
    ensure!(
        metadata.file_type().is_file(),
        "path is not a regular file: {rel}"
    );
    ensure!(
        metadata.len() <= MAX_FILE_BYTES as u64,
        "file exceeds {MAX_FILE_BYTES} byte limit: {rel}"
    );
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("cannot read {rel}"))?;
    ensure!(
        bytes.len() <= MAX_FILE_BYTES,
        "file exceeds {MAX_FILE_BYTES} byte limit: {rel}"
    );
    Ok(bytes)
}

fn replace_unique(rel: &str, source: &str, old: &str, new: &str) -> Result<String> {
    let Some(first) = source.find(old) else {
        bail!("old snippet not found in {rel}");
    };
    let step = old.chars().next().map_or(1, char::len_utf8);
    if source[first + step..].contains(old) {
        bail!("old snippet occurs multiple times in {rel}");
    }
    let mut updated = String::with_capacity(source.len() - old.len() + new.len());
    updated.push_str(&source[..first]);
    updated.push_str(new);
    updated.push_str(&source[first + old.len()..]);
    Ok(updated)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn utf16_end_position(text: &str) -> (u64, u64) {
    match text.rsplit_once('\n') {
        Some((before, tail)) => (
            before.bytes().filter(|&b| b == b'\n').count() as u64 + 1,
            tail.encode_utf16().count() as u64,
        ),
        None => (0, text.encode_utf16().count() as u64),
    }
}

fn append_unified_diff(out: &mut String, rel: &str, old_text: &str, new_text: &str) {
    let diff = similar::TextDiff::from_lines(old_text, new_text);
    let (old_hdr, new_hdr) = (format!("a/{rel}"), format!("b/{rel}"));
    out.push_str(
        &diff
            .unified_diff()
            .context_radius(3)
            .header(&old_hdr, &new_hdr)
            .to_string(),
    );
}

fn build_workspace_edit(rewrites: &[(PathBuf, (u64, u64), String)]) -> Value {
    let changes: Vec<Value> = rewrites
        .iter()
        .map(|(path, (end_line, end_char), new_text)| {
            json!({
                "textDocument": { "uri": prod_code_protocol::path::file_uri(path), "version": null },
                "edits": [{
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": *end_line, "character": *end_char },
                    },
                    "newText": new_text,
                }],
            })
        })
        .collect();
    json!({ "documentChanges": changes })
}
