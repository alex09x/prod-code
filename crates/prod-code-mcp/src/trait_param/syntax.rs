/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use anyhow::Result;

use super::types::Owner;

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `text` uses `name` as a word of its own.
pub fn mentions(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(i, _)| {
        !text[..i].chars().next_back().is_some_and(is_ident)
            && !text[i + name.len()..].chars().next().is_some_and(is_ident)
    })
}

/// The trait or trait implementation whose block holds `at`, if the innermost block around it
/// is one.
pub fn owner_of(text: &str, at: usize) -> Option<Owner> {
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
    let header_start = text[..open].rfind([';', '}', '{']).map_or(0, |i| i + 1);
    let header = &text[header_start..open];
    // Attributes and doc comments above the item are not its header.
    let item_at = header
        .match_indices("trait ")
        .chain(header.match_indices("impl"))
        .map(|(i, _)| i)
        .filter(|i| !header[..*i].chars().next_back().is_some_and(is_ident))
        .min()?;
    let item = &header[item_at..];
    if let Some(rest) = item.strip_prefix("trait ") {
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|c| is_ident(*c))
            .collect();
        return (!name.is_empty()).then_some(Owner::Trait { name });
    }
    // `impl<T> path::Trait<X> for Type`: the trait is the last segment before ` for `.
    let rest = item.strip_prefix("impl")?;
    let rest_at = header_start + item_at + 4;
    // The implementation's own generics (`impl<T: Copy>`) come first and are skipped.
    let lead = rest.len() - rest.trim_start().len();
    let mut generics_end = 0;
    if rest[lead..].starts_with('<') {
        let mut angle = 0i32;
        for (i, c) in rest.char_indices().skip(lead) {
            match c {
                '<' => angle += 1,
                '>' => {
                    angle -= 1;
                    if angle == 0 {
                        generics_end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let mut angle = 0i32;
    let mut trait_end = None;
    for (i, c) in rest.char_indices().skip_while(|(i, _)| *i < generics_end) {
        match c {
            '<' => angle += 1,
            '>' => angle -= 1,
            _ if angle == 0 && rest[i..].starts_with(" for ") => {
                trait_end = Some(i);
                break;
            }
            _ => {}
        }
    }
    let trait_path = &rest[generics_end..trait_end?];
    // The last path segment, before any generic arguments of the trait.
    let bare_end = trait_path.find('<').unwrap_or(trait_path.len());
    let bare = &trait_path[..bare_end];
    let name_start = bare
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map(|(i, _)| i)?;
    Some(Owner::Impl {
        trait_at: rest_at + generics_end + name_start,
    })
}

/// The byte spans of the items of a comma-separated list (`a, b(c, d), e`), trimmed, relative
/// to the list.
pub fn item_spans(list: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    // `<` opens generic arguments only right after a name or `::` (`Vec<u8>`, `f::<T>`); with a
    // space before it, it is a comparison.
    let mut generics = 0i32;
    let mut start = 0;
    let chars: Vec<(usize, char)> = list.char_indices().collect();
    let push = |out: &mut Vec<(usize, usize)>, from: usize, to: usize| {
        let item = &list[from..to];
        let lead = item.len() - item.trim_start().len();
        let trail = item.len() - item.trim_end().len();
        if from + lead < to - trail {
            out.push((from + lead, to - trail));
        }
    };
    for (k, (i, c)) in chars.iter().copied().enumerate() {
        let prev = if k > 0 { chars[k - 1].1 } else { ' ' };
        let next = chars.get(k + 1).map_or(' ', |(_, c)| *c);
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '<' if (is_ident(prev) || prev == ':') && next != '<' && next != '=' => generics += 1,
            '>' if generics > 0 && prev != '-' && prev != '=' => generics -= 1,
            ',' if depth == 0 && generics == 0 => {
                push(&mut out, start, i);
                start = i + 1;
            }
            _ => {}
        }
    }
    push(&mut out, start, list.len());
    out
}

/// The range of `list` to cut to remove item `index`, with the comma that separates it: from
/// its start to the next item, or, for the last, from the end of the one before.
pub fn removal(spans: &[(usize, usize)], index: usize) -> Option<(usize, usize)> {
    let (from, to) = *spans.get(index)?;
    Some(if let Some((next, _)) = spans.get(index + 1) {
        (from, *next)
    } else if index > 0 {
        (spans[index - 1].1, to)
    } else {
        (from, to)
    })
}

/// Why dropping `arg` would change what the program does, if it would.
pub fn effect_of(arg: &str) -> Option<&'static str> {
    if arg.contains(".await") {
        Some("awaits")
    } else if arg.contains('?') {
        Some("can return early with `?`")
    } else if ["!(", "![", "!{"].iter().any(|m| arg.contains(m)) {
        Some("expands a macro")
    } else if arg.contains('(') {
        Some("calls something")
    } else {
        None
    }
}

pub(crate) fn position(path: &Path, line: u32, col: u32) -> Result<serde_json::Value> {
    let uri = url::Url::from_file_path(path)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", path))?
        .to_string();
    Ok(serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
    }))
}

/// The offset of `fn method` inside the block that opens at or after `from` in `text`.
pub(crate) fn method_in_block(text: &str, from: usize, method: &str) -> Option<usize> {
    let open = from + text[from..].find('{')?;
    let close = crate::parameter_object::matching_bracket(text, open)?;
    let needle = format!("fn {method}");
    text[open..close]
        .match_indices(&needle)
        .map(|(i, _)| open + i)
        .find(|i| {
            !text[..*i].chars().next_back().is_some_and(is_ident)
                && !text[i + needle.len()..]
                    .chars()
                    .next()
                    .is_some_and(is_ident)
        })
        .map(|i| i + 3)
}
