/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::item::offset_of;

/// A `use` statement's full text, from `use` to the `;`, and where it ends.
pub(crate) fn use_statements(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    for (offset, _) in text.match_indices("use ") {
        if offset < at {
            continue;
        }
        // Only a statement in column zero, so `pub use` counts while an indented `use` — one
        // inside a function body or a test module — is somebody else's import, not the file's.
        let line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let before = &text[line_start..offset];
        if !before.is_empty()
            && before != "pub "
            && !(before.starts_with("pub(") && before.ends_with(") "))
        {
            continue;
        }
        let Some(semi) = text[offset..].find(';') else {
            continue;
        };
        out.push((line_start, offset + semi + 1));
        at = offset + semi + 1;
    }
    out
}

/// Takes `name` out of every `use` statement in `text` that brings it in.
///
/// Returns the new text and what it did, so the report can say it. A grouped import keeps its
/// other names; a statement that imported nothing else goes away with its line.
pub fn drop_import(text: &str, name: &str) -> (String, Vec<String>) {
    let mut out = text.to_string();
    let mut notes = Vec::new();
    for _ in 0..64 {
        let Some((start, end)) = use_statements(&out).into_iter().find(|(s, e)| {
            let stmt = &out[*s..*e];
            leaf_names(stmt).iter().any(|leaf| leaf == name)
        }) else {
            break;
        };
        let stmt = out[start..end].to_string();
        let leaves = leaf_names(&stmt);
        let replacement = if leaves.len() <= 1 {
            notes.push(format!("dropped `{}`", stmt.trim()));
            String::new()
        } else {
            let shrunk = remove_from_group(&stmt, name);
            notes.push(format!("narrowed `{}` to `{}`", stmt.trim(), shrunk.trim()));
            shrunk
        };
        let mut tail_end = end;
        if replacement.is_empty() {
            // Take the newline with the line, or an empty line is left behind.
            if out[tail_end..].starts_with("\r\n") {
                tail_end += 2;
            } else if out[tail_end..].starts_with('\n') {
                tail_end += 1;
            }
        }
        out.replace_range(start..tail_end, &replacement);
    }
    (out, notes)
}

fn is_valid_rust_ident(s: &str) -> bool {
    let s = s.strip_prefix("r#").unwrap_or(s);
    !s.is_empty()
        && s != "_"
        && s != "self"
        && s != "super"
        && s != "crate"
        && s.chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (idx, ch) in s.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&s[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn collect_use_group_entries(prefix: &str, group: &str, out: &mut Vec<(String, String)>) {
    for raw in split_top_level_commas(group) {
        let seg = raw.trim();
        if seg.is_empty() {
            continue;
        }
        if let Some(open) = seg.find('{')
            && let Some(close) = seg.rfind('}')
            && close > open
        {
            let sub = seg[..open].trim().trim_end_matches("::").trim();
            let combined = match (prefix.is_empty(), sub.is_empty()) {
                (true, _) => sub.to_string(),
                (false, true) => prefix.to_string(),
                (false, false) => format!("{prefix}::{sub}"),
            };
            collect_use_group_entries(&combined, &seg[open + 1..close], out);
            continue;
        }
        if seg == "self" {
            let leaf = prefix.rsplit("::").next().unwrap_or(prefix).trim();
            if is_valid_rust_ident(leaf) && !prefix.is_empty() {
                out.push((prefix.to_string(), leaf.to_string()));
            }
            continue;
        }
        let rel_path = if prefix.is_empty() {
            seg.to_string()
        } else {
            format!("{prefix}::{seg}")
        };
        let leaf = if let Some((_, alias)) = seg.split_once(" as ") {
            alias.trim()
        } else {
            seg.rsplit("::").next().unwrap_or(seg).trim()
        };
        if is_valid_rust_ident(leaf) {
            out.push((rel_path, leaf.to_string()));
        }
    }
}

/// Returns `(rel_path_from_first_brace_or_full, leaf_ident)` for every named item imported by `stmt`.
/// Glob imports (`*`) and stray punctuation are excluded (#1117).
pub(crate) fn use_tree_entries(stmt: &str) -> Vec<(String, String)> {
    let Some(at) = stmt.find("use ") else {
        return Vec::new();
    };
    let clean: String = stmt[at + 4..]
        .lines()
        .map(|line| line.split("//").next().unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    let body = clean.trim().trim_end_matches(';').trim();
    let mut out = Vec::new();
    if let Some(open) = body.find('{')
        && let Some(close) = body.rfind('}')
        && close > open
    {
        collect_use_group_entries("", &body[open + 1..close], &mut out);
    } else {
        let leaf = if let Some((_, alias)) = body.split_once(" as ") {
            alias.trim()
        } else {
            body.rsplit("::").next().unwrap_or(body).trim()
        };
        if is_valid_rust_ident(leaf) {
            out.push((body.to_string(), leaf.to_string()));
        }
    }
    out
}

/// The names a `use` statement actually brings into scope.
pub(crate) fn leaf_names(stmt: &str) -> Vec<String> {
    use_tree_entries(stmt)
        .into_iter()
        .map(|(_, leaf)| leaf)
        .collect()
}

/// The same statement without `name` in its group.
pub(crate) fn remove_from_group(stmt: &str, name: &str) -> String {
    let Some((head, rest)) = stmt.split_once('{') else {
        return stmt.to_string();
    };
    let (_, tail) = rest.rsplit_once('}').unwrap_or((rest, ""));
    let entries = use_tree_entries(stmt);
    let kept: Vec<String> = entries
        .into_iter()
        .filter(|(_, leaf)| leaf != name)
        .map(|(rel, _)| rel)
        .collect();
    format!("{head}{{{}}}{tail}", kept.join(", "))
}

/// `text` with `use_line` added, after the imports it already has.
pub fn add_import(text: &str, use_line: &str) -> String {
    if text.contains(use_line) {
        return text.to_string();
    }
    let after = use_statements(text)
        .last()
        .map(|(_, end)| *end)
        .or_else(|| Some(super::preamble::preamble_end(text)))
        .unwrap_or(0);
    let mut out = text.to_string();
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let insert = if out[after..].starts_with(['\n', '\r']) {
        format!("{ending}{use_line}")
    } else {
        format!("{use_line}{ending}")
    };
    out.insert_str(after, &insert);
    out
}

/// Whether `text` uses `name` as a word of its own, rather than inside a longer identifier.
pub(crate) fn mentions(text: &str, name: &str) -> bool {
    if !is_valid_rust_ident(name) {
        return false;
    }
    let mut at = 0;
    while let Some(i) = text[at..].find(name) {
        let start = at + i;
        let end = start + name.len();
        let before_ok = start == 0
            || !text[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after_ok = text[end..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if before_ok && after_ok {
            return true;
        }
        at = end;
    }
    false
}

/// The same statement bringing in only `names`, preserving any nested submodule prefixes (#1117).
pub(crate) fn narrowed(stmt: &str, names: &[String]) -> String {
    let Some((head, _)) = stmt.split_once('{') else {
        return stmt.trim().to_string();
    };
    let entries = use_tree_entries(stmt);
    let rel_paths: Vec<String> = names
        .iter()
        .map(|name| {
            entries
                .iter()
                .find(|(_, leaf)| leaf == name)
                .map(|(rel, _)| rel.clone())
                .unwrap_or_else(|| name.clone())
        })
        .collect();
    if rel_paths.len() == 1 {
        return format!("{}{};", head.trim_start(), rel_paths[0]);
    }
    format!("{}{{{}}};", head.trim_start(), rel_paths.join(", "))
}

/// Returns `true` when `text` has a top-level `use super::*;` statement.
pub(crate) fn has_use_super_glob(text: &str) -> bool {
    use_statements(text).into_iter().any(|(s, e)| {
        let stmt = text[s..e].trim();
        stmt == "use super::*;" || stmt.ends_with(" use super::*;")
    })
}

/// Whether `text` declares an item called `name` itself: an import of it there would name it
/// twice (E0255), as moving a function next to the helper it calls did (#1502).
pub(crate) fn defines(text: &str, name: &str) -> bool {
    const KINDS: &[&str] = &[
        "fn",
        "struct",
        "enum",
        "trait",
        "type",
        "const",
        "static",
        "mod",
        "union",
        "macro_rules!",
    ];
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    text.lines().any(|line| {
        let mut previous: Option<&str> = None;
        for word in line.split_whitespace() {
            if previous.is_some_and(|p| KINDS.contains(&p)) {
                let declared: String = word.chars().take_while(|c| is_ident(*c)).collect();
                if declared == name {
                    return true;
                }
            }
            previous = Some(word);
        }
        false
    })
}

/// The imports the moved item takes with it.
///
/// An item does not carry its old module's whole header, only the statements that bring in a
/// name the item actually spells — narrowed to those names, so the target gains no import it
/// does not use. What the item needed from a *private* sibling of its old module cannot be
/// carried at all; that is what the type check is for.
pub fn carry_imports(source_text: &str, item: &str, target_text: &str) -> (String, Vec<String>) {
    let mut out = target_text.to_string();
    let mut notes = Vec::new();
    for (start, end) in use_statements(source_text) {
        let stmt = &source_text[start..end];
        let held: Vec<String> = use_statements(&out)
            .into_iter()
            .flat_map(|(s, e)| leaf_names(&out[s..e]))
            .collect();
        let needed: Vec<String> = leaf_names(stmt)
            .into_iter()
            .filter(|leaf| {
                mentions(item, leaf) && !held.contains(leaf) && !defines(target_text, leaf)
            })
            .collect();
        if needed.is_empty() {
            continue;
        }
        let line = narrowed(stmt, &needed);
        out = add_import(&out, &line);
        notes.push(format!("carried `{line}`"));
    }
    (out, notes)
}

/// Rewrites `old::path::Name` into `new::path::Name` at the positions the analyzer reported.
///
/// A reference that is spelled bare (`Name`, brought in by a `use`) has nothing to requalify
/// and is left for [`drop_import`] and [`add_import`]; one that is qualified is rewritten in
/// place, from the last position backwards so the earlier offsets stay true.
pub fn requalify(
    text: &str,
    positions: &[(u32, u32)],
    name: &str,
    new_prefix: &str,
) -> (String, usize) {
    let mut out = text.to_string();
    let mut bare = 0;
    let mut sorted: Vec<(u32, u32)> = positions.to_vec();
    sorted.sort_unstable();
    for (line, col) in sorted.into_iter().rev() {
        let Some(offset) = offset_of(&out, line, col) else {
            continue;
        };
        if !out[offset..].starts_with(name) {
            continue;
        }
        let path_start = out[..offset]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == ':')
            .last()
            .map(|(i, _)| i)
            .unwrap_or(offset);
        if path_start == offset {
            bare += 1; // a bare `Name`; it is the import that has to carry it
            continue;
        }
        out.replace_range(path_start..offset, &format!("{new_prefix}::"));
    }
    (out, bare)
}
