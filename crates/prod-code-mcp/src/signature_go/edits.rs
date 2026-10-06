/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature_go::text::offset_at;
use crate::signature_go::types::TextEdit;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

/// Refuses a path that is not an existing file under the checkout.
pub(crate) fn ensure_inside(canonical_root: &Path, path: &Path) -> Result<()> {
    let real = std::fs::canonicalize(path).with_context(|| {
        format!(
            "{} is not a file in the checkout; nothing was written",
            path.display()
        )
    })?;
    anyhow::ensure!(
        real.starts_with(canonical_root),
        "{} is outside the checkout {}; nothing was written",
        path.display(),
        canonical_root.display()
    );
    Ok(())
}

/// The text edits of gopls's answer, per file, as byte ranges of the file as it is. A file
/// outside the checkout, a file operation, overlapping edits, or an answer that is not
/// well-formed (a file's edits that are not a list, a position that does not fit the protocol's
/// unsigned 32-bit integers or the file) stop the change: a malformed part is never read as
/// "no edits" or as another position.
pub(crate) fn edits_by_file(
    canonical_root: &Path,
    edit: &serde_json::Value,
    originals: &mut BTreeMap<PathBuf, String>,
) -> Result<BTreeMap<PathBuf, Vec<TextEdit>>> {
    let mut raw: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    if let Some(changes) = edit.get("documentChanges").filter(|c| !c.is_null()) {
        let changes = changes.as_array().with_context(|| {
            format!("gopls's `documentChanges` is not a list: {changes}; nothing was written")
        })?;
        for change in changes {
            anyhow::ensure!(
                change.get("kind").is_none(),
                "gopls proposed creating, renaming or deleting a file ({}); nothing was written",
                change
            );
            let uri = change
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .context("a change in gopls's edit names no file; nothing was written")?;
            let list = change
                .get("edits")
                .and_then(|e| e.as_array())
                .cloned()
                .with_context(|| {
                    format!(
                        "gopls's change to {uri} has no list of edits: {change}; nothing was \
                         written"
                    )
                })?;
            raw.push((uri.to_string(), list));
        }
    } else if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, list) in changes {
            let list = list.as_array().cloned().with_context(|| {
                format!("gopls's edits for {uri} are not a list: {list}; nothing was written")
            })?;
            raw.push((uri.clone(), list));
        }
    } else {
        anyhow::bail!("gopls's answer is not a workspace edit: {edit}; nothing was written");
    }
    let mut out: BTreeMap<PathBuf, Vec<TextEdit>> = BTreeMap::new();
    for (uri, list) in raw {
        let path = PathBuf::from(crate::remote_fs::uri_to_path(&uri));
        ensure_inside(canonical_root, &path)?;
        // The same file under the spelling the references used, when it is one of them.
        let key = originals
            .keys()
            .find(|k| same_file(k, &path))
            .cloned()
            .unwrap_or_else(|| path.clone());
        if !originals.contains_key(&key) {
            let t = std::fs::read_to_string(&key)
                .with_context(|| format!("cannot read {}; nothing was written", key.display()))?;
            originals.insert(key.clone(), t);
        }
        let text = &originals[&key];
        let entry = out.entry(key.clone()).or_default();
        for e in list {
            // A position past `u32::MAX` is not a protocol position; truncating it would read
            // as a small one and splice another place in the file.
            let at = |p: &str| {
                let number = |field: &str| {
                    e.pointer(&format!("/range/{p}/{field}"))
                        .and_then(|v| v.as_u64())
                        .and_then(|v| u32::try_from(v).ok())
                };
                number("line")
                    .zip(number("character"))
                    .and_then(|(l, c)| offset_at(text, l, c))
            };
            let new_text = e.get("newText").and_then(|t| t.as_str());
            let (Some(s), Some(end), Some(new_text)) = (at("start"), at("end"), new_text) else {
                anyhow::bail!(
                    "an edit gopls proposed for {} is out of range or malformed: {e}; nothing was \
                     written",
                    key.display()
                );
            };
            anyhow::ensure!(s <= end, "gopls proposed an inverted range: {e}");
            entry.push((s, end, new_text.to_string()));
        }
        entry.sort_by_key(|(s, e, _)| (*s, *e));
        for pair in entry.windows(2) {
            anyhow::ensure!(
                pair[0].1 <= pair[1].0,
                "gopls proposed overlapping edits in {}; nothing was written",
                key.display()
            );
        }
    }
    Ok(out)
}
