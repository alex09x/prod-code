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

/// The text edits of a `WorkspaceEdit`, per document URI; None when it also creates, renames or
/// deletes files, which a deletion of an item should never do.
pub fn text_edits(edit: &serde_json::Value) -> Option<Vec<(String, Vec<serde_json::Value>)>> {
    let mut out = Vec::new();
    if let Some(changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in changes {
            if change.get("kind").is_some() {
                return None;
            }
            let uri = change.pointer("/textDocument/uri")?.as_str()?.to_string();
            out.push((uri, change.get("edits")?.as_array()?.clone()));
        }
    } else if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, edits) in changes {
            out.push((uri.clone(), edits.as_array()?.clone()));
        }
    }
    Some(out)
}

/// The edits that turn `old` into `new`, one per changed run of lines. The analyzer may answer
/// a deletion with the whole file replaced; two such answers always overlap, while the lines
/// each one really changes usually do not.
pub fn minimal_edits(old: &str, new: &str) -> Vec<serde_json::Value> {
    let diff = similar::TextDiff::from_lines(old, new);
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    diff.ops()
        .iter()
        .filter(|op| op.tag() != similar::DiffTag::Equal)
        .map(|op| {
            let (o, n) = (op.old_range(), op.new_range());
            serde_json::json!({
                "range": {
                    "start": { "line": o.start, "character": 0 },
                    "end": { "line": o.end, "character": 0 }
                },
                "newText": new_lines[n.start..n.end].concat()
            })
        })
        .collect()
}

/// The (start, end) of an LSP range as (line, character) pairs.
fn span(edit: &serde_json::Value) -> Option<((u64, u64), (u64, u64))> {
    let at = |p: &str| {
        Some((
            edit.pointer(&format!("/range/{p}/line"))?.as_u64()?,
            edit.pointer(&format!("/range/{p}/character"))?.as_u64()?,
        ))
    };
    Some((at("start")?, at("end")?))
}

/// Adds the edits of one deletion to `merged` unless one of them overlaps an edit already there.
pub fn merge(
    merged: &mut BTreeMap<String, Vec<serde_json::Value>>,
    deletion: Vec<(String, Vec<serde_json::Value>)>,
) -> bool {
    for (uri, edits) in &deletion {
        let taken = merged.get(uri).map(|v| v.as_slice()).unwrap_or(&[]);
        for e in edits {
            let Some((s, en)) = span(e) else {
                return false;
            };
            if taken
                .iter()
                .filter_map(span)
                .any(|(ts, ten)| s < ten && ts < en.max(s))
            {
                return false;
            }
        }
    }
    for (uri, edits) in deletion {
        merged.entry(uri).or_default().extend(edits);
    }
    true
}
