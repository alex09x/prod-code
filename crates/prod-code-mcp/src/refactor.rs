//! Applying gateway refactorings (LSP `WorkspaceEdit`) to the local checkout. Rewritten files
//! are recorded in the sync watermark so the next pre-flight does not push them back.

use crate::sync::apply_pulled_files;
use anyhow::{Context, Result, anyhow};
use prod_code_protocol::FileDelta;
use std::path::Path;
use url::Url;

fn uri_to_relative(root: &Path, uri: &str) -> Result<String> {
    let path = Url::parse(uri)
        .ok()
        .and_then(|u| u.to_file_path().ok())
        .ok_or_else(|| anyhow!("not a file URI: {uri}"))?;
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let rel = path.strip_prefix(root).map_err(|_| {
        anyhow!(
            "{} is outside the checkout {}",
            path.display(),
            root.display()
        )
    })?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

/// Applies LSP text edits (0-based line/character, character counted in chars) to `text`.
/// A single edit starting at 0:0 and ending at or past the last line replaces the whole file.
fn apply_text_edits(text: &str, edits: &[serde_json::Value]) -> Result<String> {
    let line_count = text.lines().count() as u64;
    if let [edit] = edits
        && edit.pointer("/range/start/line").and_then(|v| v.as_u64()) == Some(0)
        && edit
            .pointer("/range/start/character")
            .and_then(|v| v.as_u64())
            == Some(0)
        && edit
            .pointer("/range/end/line")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            >= line_count
    {
        return Ok(edit
            .get("newText")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string());
    }
    // General case: convert positions to char offsets and apply from the end backwards.
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(
            text.char_indices()
                .filter(|(_, c)| *c == '\n')
                .map(|(i, _)| i + 1),
        )
        .collect();
    let offset = |line: u64, character: u64| -> usize {
        let start = line_starts
            .get(line as usize)
            .copied()
            .unwrap_or(text.len());
        let rest = &text[start..];
        let mut it = rest.char_indices();
        let end_of_line = rest.find('\n').unwrap_or(rest.len());
        it.nth(character as usize)
            .map(|(i, _)| start + i.min(end_of_line))
            .unwrap_or(start + end_of_line)
    };
    let mut spans: Vec<(usize, usize, String)> = edits
        .iter()
        .map(|e| {
            let sl = e
                .pointer("/range/start/line")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let sc = e
                .pointer("/range/start/character")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let el = e
                .pointer("/range/end/line")
                .and_then(|v| v.as_u64())
                .unwrap_or(sl);
            let ec = e
                .pointer("/range/end/character")
                .and_then(|v| v.as_u64())
                .unwrap_or(sc);
            let new_text = e
                .get("newText")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string();
            (offset(sl, sc), offset(el, ec), new_text)
        })
        .collect();
    spans.sort_by_key(|span| std::cmp::Reverse(span.0));
    let mut out = text.to_string();
    for (start, end, new_text) in spans {
        if start > end || end > out.len() {
            return Err(anyhow!("text edit range {start}..{end} out of bounds"));
        }
        out.replace_range(start..end, &new_text);
    }
    Ok(out)
}

/// Applies a `WorkspaceEdit` (`documentChanges` or `changes`) to the checkout at `root`.
/// Returns the relative paths written, moved or deleted, in application order.
pub fn apply_workspace_edit(root: &Path, edit: &serde_json::Value) -> Result<Vec<String>> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut deltas: Vec<FileDelta> = Vec::new();
    let mut touched = Vec::new();

    let mut text_edits: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    if let Some(changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in changes {
            match change.get("kind").and_then(|k| k.as_str()) {
                Some("rename") => {
                    let from = uri_to_relative(
                        &root,
                        change.get("oldUri").and_then(|u| u.as_str()).unwrap_or(""),
                    )?;
                    let to = uri_to_relative(
                        &root,
                        change.get("newUri").and_then(|u| u.as_str()).unwrap_or(""),
                    )?;
                    let to_abs = root.join(&to);
                    if let Some(parent) = to_abs.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::rename(root.join(&from), &to_abs)
                        .with_context(|| format!("rename {from} -> {to}"))?;
                    deltas.push(FileDelta {
                        relative_path: from.clone(),
                        content: None,
                        is_executable: false,
                    });
                    if to_abs.is_file() {
                        deltas.push(FileDelta {
                            relative_path: to.clone(),
                            content: Some(std::fs::read(&to_abs)?),
                            is_executable: false,
                        });
                    }
                    touched.push(from);
                    touched.push(to);
                }
                Some("create") => {
                    let rel = uri_to_relative(
                        &root,
                        change.get("uri").and_then(|u| u.as_str()).unwrap_or(""),
                    )?;
                    let abs = root.join(&rel);
                    if let Some(parent) = abs.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    if !abs.exists() {
                        std::fs::write(&abs, b"")?;
                    }
                }
                Some("delete") => {
                    let rel = uri_to_relative(
                        &root,
                        change.get("uri").and_then(|u| u.as_str()).unwrap_or(""),
                    )?;
                    let abs = root.join(&rel);
                    if abs.is_file() {
                        std::fs::remove_file(&abs)?;
                    }
                    deltas.push(FileDelta {
                        relative_path: rel.clone(),
                        content: None,
                        is_executable: false,
                    });
                    touched.push(rel);
                }
                _ => {
                    let uri = change
                        .pointer("/textDocument/uri")
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    let edits = change
                        .get("edits")
                        .and_then(|e| e.as_array())
                        .cloned()
                        .unwrap_or_default();
                    text_edits.push((uri_to_relative(&root, uri)?, edits));
                }
            }
        }
    } else if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, edits) in changes {
            let edits = edits.as_array().cloned().unwrap_or_default();
            text_edits.push((uri_to_relative(&root, uri)?, edits));
        }
    }

    for (rel, edits) in text_edits {
        let abs = root.join(&rel);
        let current = std::fs::read_to_string(&abs).unwrap_or_default();
        let new_text = apply_text_edits(&current, &edits)?;
        deltas.push(FileDelta {
            relative_path: rel.clone(),
            content: Some(new_text.into_bytes()),
            is_executable: false,
        });
        touched.push(rel);
    }

    // The gateway has not seen these edits (it only computed them), so they must not be
    // recorded as synced: forget them everywhere and let the next sync upload them.
    apply_pulled_files(&root, &deltas)?;
    crate::sync::forget_synced_files(&root, &touched);
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_file_and_ranged_edits() {
        let text = "fn a() {}\nfn b() {}\n";
        let whole = serde_json::json!([{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 2, "character": 0 } }, "newText": "fn z() {}\n" }]);
        assert_eq!(
            apply_text_edits(text, whole.as_array().unwrap()).unwrap(),
            "fn z() {}\n"
        );
        let ranged = serde_json::json!([
            { "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 4 } }, "newText": "alpha" },
            { "range": { "start": { "line": 1, "character": 3 }, "end": { "line": 1, "character": 4 } }, "newText": "beta" }
        ]);
        assert_eq!(
            apply_text_edits(text, ranged.as_array().unwrap()).unwrap(),
            "fn alpha() {}\nfn beta() {}\n"
        );
    }

    #[test]
    fn apply_workspace_edit_writes_moves_and_records() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "mod old_name;\n").unwrap();
        std::fs::write(root.join("src/old_name.rs"), "pub fn f() {}\n").unwrap();
        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let edit = serde_json::json!({ "documentChanges": [
            { "kind": "rename", "oldUri": uri("src/old_name.rs"), "newUri": uri("src/new_name.rs") },
            { "textDocument": { "uri": uri("src/lib.rs"), "version": null },
              "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } }, "newText": "mod new_name;\n" } ] }
        ]});
        let touched = apply_workspace_edit(&root, &edit).unwrap();
        assert_eq!(
            touched,
            vec!["src/old_name.rs", "src/new_name.rs", "src/lib.rs"]
        );
        assert!(!root.join("src/old_name.rs").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("src/new_name.rs")).unwrap(),
            "pub fn f() {}\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "mod new_name;\n"
        );
        // Nothing the client rewrote counts as synced: every gateway still has the old text.
        let state = crate::sync::load_sync_cache(&root);
        assert!(!state.files.contains_key("src/lib.rs"));
        assert!(!state.files.contains_key("src/new_name.rs"));
        assert!(!state.files.contains_key("src/old_name.rs"));
        // Anything outside the checkout is refused.
        let outside = serde_json::json!({ "changes": { "file:///etc/hosts": [] } });
        assert!(apply_workspace_edit(&root, &outside).is_err());
        crate::sync::clear_sync_cache(&root);
    }
}
