//! Pruning orphans (roadmap 8.6): every function and type the dead-code scan finds unreferenced
//! is removed with the analyzer's safe delete, all in one edit that is type-checked before
//! anything is written.
//!
//! Only what the scan calls dead is taken: exported items (something outside the checkout may
//! use them) and methods that may be reached through a trait are left alone. Each deletion is
//! computed against the checkout as it is; one that overlaps another is left for the next run.
//! Removing a function can orphan the functions only it called, so a second run may find more.

use anyhow::Result;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::dead_code::DeadItem;

/// What the pruning did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Pruned {
    #[serde(skip)]
    pub root: PathBuf,
    pub removed: Vec<DeadItem>,
    /// Items the scan listed that were not removed, and why.
    pub skipped: Vec<(DeadItem, String)>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
    pub symbols_checked: usize,
}

impl Pruned {
    pub fn render(&self) -> String {
        let mut out = format!(
            "{} orphan(s) of {} symbol(s) checked\n",
            self.removed.len(),
            self.symbols_checked
        );
        for d in &self.removed {
            out.push_str(&format!(
                "  - {} {} ({}:{})\n",
                d.kind, d.name, d.file, d.line
            ));
        }
        for (d, why) in &self.skipped {
            out.push_str(&format!(
                "  kept {} {} ({}:{}): {why}\n",
                d.kind, d.name, d.file, d.line
            ));
        }
        for (path, new_text) in &self.rewritten {
            let full = Path::new(path);
            let old_text = if self.applied {
                crate::refactor::text_before_apply(full)
            } else {
                std::fs::read_to_string(full).unwrap_or_default()
            };
            let rel = full
                .strip_prefix(&self.root)
                .unwrap_or(full)
                .display()
                .to_string();
            out.push('\n');
            out.push_str(
                &similar::TextDiff::from_lines(old_text.as_str(), new_text.as_str())
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        if self.removed.is_empty() {
            out.push_str("\nnothing to prune\n");
            return out;
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        out.push_str(if self.applied {
            "\n[applied] a second run may find what these removals orphaned\n"
        } else {
            "\nnothing was written; pass `apply: true` to make this edit\n"
        });
        out
    }
}

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

/// Removes every orphan the dead-code scan finds in the checkout at `root`.
pub async fn prune_orphans(
    remote: SocketAddr,
    root: &Path,
    max_files: usize,
    apply: bool,
    force: bool,
) -> Result<Pruned> {
    let report = crate::dead_code::find_dead_code(remote, root, false, max_files).await?;
    let mut merged: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    let mut removed = Vec::new();
    let mut skipped = Vec::new();
    for item in report.dead {
        let file = root.join(&item.file);
        let uri = url::Url::from_file_path(&file)
            .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
            .to_string();
        let answer = crate::tools::execute_lsp_query(
            remote,
            root,
            &file,
            "prodCode/safeDelete",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": item.line.saturating_sub(1), "character": item.col.saturating_sub(1) }
            }),
        )
        .await;
        let deletion = match answer.as_ref().ok().and_then(text_edits) {
            Some(d) if !d.is_empty() => d,
            Some(_) => {
                skipped.push((item, "safe delete produced no edit".to_string()));
                continue;
            }
            None => {
                let why = match answer {
                    Err(e) => format!("safe delete refused: {e:#}"),
                    Ok(_) => "safe delete would move or create files".to_string(),
                };
                skipped.push((item, why));
                continue;
            }
        };
        // Each answer, reduced to the lines it changes in the file as it is.
        let mut reduced = Vec::with_capacity(deletion.len());
        for (uri, edits) in deletion {
            let path = crate::remote_fs::uri_to_path(&uri);
            let old = std::fs::read_to_string(&path).unwrap_or_default();
            let new = crate::refactor::apply_text_edits(&old, &edits)?;
            reduced.push((uri, minimal_edits(&old, &new)));
        }
        if merge(&mut merged, reduced) {
            removed.push(item);
        } else {
            skipped.push((item, "it overlaps another removal; run again".to_string()));
        }
    }
    let edit = serde_json::json!({ "changes": merged });
    let (texts, _) = crate::refactor::planned_texts(root, &edit)?;
    let mut diagnostics = Vec::new();
    if !texts.is_empty() {
        let reports = crate::diagnostics::validate_texts(remote, root, &texts, &[]).await?;
        diagnostics = reports
            .iter()
            .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
            .filter(|(_, d)| d.severity == "error")
            .map(|(f, d)| {
                format!(
                    "{}{} ({f}:{}:{})",
                    d.message.lines().next().unwrap_or(""),
                    d.code
                        .as_deref()
                        .map(|c| format!(" [{c}]"))
                        .unwrap_or_default(),
                    d.line,
                    d.col
                )
            })
            .collect();
    }
    let files: BTreeMap<PathBuf, String> = texts.into_iter().collect();
    let mut applied = false;
    if apply && !files.is_empty() {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the pruned checkout does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }
    Ok(Pruned {
        root: root.to_path_buf(),
        removed,
        skipped,
        rewritten: files
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
        symbols_checked: report.symbols_checked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(sl: u64, sc: u64, el: u64, ec: u64) -> serde_json::Value {
        serde_json::json!({
            "range": { "start": { "line": sl, "character": sc }, "end": { "line": el, "character": ec } },
            "newText": ""
        })
    }

    #[test]
    fn a_deletion_that_overlaps_another_is_left_for_the_next_run() {
        let mut merged = BTreeMap::new();
        assert!(merge(
            &mut merged,
            vec![("a".into(), vec![edit(0, 0, 3, 0)])]
        ));
        assert!(merge(
            &mut merged,
            vec![("a".into(), vec![edit(3, 0, 5, 0)])]
        ));
        assert!(!merge(
            &mut merged,
            vec![("a".into(), vec![edit(2, 0, 4, 0)])]
        ));
        assert!(merge(
            &mut merged,
            vec![("b".into(), vec![edit(2, 0, 4, 0)])]
        ));
        assert_eq!(merged["a"].len(), 2);
    }

    #[test]
    fn two_whole_file_answers_reduce_to_edits_that_do_not_overlap() {
        let old = "a\nfn one() {}\nb\nfn two() {}\nc\n";
        let first = minimal_edits(old, "a\nb\nfn two() {}\nc\n");
        let second = minimal_edits(old, "a\nfn one() {}\nb\nc\n");
        let mut merged = BTreeMap::new();
        assert!(merge(&mut merged, vec![("f".into(), first)]));
        assert!(merge(&mut merged, vec![("f".into(), second)]));
        assert_eq!(
            crate::refactor::apply_text_edits(old, &merged["f"]).unwrap(),
            "a\nb\nc\n"
        );
    }

    #[test]
    fn only_text_edits_are_taken_from_an_answer() {
        let changes = serde_json::json!({ "changes": { "file:///x.rs": [edit(0, 0, 1, 0)] } });
        assert_eq!(text_edits(&changes).unwrap().len(), 1);
        let doc = serde_json::json!({ "documentChanges": [
            { "textDocument": { "uri": "file:///x.rs", "version": null }, "edits": [edit(0, 0, 1, 0)] }
        ] });
        assert_eq!(text_edits(&doc).unwrap()[0].0, "file:///x.rs");
        let moves = serde_json::json!({ "documentChanges": [ { "kind": "delete", "uri": "file:///x.rs" } ] });
        assert!(text_edits(&moves).is_none());
    }
}
