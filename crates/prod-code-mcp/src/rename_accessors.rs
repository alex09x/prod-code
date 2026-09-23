//! Renaming a field together with its accessors: `timeout` becomes `deadline`, and so do
//! `timeout()`, `set_timeout()`, `get_timeout()` and `timeout_mut()` in the struct's own `impl`
//! blocks, with every call.
//!
//! Each rename is the analyzer's, computed against the checkout as it is; the edits are then
//! merged into one change per file, character by character against that common base. Renames of
//! different names touch different text, and where two of them would touch the same text the
//! merge refuses instead of guessing.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The accessor names a field `old` can have, each with the name it takes when the field becomes
/// `new`: `f`, `get_f`, `set_f`, `f_mut`.
pub fn accessor_names(old: &str, new: &str) -> Vec<(String, String)> {
    vec![
        (old.to_string(), new.to_string()),
        (format!("get_{old}"), format!("get_{new}")),
        (format!("set_{old}"), format!("set_{new}")),
        (format!("{old}_mut"), format!("{new}_mut")),
    ]
}

/// `text` as tokens: each identifier whole, every other character on its own. A rename replaces
/// whole identifiers, so a diff over these never splits one, and two renames of the same
/// identifier always meet on the same token.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let mut end = i + c.len_utf8();
        if is_ident(c) {
            while let Some(&(j, n)) = chars.peek() {
                if !is_ident(n) {
                    break;
                }
                end = j + n.len_utf8();
                chars.next();
            }
        }
        out.push(&text[i..end]);
    }
    out
}

/// The edits that turn `base` into `changed`, as (start, end, replacement) over `base`'s tokens.
fn token_edits<'a>(base: &[&'a str], changed: &[&'a str]) -> Vec<(usize, usize, Vec<&'a str>)> {
    similar::capture_diff_slices(similar::Algorithm::Myers, base, changed)
        .into_iter()
        .filter_map(|op| {
            let (tag, old, new) = op.as_tag_tuple();
            (tag != similar::DiffTag::Equal).then(|| (old.start, old.end, changed[new].to_vec()))
        })
        .collect()
}

/// `base` with the changes of both `a` and `b`, when they touch different text; `None` when an
/// edit of one overlaps an edit of the other. An edit both make identically counts once.
pub fn merge_three(base: &str, a: &str, b: &str) -> Option<String> {
    let base_tokens = tokens(base);
    let a_tokens = tokens(a);
    let b_tokens = tokens(b);
    let mut edits = token_edits(&base_tokens, &a_tokens);
    for edit in token_edits(&base_tokens, &b_tokens) {
        if !edits.contains(&edit) {
            edits.push(edit);
        }
    }
    edits.sort_by_key(|(start, end, _)| (*start, *end));
    for pair in edits.windows(2) {
        let (s1, e1, _) = &pair[0];
        let (s2, _, _) = &pair[1];
        // Overlapping ranges, or two insertions at one point: the order would be a guess.
        if e1 > s2 || (s1 == e1 && s1 == s2) {
            return None;
        }
    }
    let mut out = base_tokens;
    for (start, end, replacement) in edits.into_iter().rev() {
        out.splice(start..end, replacement);
    }
    Some(out.concat())
}

/// The struct whose braces enclose `at`, by name.
fn enclosing_struct(text: &str, at: usize) -> Option<String> {
    let mut depth = 0i32;
    let open = text[..at].char_indices().rev().find_map(|(i, c)| {
        match c {
            '}' => depth += 1,
            '{' if depth == 0 => return Some(i),
            '{' => depth -= 1,
            _ => {}
        }
        None
    })?;
    // The item's own header: from after the previous item (`}` or `;`) to the brace.
    let item_start = text[..open].rfind(['}', ';']).map_or(0, |i| i + 1);
    let head = text[item_start..open].trim_end();
    let head = head.split(" where").next().unwrap_or(head).trim_end();
    let before_generics = match head.strip_suffix('>') {
        Some(_) => &head[..head.find('<')?],
        None => head,
    }
    .trim_end();
    let name_start = before_generics
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map(|(i, _)| i)?;
    before_generics[..name_start]
        .trim_end()
        .ends_with("struct")
        .then(|| before_generics[name_start..].to_string())
}

/// Where the analyzer says the symbol at `line`:`col` of `file` is declared.
async fn definition(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<(PathBuf, u32, u32)> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    let answer = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/definition",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        }),
    )
    .await?;
    let location = match &answer {
        serde_json::Value::Array(items) => items.first().cloned(),
        other if other.is_object() => Some(other.clone()),
        _ => None,
    }
    .context("the analyzer does not know where the field is declared")?;
    let path = location
        .get("uri")
        .or_else(|| location.get("targetUri"))
        .and_then(|u| u.as_str())
        .map(|u| PathBuf::from(crate::remote_fs::uri_to_path(u)))
        .context("the definition has no file")?;
    let range = location
        .get("targetSelectionRange")
        .or_else(|| location.get("range"))
        .context("the definition has no position")?;
    let at = |key: &str| {
        range
            .pointer(&format!("/start/{key}"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32
            + 1
    };
    Ok((path, at("line"), at("character")))
}

/// Every file the renames change, merged into one text per file against the checkout, and the
/// renames that went into it (`timeout → deadline`, `set_timeout → set_deadline`).
pub async fn plan(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    new_name: &str,
) -> Result<(BTreeMap<PathBuf, String>, Vec<String>)> {
    let (def_path, def_line, def_col) = definition(remote, root, file, line, col).await?;
    let def_text = std::fs::read_to_string(&def_path)
        .with_context(|| format!("cannot read {}", def_path.display()))?;
    let at = crate::signature::offset_of(&def_text, def_line, def_col)
        .context("the definition is not in its file")?;
    let field: String = def_text[at..]
        .chars()
        .take_while(|c| is_ident(*c))
        .collect();
    let owner = enclosing_struct(&def_text, at).with_context(|| {
        format!("`{field}` is not a field of a struct; `accessors` applies to a field")
    })?;

    // The field's own rename, then each accessor's, all against the checkout as it is.
    let mut renames: Vec<(PathBuf, u32, u32, String, String)> = vec![(
        def_path.clone(),
        def_line,
        def_col,
        field.clone(),
        new_name.to_string(),
    )];
    for (old, new) in accessor_names(&field, new_name).into_iter().skip(1).chain(
        // A getter named after the field itself is a method, found like the others.
        std::iter::once((field.clone(), new_name.to_string())),
    ) {
        let hits = crate::tools::workspace_symbol_search(remote, root, &old, None, 50).await?;
        for hit in hits {
            let is_method = matches!(hit.kind, "Method" | "Function");
            let in_owner = hit
                .container
                .as_deref()
                .is_some_and(|c| c == owner || c.ends_with(&format!(" {owner}")));
            if hit.name == old && is_method && in_owner {
                renames.push((
                    hit.path.clone(),
                    hit.line,
                    hit.col,
                    old.clone(),
                    new.clone(),
                ));
            }
        }
    }

    let mut merged: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut base: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut done = Vec::new();
    for (index, (path, l, c, old, new)) in renames.into_iter().enumerate() {
        let uri = url::Url::from_file_path(&path)
            .map_err(|_| anyhow::anyhow!("invalid path {:?}", path))?
            .to_string();
        let edit = crate::tools::execute_lsp_query(
            remote,
            root,
            &path,
            "textDocument/rename",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": l - 1, "character": c - 1 },
                "newName": new,
            }),
        )
        .await
        .with_context(|| format!("the rename of `{old}` to `{new}` was refused"))?;
        if edit.is_null() {
            continue;
        }
        let (planned, _) = crate::refactor::planned_texts(root, &edit)?;
        for (p, text) in planned {
            let original = base
                .entry(p.clone())
                .or_insert_with(|| std::fs::read_to_string(&p).unwrap_or_default())
                .clone();
            let so_far = merged.get(&p).cloned().unwrap_or_else(|| original.clone());
            let combined = merge_three(&original, &so_far, &text).with_context(|| {
                format!(
                    "the rename of `{old}` to `{new}` touches the same text in {} as an earlier \
                     one; rename them one at a time",
                    p.display()
                )
            })?;
            merged.insert(p, combined);
        }
        // The first rename is the field's; the others are methods.
        done.push(if index == 0 {
            format!("the field `{old}` → `{new}`")
        } else {
            format!("`{old}()` → `{new}()`")
        });
    }
    Ok((merged, done))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_renames_of_different_names_merge_even_on_one_line() {
        let base = "c.set_timeout(c.timeout() * 2);\nlet t = x.timeout;\n";
        let field = "c.set_timeout(c.timeout() * 2);\nlet t = x.deadline;\n";
        let getter = "c.set_timeout(c.deadline() * 2);\nlet t = x.timeout;\n";
        let setter = "c.set_deadline(c.timeout() * 2);\nlet t = x.timeout;\n";
        let one = merge_three(base, field, getter).expect("different text");
        let all = merge_three(base, &one, setter).expect("different text");
        assert_eq!(
            all,
            "c.set_deadline(c.deadline() * 2);\nlet t = x.deadline;\n"
        );
        // The same edit made by both counts once.
        assert_eq!(merge_three(base, field, field).as_deref(), Some(field));
        // Two different edits of the same text are not merged, even when the new names share
        // letters with the old one (a character diff would interleave them).
        let other = "c.set_timeout(c.timeout() * 2);\nlet t = x.other;\n";
        assert!(merge_three(base, field, other).is_none());
        assert_eq!(tokens("a.b_c(1);"), ["a", ".", "b_c", "(", "1", ")", ";"]);
    }

    #[test]
    fn a_field_s_struct_and_its_accessor_names_are_found() {
        let t =
            "pub struct Conn<T> where T: Clone {\n    timeout: u64,\n}\nfn f() { let x = 1; }\n";
        assert_eq!(
            enclosing_struct(t, t.find("timeout").unwrap()).as_deref(),
            Some("Conn")
        );
        assert!(enclosing_struct(t, t.find("x =").unwrap()).is_none());
        let names: Vec<String> = accessor_names("timeout", "deadline")
            .into_iter()
            .map(|(o, n)| format!("{o}>{n}"))
            .collect();
        assert_eq!(
            names,
            [
                "timeout>deadline",
                "get_timeout>get_deadline",
                "set_timeout>set_deadline",
                "timeout_mut>deadline_mut"
            ]
        );
    }
}
