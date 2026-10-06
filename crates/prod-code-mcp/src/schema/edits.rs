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
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::detect::{Kind, kind_of};
use super::types::{Occurrence, Variant, lsp_end_position};

pub(crate) const NOT_FOUND: &str = "does not appear under";

/// Builds a standard LSP `WorkspaceEdit` (`documentChanges`) representing the rewritten files.
pub fn make_workspace_edit(rewritten: &[(PathBuf, String)]) -> serde_json::Value {
    make_workspace_edit_with_ends(rewritten, &BTreeMap::new())
}

/// Builds a standard LSP `WorkspaceEdit` (`documentChanges`) using known pre-apply end positions.
pub fn make_workspace_edit_with_ends(
    rewritten: &[(PathBuf, String)],
    known_ends: &BTreeMap<PathBuf, (u32, u32)>,
) -> serde_json::Value {
    let changes: Vec<serde_json::Value> = rewritten
        .iter()
        .map(|(path, new_text)| {
            let (end_line, end_char) = known_ends.get(path).copied().unwrap_or_else(|| {
                let text = crate::refactor::text_before_apply(path);
                lsp_end_position(&text)
            });
            serde_json::json!({
                "textDocument": { "uri": prod_code_protocol::path::file_uri(path), "version": null },
                "edits": [ {
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": end_line, "character": end_char }
                    },
                    "newText": new_text
                } ]
            })
        })
        .collect();
    serde_json::json!({ "documentChanges": changes })
}

/// Builds a standard LSP `WorkspaceEdit` (`documentChanges`) using known pre-apply line counts.
pub fn make_workspace_edit_with_lines(
    rewritten: &[(PathBuf, String)],
    known_lines: &BTreeMap<PathBuf, usize>,
) -> serde_json::Value {
    let ends: BTreeMap<PathBuf, (u32, u32)> = known_lines
        .iter()
        .map(|(p, l)| (p.clone(), (*l as u32, 0)))
        .collect();
    make_workspace_edit_with_ends(rewritten, &ends)
}

/// Writes every rewritten file of the checkout at `root` in one edit.
pub(crate) fn write_rewritten(
    root: &Path,
    rewritten: &[(PathBuf, String)],
    ends: &BTreeMap<PathBuf, (u32, u32)>,
) -> Result<()> {
    let edit = make_workspace_edit_with_ends(rewritten, ends);
    crate::refactor::apply_workspace_edit(root, &edit)?;
    Ok(())
}

pub(crate) fn still_spelled(text: &str, occurrence: &Occurrence, variant: &Variant) -> bool {
    text.lines()
        .nth(occurrence.line as usize - 1)
        .map(|line| {
            let at: String = line
                .chars()
                .skip(occurrence.col as usize - 1)
                .take(occurrence.len)
                .collect();
            at == variant.from
        })
        .unwrap_or(false)
}

/// Do two edit ranges want any of the same characters?
pub(crate) fn overlaps(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> bool {
    let (a_start, a_end) = ((a.0, a.1), (a.2, a.3));
    let (b_start, b_end) = ((b.0, b.1), (b.2, b.3));
    a_start < b_end && b_start < a_end
}

/// Is this occurrence inside a comment? A rename does not follow a name into prose, and
/// neither does this: such an occurrence is reported instead of quietly rewritten.
///
/// Which marker starts a comment depends on the language, and `#` in particular is a comment
/// in Python and an attribute in Rust, so it is not treated as one for a Rust file.
pub(crate) fn in_comment(texts: &BTreeMap<PathBuf, String>, o: &Occurrence) -> bool {
    let Some(text) = texts.get(&o.file) else {
        return false;
    };
    let Some(line) = text.lines().nth(o.line as usize - 1) else {
        return false;
    };
    let prefix: String = line.chars().take(o.col as usize - 1).collect();
    let markers: &[&str] = match kind_of(&o.file) {
        Kind::Code("python") => &["#"],
        Kind::Code("rust")
        | Kind::Code("go")
        | Kind::Code("typescript")
        | Kind::Code("javascript")
        | Kind::Code("swift")
        | Kind::Code("c/c++") => &["//", "/*"],
        Kind::Text("sql") => &["--"],
        Kind::Text("python") | Kind::Text("shell") | Kind::Text("yaml") | Kind::Text("toml") => {
            &["#"]
        }
        _ => &[],
    };
    markers.iter().any(|m| prefix.contains(m))
}

/// The text edits of a workspace edit, per file, and whether they replace the file wholesale.
///
/// The two shapes are not interchangeable: the forwarded language servers answer a rename with
/// one edit per occurrence, while our in-process Rust engine answers with the file's whole new
/// text. Merging the second with anything else produces nonsense, so it is flagged here and
/// handled separately.
pub(crate) fn ranged_edits(
    edit: &serde_json::Value,
) -> Vec<(PathBuf, Vec<serde_json::Value>, bool)> {
    let mut out = Vec::new();
    let changes = edit
        .get("documentChanges")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    for change in changes {
        let Some(uri) = change.pointer("/textDocument/uri").and_then(|u| u.as_str()) else {
            continue;
        };
        let Some(list) = change.get("edits").and_then(|e| e.as_array()) else {
            continue;
        };
        out.push((
            PathBuf::from(crate::remote_fs::uri_to_path(uri)),
            list.clone(),
            replaces_whole_file(list),
        ));
    }
    if out.is_empty()
        && let Some(map) = edit.get("changes").and_then(|c| c.as_object())
    {
        for (uri, list) in map {
            if let Some(list) = list.as_array() {
                out.push((
                    PathBuf::from(crate::remote_fs::uri_to_path(uri)),
                    list.clone(),
                    replaces_whole_file(list),
                ));
            }
        }
    }
    out
}

/// One edit that starts at the top of the file and ends at the start of a later line is a
/// whole-file replacement; a rename's edit covers one identifier and never looks like that.
pub(crate) fn replaces_whole_file(edits: &[serde_json::Value]) -> bool {
    match edits {
        [only] => {
            let (sl, sc, el, ec) = span_of(only);
            sl == 0 && sc == 0 && ec == 0 && el >= 1
        }
        _ => false,
    }
}

/// A text edit's range as (start line, start character, end line, end character), 0-based.
pub(crate) fn span_of(edit: &serde_json::Value) -> (u32, u32, u32, u32) {
    let at = |p: &str| edit.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    (
        at("/range/start/line"),
        at("/range/start/character"),
        at("/range/end/line"),
        at("/range/end/character"),
    )
}

/// One semantic rename, on the session of the project the file belongs to.
pub(crate) async fn rename_symbol(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    new_name: &str,
) -> Result<serde_json::Value> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/rename",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "newName": new_name,
        }),
    )
    .await
}
