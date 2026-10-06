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
            if out[tail_end..].starts_with('\n') {
                tail_end += 1;
            }
        }
        out.replace_range(start..tail_end, &replacement);
    }
    (out, notes)
}

/// The names a `use` statement actually brings into scope.
pub(crate) fn leaf_names(stmt: &str) -> Vec<String> {
    let Some(at) = stmt.find("use ") else {
        return Vec::new();
    };
    let body = stmt[at + 4..].trim().trim_end_matches(';').trim();
    match body.split_once('{') {
        None => body
            .rsplit("::")
            .next()
            .map(|leaf| vec![leaf.trim().to_string()])
            .unwrap_or_default(),
        Some((_, group)) => group
            .trim_end_matches('}')
            .split(',')
            .map(|item| {
                item.split(" as ")
                    .next()
                    .unwrap_or(item)
                    .rsplit("::")
                    .next()
                    .unwrap_or(item)
                    .trim()
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .collect(),
    }
}

/// The same statement without `name` in its group.
pub(crate) fn remove_from_group(stmt: &str, name: &str) -> String {
    let Some((head, rest)) = stmt.split_once('{') else {
        return stmt.to_string();
    };
    let (group, tail) = rest.rsplit_once('}').unwrap_or((rest, ""));
    let kept: Vec<&str> = group
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty() && item.split(" as ").next().unwrap_or(item) != name)
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
        .or_else(|| {
            // No imports yet: go under the file's own header, not above it.
            let mut at = 0;
            for line in text.lines() {
                let t = line.trim();
                if t.starts_with("//!") || t.starts_with("#![") || t.is_empty() {
                    at += line.len() + 1;
                } else {
                    break;
                }
            }
            Some(at.min(text.len()))
        })
        .unwrap_or(0);
    let mut out = text.to_string();
    let insert = if out[after..].starts_with('\n') {
        format!("\n{use_line}")
    } else {
        format!("{use_line}\n")
    };
    out.insert_str(after, &insert);
    out
}

/// Whether `text` uses `name` as a word of its own, rather than inside a longer identifier.
pub(crate) fn mentions(text: &str, name: &str) -> bool {
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

/// The same statement bringing in only `names`.
pub(crate) fn narrowed(stmt: &str, names: &[String]) -> String {
    let Some((head, _)) = stmt.split_once('{') else {
        return stmt.trim().to_string();
    };
    if names.len() == 1 {
        return format!("{}{};", head.trim_start(), names[0]);
    }
    format!("{}{{{}}};", head.trim_start(), names.join(", "))
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
            .filter(|leaf| mentions(item, leaf) && !held.contains(leaf))
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
