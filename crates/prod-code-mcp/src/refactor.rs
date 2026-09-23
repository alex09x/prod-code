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
pub(crate) fn apply_text_edits(text: &str, edits: &[serde_json::Value]) -> Result<String> {
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

/// What every file an edit rewrites would contain, without writing anything: the text edits of
/// a `WorkspaceEdit` applied in memory to the files as they are. File renames, creations and
/// deletions are not modelled; the second value says whether the edit had any, so a caller can
/// say that part was not checked.
pub(crate) fn planned_texts(
    root: &Path,
    edit: &serde_json::Value,
) -> Result<(Vec<(std::path::PathBuf, String)>, bool)> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut per_file: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    let mut moves_files = false;
    if let Some(changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in changes {
            if change.get("kind").and_then(|k| k.as_str()).is_some() {
                moves_files = true;
                continue;
            }
            let uri = change
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let edits = change
                .get("edits")
                .and_then(|e| e.as_array())
                .cloned()
                .unwrap_or_default();
            per_file.push((uri_to_relative(&root, uri)?, edits));
        }
    } else if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, edits) in changes {
            per_file.push((
                uri_to_relative(&root, uri)?,
                edits.as_array().cloned().unwrap_or_default(),
            ));
        }
    }
    let mut out = Vec::with_capacity(per_file.len());
    for (rel, edits) in per_file {
        let abs = root.join(&rel);
        let current = std::fs::read_to_string(&abs).unwrap_or_default();
        out.push((abs, apply_text_edits(&current, &edits)?));
    }
    Ok((out, moves_files))
}

/// Applies a `WorkspaceEdit` (`documentChanges` or `changes`) to the checkout at `root`.
/// Returns the relative paths written, moved or deleted, in application order.
pub fn apply_workspace_edit(root: &Path, edit: &serde_json::Value) -> Result<Vec<String>> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    // Every path the edit can touch, and what is there now, before the first byte moves. A
    // multi-file refactor that stops halfway is worse than one that never started: the checkout
    // is inconsistent and nothing says which half landed.
    let paths = paths_touched_by(&root, edit)?;
    let before: Vec<(String, Option<Vec<u8>>)> = paths
        .iter()
        .map(|rel| {
            let abs = root.join(rel);
            let bytes = if abs.is_file() {
                std::fs::read(&abs).ok()
            } else {
                None
            };
            (rel.clone(), bytes)
        })
        .collect();
    match apply_unguarded(&root, edit) {
        Ok(touched) => {
            remember_applied(&root, &before);
            Ok(touched)
        }
        Err(err) => {
            let restored = restore(&root, &before);
            crate::sync::forget_synced_files(&root, &paths);
            Err(err.context(format!(
                "the edit failed partway and was undone: {restored} file(s) put back as they were"
            )))
        }
    }
}

/// What each file held before the last edit applied to it, and what the edit left there.
type Applied = std::collections::HashMap<std::path::PathBuf, (String, Vec<u8>)>;

fn applied() -> &'static std::sync::Mutex<Applied> {
    static APPLIED: std::sync::OnceLock<std::sync::Mutex<Applied>> = std::sync::OnceLock::new();
    APPLIED.get_or_init(Default::default)
}

/// Records, for every file an edit rewrote, the text it had before and the bytes it has now, so a
/// report rendered after the write can still show what changed (#122).
fn remember_applied(root: &Path, before: &[(String, Option<Vec<u8>>)]) {
    let Ok(mut map) = applied().lock() else {
        return;
    };
    for (rel, old) in before {
        let abs = root.join(rel);
        let Ok(now) = std::fs::read(&abs) else {
            continue;
        };
        let old = old
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        map.insert(abs, (old, now));
    }
}

/// The text a file had before the edit that produced what is on disk now: what a report shows as
/// the old side of its diff. When no edit wrote the file, or the file has changed since, that is
/// simply what is on disk.
pub fn text_before_apply(path: &Path) -> String {
    let current = std::fs::read(path).unwrap_or_default();
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Ok(map) = applied().lock()
        && let Some((old, written)) = map.get(&canonical).or_else(|| map.get(path))
        && *written == current
    {
        return old.clone();
    }
    String::from_utf8_lossy(&current).into_owned()
}

/// Every checkout-relative path an edit renames, creates, deletes or rewrites, in the order the
/// edit names them. Refuses a path outside the checkout, the same as applying would.
fn paths_touched_by(root: &Path, edit: &serde_json::Value) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |rel: String| {
        if !out.contains(&rel) {
            out.push(rel);
        }
    };
    if let Some(changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in changes {
            let uri_at = |key: &str| change.get(key).and_then(|u| u.as_str()).unwrap_or("");
            match change.get("kind").and_then(|k| k.as_str()) {
                Some("rename") => {
                    push(uri_to_relative(root, uri_at("oldUri"))?);
                    push(uri_to_relative(root, uri_at("newUri"))?);
                }
                Some("create") | Some("delete") => push(uri_to_relative(root, uri_at("uri"))?),
                _ => {
                    let uri = change
                        .pointer("/textDocument/uri")
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    push(uri_to_relative(root, uri)?);
                }
            }
        }
    } else if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for uri in changes.keys() {
            push(uri_to_relative(root, uri)?);
        }
    }
    Ok(out)
}

/// Puts every snapshotted path back: the bytes it had, or no file at all where there was none.
/// A directory is never removed — only files this edit could have created.
fn restore(root: &Path, before: &[(String, Option<Vec<u8>>)]) -> usize {
    let mut restored = 0;
    for (rel, bytes) in before {
        let abs = root.join(rel);
        match bytes {
            Some(bytes) => {
                if std::fs::read(&abs).ok().as_deref() == Some(bytes.as_slice()) {
                    continue;
                }
                if let Some(parent) = abs.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::write(&abs, bytes).is_ok() {
                    restored += 1;
                }
            }
            None => {
                if abs.is_file() && std::fs::remove_file(&abs).is_ok() {
                    restored += 1;
                }
            }
        }
    }
    restored
}

fn apply_unguarded(root: &Path, edit: &serde_json::Value) -> Result<Vec<String>> {
    let root = root.to_path_buf();
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

    /// A report rendered after an edit was written still has the old text to diff against (#122),
    /// and a file changed again since, by anything, is read as it is.
    #[test]
    fn the_text_before_an_applied_edit_is_kept_until_the_file_changes_again() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        let lib = root.join("src/lib.rs");
        std::fs::write(&lib, "pub fn old() {}\n").unwrap();
        assert_eq!(text_before_apply(&lib), "pub fn old() {}\n");

        let edit = serde_json::json!({ "changes": { format!("file://{}", lib.display()): [
            { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
              "newText": "pub fn new() {}\n" }
        ] } });
        apply_workspace_edit(&root, &edit).unwrap();
        assert_eq!(std::fs::read_to_string(&lib).unwrap(), "pub fn new() {}\n");
        assert_eq!(text_before_apply(&lib), "pub fn old() {}\n");

        std::fs::write(&lib, "pub fn later() {}\n").unwrap();
        assert_eq!(text_before_apply(&lib), "pub fn later() {}\n");
    }

    /// A multi-file edit either lands whole or not at all. The second write here cannot happen —
    /// its parent directory is a regular file — and the first one, which already succeeded, has
    /// to be put back, or the checkout is left half-refactored with nothing to say so.
    #[test]
    fn a_write_that_fails_halfway_leaves_every_file_as_it_was() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        crate::sync::clear_sync_cache(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn old() {}\n").unwrap();
        std::fs::write(root.join("src/other.rs"), "pub fn keep() {}\n").unwrap();
        // A regular file where a directory would have to be.
        std::fs::write(root.join("blocker"), "not a directory\n").unwrap();

        let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
        let whole = |text: &str| {
            serde_json::json!([{ "range": { "start": { "line": 0, "character": 0 },
                                             "end": { "line": 1, "character": 0 } },
                                  "newText": text }])
        };
        let edit = serde_json::json!({ "documentChanges": [
            { "textDocument": { "uri": uri("src/lib.rs"), "version": null }, "edits": whole("pub fn new() {}\n") },
            { "kind": "rename", "oldUri": uri("src/other.rs"), "newUri": uri("src/moved.rs") },
            { "textDocument": { "uri": uri("blocker/inner.rs"), "version": null }, "edits": whole("x\n") }
        ]});

        let err = apply_workspace_edit(&root, &edit).expect_err("the third write cannot happen");
        assert!(
            format!("{err:#}").contains("put back"),
            "the error says the checkout was restored: {err:#}"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "pub fn old() {}\n",
            "the edit that succeeded before the failure is undone"
        );
        assert!(
            root.join("src/other.rs").is_file() && !root.join("src/moved.rs").exists(),
            "the rename is undone too"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("blocker")).unwrap(),
            "not a directory\n",
            "and nothing that was in the way is disturbed"
        );
        crate::sync::clear_sync_cache(&root);
    }
}
