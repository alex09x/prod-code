/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use url::Url;

/// The text of a file the analyzer reports a reference in, read once into `texts`. A file that
/// cannot be read is an error: taken as empty, its references would not be in it, and the plan
/// would go on as if they did not exist (#446).
pub fn referenced_text<'a>(
    texts: &'a mut BTreeMap<PathBuf, String>,
    path: &Path,
) -> Result<&'a mut String> {
    match texts.entry(path.to_path_buf()) {
        Entry::Occupied(known) => Ok(known.into_mut()),
        Entry::Vacant(slot) => {
            let text = std::fs::read_to_string(path).with_context(|| {
                format!(
                    "cannot read {}, where the analyzer reports a reference; nothing was planned",
                    path.display()
                )
            })?;
            Ok(slot.insert(text))
        }
    }
}

/// The locations of a `definition`, `declaration` or `implementation` answer, as (file, 1-based
/// line, 1-based column): `null` is none, and a `Location`, a `LocationLink` or a list of either
/// is read whole. Any other answer, and an entry without a file or a start, is an error naming
/// `what` was asked: a planner that dropped it would leave that declaration as it was (#446).
pub fn lsp_locations(answer: &serde_json::Value, what: &str) -> Result<Vec<(PathBuf, u32, u32)>> {
    let entries = match answer {
        serde_json::Value::Null => return Ok(Vec::new()),
        serde_json::Value::Array(all) => all.iter().collect(),
        one @ serde_json::Value::Object(_) => vec![one],
        other => anyhow::bail!("the analyzer's {what} is not a location or a list: {other}"),
    };
    let mut out = Vec::with_capacity(entries.len());
    for (n, loc) in entries.iter().enumerate() {
        let uri = loc.get("uri").or_else(|| loc.get("targetUri"));
        let start = loc
            .pointer("/range/start")
            .or_else(|| loc.pointer("/targetSelectionRange/start"));
        let at = |key: &str| {
            start
                .and_then(|s| s.get(key))
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
                .and_then(|v| v.checked_add(1))
        };
        let (Some(uri), Some(line), Some(col)) =
            (uri.and_then(|u| u.as_str()), at("line"), at("character"))
        else {
            anyhow::bail!(
                "entry {} of {} in the analyzer's {what} has no file or start position: {loc}",
                n + 1,
                entries.len()
            );
        };
        let parsed = Url::parse(uri)
            .with_context(|| format!("invalid URI in the analyzer's {what}: {uri}"))?;
        anyhow::ensure!(
            parsed.scheme() == "file" && parsed.query().is_none() && parsed.fragment().is_none(),
            "the analyzer's {what} does not name a plain local file: {uri}"
        );
        let path = parsed.to_file_path().map_err(|_| {
            anyhow::anyhow!("the analyzer's {what} does not name a local file: {uri}")
        })?;
        out.push((path, line, col));
    }
    Ok(out)
}
