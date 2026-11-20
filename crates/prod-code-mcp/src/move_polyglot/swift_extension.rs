/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A member of a Swift type moves into an `extension` of the same type in another file (#2040).
//!
//! Moved alone it would be a free declaration outside its type (#1978), so the move writes it
//! into the target file's `extension` of that type — the first one without a `where` clause — or
//! appends a new one. The member keeps its text, so its signature and every caller stay as they
//! were. What Swift does not allow in an extension, or what the move would cut off from the rest
//! of the type, is refused with the reason: a stored property, an enum case, `deinit`, an
//! `override`, a designated or `required` initializer of a class, a `private` or `fileprivate`
//! member, a requirement of a protocol, a member of a constrained extension, and anything
//! declared inside a function or an accessor.

use crate::caller_migration::mask::lexical_code_mask;
use crate::parameter_object::Language;

const TYPE_KEYWORDS: [&str; 6] = ["class", "struct", "enum", "actor", "extension", "protocol"];
const MEMBER_KEYWORDS: [&str; 7] = ["func", "var", "let", "init", "subscript", "deinit", "case"];

/// One enclosing `{`: the keyword that opened it, the name it declares, and whether its header
/// carries a `where` clause. `keyword` is `None` for anything but a type or an extension.
struct Opener {
    keyword: Option<&'static str>,
    name: String,
    /// A `where` clause, or a bound generic argument (`extension Array<Int>`).
    constrained: bool,
    /// An access modifier, an attribute (`@MainActor`) or a compilation condition (`#if`) on
    /// the header: what its members get from it, a plain extension would not give them.
    decorated: bool,
    /// Declared `private` or `fileprivate`: another file cannot name it.
    file_private: bool,
}

const ACCESS_MODIFIERS: [&str; 6] = [
    "public",
    "private",
    "fileprivate",
    "internal",
    "package",
    "open",
];

fn code_mask(source: &str) -> Vec<bool> {
    lexical_code_mask(source, Language::Swift)
}

/// The code text of the header before the `{` at `brace`: back to the previous `{`, `}` or `;`
/// in code, with comments and strings blanked.
fn header(source: &str, mask: &[bool], brace: usize) -> String {
    let bytes = source.as_bytes();
    let mut start = brace;
    while start > 0 {
        let i = start - 1;
        if mask[i] && matches!(bytes[i], b'{' | b'}' | b';') {
            break;
        }
        start = i;
    }
    source[start..brace]
        .char_indices()
        .map(|(i, c)| if mask[start + i] { c } else { ' ' })
        .collect()
}

fn words(text: &str) -> Vec<&str> {
    text.split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .filter(|w| !w.is_empty())
        .collect()
}

/// A word that can stand before a declaration's keyword: an attribute or a modifier.
fn is_modifier(word: &str) -> bool {
    let base = word.split('(').next().unwrap_or(word);
    word.starts_with('@')
        || ACCESS_MODIFIERS.contains(&base)
        || matches!(
            base,
            "final"
                | "indirect"
                | "nonisolated"
                | "static"
                | "dynamic"
                | "override"
                | "mutating"
                | "nonmutating"
                | "convenience"
                | "required"
                | "lazy"
                | "weak"
                | "unowned"
                | "distributed"
        )
}

/// The lines of `header` that belong to the declaration its `{` opens: from the last line whose
/// first word after its modifiers is a declaration keyword, with the attribute lines above it.
/// A brace-free declaration before it (`let version = 1`, `case a`) is left out.
fn declaration_lines(header: &str) -> String {
    let lines: Vec<&str> = header
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let starts_declaration = |line: &str| {
        words(line)
            .into_iter()
            .find(|w| !is_modifier(w))
            .is_some_and(|w| TYPE_KEYWORDS.contains(&w) || MEMBER_KEYWORDS.contains(&w))
    };
    let Some(mut first) = lines.iter().rposition(|l| starts_declaration(l)) else {
        return lines.join(" ");
    };
    while first > 0 && lines[first - 1].starts_with('@') {
        first -= 1;
    }
    lines[first..].join(" ")
}

fn opener(header: &str) -> Opener {
    let declaration = declaration_lines(header);
    let tokens = words(&declaration);
    for (i, token) in tokens.iter().enumerate() {
        // A function, an initializer or an accessor: its parameter labels (`actor target:`,
        // `struct name:`) are no type keywords.
        if MEMBER_KEYWORDS.contains(token) {
            break;
        }
        let Some(&keyword) = TYPE_KEYWORDS.iter().find(|k| *k == token) else {
            continue;
        };
        // `class func`, `class var`: a type member, not a class.
        if tokens
            .get(i + 1)
            .is_some_and(|next| MEMBER_KEYWORDS.contains(next))
        {
            break;
        }
        let spelled = tokens.get(i + 1).copied().unwrap_or("");
        let name = spelled.split(['<', ':', '{']).next().unwrap_or("");
        let before = &tokens[..i];
        let decorated = before.iter().any(|t| {
            ACCESS_MODIFIERS.contains(&t.split('(').next().unwrap_or(t)) || t.starts_with('@')
        });
        return Opener {
            keyword: Some(keyword),
            name: name.to_string(),
            constrained: tokens.contains(&"where")
                || (keyword == "extension" && spelled.contains('<')),
            decorated,
            file_private: before
                .iter()
                .any(|t| matches!(*t, "private" | "fileprivate")),
        };
    }
    Opener {
        keyword: None,
        name: String::new(),
        constrained: false,
        decorated: false,
        file_private: false,
    }
}

/// How many `#if` blocks are open at byte `offset` of `source`: counted over the whole file,
/// since a header reaches back only to the previous brace.
fn open_conditions(source: &str, mask: &[bool], offset: usize) -> usize {
    let mut depth = 0usize;
    let mut at = 0;
    for line in source[..offset.min(source.len())].split_inclusive('\n') {
        let indent = line.len() - line.trim_start().len();
        let code = line.trim_start();
        if mask.get(at + indent).copied().unwrap_or(false) {
            if code.starts_with("#if") {
                depth += 1;
            } else if code.starts_with("#endif") {
                depth = depth.saturating_sub(1);
            }
        }
        at += line.len();
    }
    depth
}

/// The enclosing `{` positions of byte `offset`, outermost first.
fn enclosing(source: &str, mask: &[bool], offset: usize) -> Vec<usize> {
    let mut stack = Vec::new();
    for (i, byte) in source.as_bytes()[..offset.min(source.len())]
        .iter()
        .enumerate()
    {
        match (mask[i], *byte) {
            (true, b'{') => stack.push(i),
            (true, b'}') => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack
}

/// The modifiers and keyword of the member: its code text up to the first `{` or `=`, without
/// doc comments and attributes.
fn member_head(item: &str) -> (Vec<String>, String) {
    let mask = code_mask(item);
    let code: String = item
        .char_indices()
        .map(|(i, c)| if mask[i] { c } else { ' ' })
        .collect();
    let head_end = code.find(['{', '=']).unwrap_or(code.len());
    let tokens: Vec<String> = words(&code[..head_end])
        .into_iter()
        .filter(|w| !w.starts_with('@'))
        .map(str::to_string)
        .collect();
    (tokens, code)
}

/// Why `item` cannot live in an extension of a type declared with `keyword`, if it cannot.
fn refusal(item: &str, keyword: &str) -> Option<String> {
    let (tokens, code) = member_head(item);
    // `init?`, `init!` and `private(set)` name the same keyword.
    let has = |w: &str| {
        tokens
            .iter()
            .any(|t| t.trim_end_matches(['?', '!']) == w || t.starts_with(&format!("{w}(")))
    };
    if has("private") || has("fileprivate") {
        return Some(
            "it is `private` or `fileprivate`, so the rest of its type could no longer reach it from another file; make it `internal` first".into(),
        );
    }
    if has("override") {
        return Some(
            "it is an `override`, and Swift does not allow an override in an extension".into(),
        );
    }
    if has("deinit") {
        return Some("a deinitializer must stay in the type's own declaration".into());
    }
    if has("case") {
        return Some("an enum case must stay in the enum's own declaration".into());
    }
    if has("init") && (has("required") || (keyword == "class" && !has("convenience"))) {
        return Some(
            "a designated or `required` initializer of a class must stay in the class's own declaration".into(),
        );
    }
    if has("let") {
        return Some("a stored property must stay in the type's own declaration".into());
    }
    if has("var") {
        let head_end = code.find(['{', '=']).unwrap_or(code.len());
        let computed = code[head_end..].starts_with('{')
            && !code.contains("willSet")
            && !code.contains("didSet");
        if !computed {
            return Some("a stored property must stay in the type's own declaration".into());
        }
    }
    None
}

/// The type path (`Outer.Inner`) whose extension can hold the Swift member that starts on the
/// 1-based `line` of `source`, or why it cannot be moved into one.
pub(super) fn member_destination(source: &str, line: u32, item: &str) -> Result<String, String> {
    let offset: usize = source
        .split_inclusive('\n')
        .take(line.saturating_sub(1) as usize)
        .map(str::len)
        .sum();
    let mask = code_mask(source);
    let mut path = Vec::new();
    let mut innermost = "";
    let braces = enclosing(source, &mask, offset);
    // A `#if` opened inside the type around the member: moved out, the member would be
    // compiled unconditionally.
    if braces.first().is_some_and(|outer| {
        open_conditions(source, &mask, offset) > open_conditions(source, &mask, *outer)
    }) {
        return Err(
            "it is inside a `#if` within its type, whose condition an extension in another file would drop".into(),
        );
    }
    for brace in braces {
        let mut opener = opener(&header(source, &mask, brace));
        if opener.keyword == Some("extension") && open_conditions(source, &mask, brace) > 0 {
            opener.decorated = true;
        }
        match opener.keyword {
            None => {
                return Err(
                    "it is declared inside a function, a closure or an accessor, not directly in a type".into(),
                );
            }
            Some("protocol") => {
                return Err(
                    "it is a requirement of a protocol, and in an extension it would become a default implementation".into(),
                );
            }
            Some("extension") if opener.constrained => {
                return Err(
                    "it is in an extension with a `where` clause or a bound generic argument, whose constraints an extension of the plain type would drop".into(),
                );
            }
            Some("extension") if opener.decorated => {
                return Err(
                    "it is in an extension with an access modifier, an attribute or a `#if`, which a plain extension would not give it; spell them on the member first".into(),
                );
            }
            Some(keyword) if keyword != "extension" && opener.file_private => {
                return Err(format!(
                    "its type `{}` is `private` or `fileprivate`, so an extension in another file could not name it",
                    opener.name
                ));
            }
            Some(keyword) => {
                if opener.name.is_empty() {
                    return Err("the name of its enclosing type could not be read".into());
                }
                path.push(opener.name);
                innermost = keyword;
            }
        }
    }
    if path.is_empty() {
        return Err("it is not inside a type".into());
    }
    if let Some(reason) = refusal(item, innermost) {
        return Err(reason);
    }
    // The analyzer accepts a call from another file to a `private` member; the compiler does not.
    let used: Vec<String> = file_private_names(source)
        .into_iter()
        .filter(|name| mentions(item, name))
        .collect();
    if !used.is_empty() {
        return Err(format!(
            "it uses {}, which {} `private` or `fileprivate` in this file and not visible from another one; make {} `internal` first, or move {} too",
            used.iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", "),
            if used.len() == 1 { "is" } else { "are" },
            if used.len() == 1 { "it" } else { "them" },
            if used.len() == 1 { "it" } else { "them" },
        ));
    }
    Ok(path.join("."))
}

/// The names this file declares `private` or `fileprivate`.
fn file_private_names(source: &str) -> Vec<String> {
    let mask = code_mask(source);
    let code: String = source
        .char_indices()
        .map(|(i, c)| if mask[i] { c } else { ' ' })
        .collect();
    let mut names = Vec::new();
    for statement in code.split(['\n', ';', '{', '}']) {
        let tokens = words(statement);
        let private = tokens
            .iter()
            .any(|t| *t == "private" || *t == "fileprivate");
        if !private {
            continue;
        }
        let Some(i) = tokens.iter().position(|t| {
            matches!(
                *t,
                "func" | "var" | "let" | "class" | "struct" | "enum" | "actor" | "typealias"
            )
        }) else {
            continue;
        };
        if let Some(name) = tokens.get(i + 1) {
            let name: String = name
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// Whether `name` appears in the code of `text` as a whole identifier, outside the member's own
/// declaration of it.
fn mentions(text: &str, name: &str) -> bool {
    let mask = code_mask(text);
    let bytes = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    text.match_indices(name).any(|(i, _)| {
        let end = i + name.len();
        mask[i]
            && (i == 0 || !ident(bytes[i - 1]))
            && (end >= bytes.len() || !ident(bytes[end]))
            && !text[..i].trim_end().ends_with("func")
    })
}

/// `item` re-indented by four spaces under its least indented line.
fn indented(item: &str) -> String {
    let indent = item
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for line in item.trim_matches('\n').lines() {
        if line.trim().is_empty() {
            out.push('\n');
        } else {
            out.push_str("    ");
            out.push_str(&line[indent.min(line.len() - line.trim_start().len())..]);
            out.push('\n');
        }
    }
    out
}

/// `target` with `item` added to its first top-level `extension <path>` without a `where`
/// clause, or with a new `extension <path>` holding it appended.
pub(super) fn insert_member(target: &str, path: &str, item: &str) -> String {
    let body = indented(item);
    let mask = code_mask(target);
    let bytes = target.as_bytes();
    let mut depth = 0usize;
    let mut open: Option<usize> = None;
    for (i, byte) in bytes.iter().enumerate() {
        if !mask[i] {
            continue;
        }
        match *byte {
            b'{' => {
                if depth == 0 {
                    let opener = opener(&header(target, &mask, i));
                    if opener.keyword == Some("extension")
                        && opener.name == path
                        && !opener.constrained
                        && !opener.decorated
                        && open_conditions(target, &mask, i) == 0
                    {
                        open = Some(i);
                    }
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && open.is_some() {
                    // Insert before the closing brace's line.
                    let line_start = target[..i].rfind('\n').map_or(0, |n| n + 1);
                    let (before, after) = if target[line_start..i].trim().is_empty() {
                        target.split_at(line_start)
                    } else {
                        target.split_at(i)
                    };
                    let mut out = before.to_string();
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    if !out.ends_with("{\n") && !out.ends_with("\n\n") {
                        out.push('\n');
                    }
                    out.push_str(&body);
                    out.push_str(after);
                    return out;
                }
            }
            _ => {}
        }
    }
    let extension = format!("extension {path} {{\n{body}}}\n");
    if target.trim().is_empty() {
        return extension;
    }
    crate::move_item::append_item(target, &extension)
}

#[cfg(test)]
#[path = "swift_extension_tests.rs"]
mod tests;
