/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::{is_ident_byte, walk_code};
use super::types::{Kind, Language, Param};

/// The top-level entries of a comma-separated list, each as the offset of its first character
/// and its text from there to its last character of code — so a comment before or after an
/// entry is not part of it. Type arguments nest in TypeScript (`Map<string, number>`), C++
/// (`std::map<int, int>`) and Swift (`Dictionary<String, Int>`); a `<` is taken as one only
/// straight after a name, since a comparison is written with spaces.
pub fn entries(list: &str, language: Language) -> Vec<(usize, &str)> {
    let bytes = list.as_bytes();
    let (mut depth, mut angle) = (0i32, 0i32);
    let mut out = Vec::new();
    let mut current: Option<(usize, usize)> = None;
    let generic = matches!(
        language,
        Language::TypeScript | Language::Cpp | Language::Swift | Language::Java
    );
    walk_code(list, 0, language, |i, c| {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'<' if generic && i > 0 && is_ident_byte(bytes[i - 1]) => angle += 1,
            // `=>` and Swift's `->` are arrows, not the end of a type argument list.
            b'>' if angle > 0 && !matches!(bytes[i - 1], b'=' | b'-') => angle -= 1,
            b',' if depth == 0 && angle == 0 => {
                if let Some((start, end)) = current.take() {
                    out.push((start, &list[start..=end]));
                }
                return true;
            }
            _ => {}
        }
        if !c.is_ascii_whitespace() {
            current = Some((current.map_or(i, |(start, _)| start), i));
        }
        true
    });
    if let Some((start, end)) = current {
        out.push((start, &list[start..=end]));
    }
    out
}

/// Splits an entry at its first top-level `=` that is an assignment — not `==`, `!=`, `<=`,
/// `>=` or TypeScript's `=>`.
pub fn split_default(entry: &str, language: Language) -> (&str, Option<&str>) {
    let bytes = entry.as_bytes();
    let mut depth = 0i32;
    let mut at = None;
    walk_code(entry, 0, language, |i, c| {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'=' if depth == 0 => {
                let next = bytes.get(i + 1).copied().unwrap_or(b' ');
                let prev = if i > 0 { bytes[i - 1] } else { b' ' };
                if next != b'=' && next != b'>' && !matches!(prev, b'=' | b'!' | b'<' | b'>') {
                    at = Some(i);
                    return false;
                }
            }
            _ => {}
        }
        true
    });
    match at {
        Some(i) => (entry[..i].trim_end(), Some(entry[i + 1..].trim())),
        None => (entry, None),
    }
}

pub fn leading_ident(text: &str) -> &str {
    let n = text.bytes().take_while(|b| is_ident_byte(*b)).count();
    &text[..n]
}

/// One entry of a parameter list, starting at `at` in the list.
pub fn parse_param(entry: &str, at: usize, language: Language) -> Param {
    let mut param = Param {
        raw: entry.to_string(),
        name: String::new(),
        name_at: at,
        ty: None,
        default: None,
        optional: false,
        shares_type: false,
        kind: Kind::Plain,
        label: None,
    };
    match language {
        Language::C | Language::Cpp | Language::Java => return parse_c_param(param, language),
        Language::Swift => return parse_swift_param(param),
        _ => {}
    }
    let mut head = 0usize;
    match language {
        Language::Python => {
            if entry == "*" || entry == "/" {
                param.name = entry.to_string();
                param.kind = Kind::Marker;
                return param;
            }
            if entry.starts_with("**") {
                param.kind = Kind::Keywords;
                head = 2;
            } else if entry.starts_with('*') {
                param.kind = Kind::Variadic;
                head = 1;
            }
        }
        Language::TypeScript => {
            // A constructor's parameter properties carry modifiers before the name.
            loop {
                let rest = &entry[head..];
                let Some(m) = [
                    "public ",
                    "private ",
                    "protected ",
                    "readonly ",
                    "override ",
                ]
                .iter()
                .find(|m| rest.starts_with(**m)) else {
                    break;
                };
                head += m.len();
                head += entry[head..].len() - entry[head..].trim_start().len();
            }
            if entry[head..].starts_with("...") {
                param.kind = Kind::Variadic;
                head += 3;
            }
        }
        // A destructuring pattern (`{ a, b }`, `[a, b]`) starts with no name and keeps none.
        Language::JavaScript if entry.starts_with("...") => {
            param.kind = Kind::Variadic;
            head = 3;
        }
        _ => {}
    }
    param.name = leading_ident(&entry[head..]).to_string();
    param.name_at = at + head;
    let mut rest = entry[head + param.name.len()..].trim_start();
    if language == Language::Go {
        if !rest.is_empty() {
            if rest.starts_with("...") {
                param.kind = Kind::Variadic;
            }
            param.ty = Some(rest.to_string());
        }
        return param;
    }
    if language == Language::TypeScript
        && let Some(after) = rest.strip_prefix('?')
    {
        param.optional = true;
        rest = after.trim_start();
    }
    let (typed, default) = split_default(rest, language);
    param.default = default.map(str::to_string);
    param.ty = typed
        .strip_prefix(':')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    param
}

/// A C or C++ parameter: a declaration whose name is its last identifier (`const char *name`,
/// `int xs[]`) or sits inside the declarator (`int (*cb)(int)`), with a C++ default after it.
/// An array parameter is a pointer, which is what its field has to be: an array field would be
/// a different type, and one without a size would not compile at all.
pub fn parse_c_param(mut param: Param, language: Language) -> Param {
    let entry = param.raw.clone();
    // C's `...` and a C++ parameter pack (`Args&&... args`) both take any number of arguments.
    if entry.contains("...") {
        param.kind = Kind::Variadic;
        param.name = entry.rsplit("...").next().unwrap_or("").trim().to_string();
        return param;
    }
    let (declaration, default) = split_default(&entry, language);
    param.default = default.map(str::to_string);
    let bytes = declaration.as_bytes();
    // A function pointer or reference names itself inside the first parentheses.
    if let Some(p) = declaration.find("(*").or_else(|| declaration.find("(&")) {
        let start = p + 2 + declaration[p + 2..].len() - declaration[p + 2..].trim_start().len();
        let name = leading_ident(&declaration[start..]);
        param.name = name.to_string();
        param.name_at += start;
        param.ty = Some(declaration.to_string());
        return param;
    }
    let mut end = declaration.len();
    let mut suffixes: Vec<&str> = Vec::new();
    while end > 0 && bytes[end - 1] == b']' {
        let Some(open) = declaration[..end].rfind('[') else {
            break;
        };
        suffixes.insert(0, &declaration[open..end]);
        end = declaration[..open].trim_end().len();
    }
    let start = declaration[..end]
        .bytes()
        .rposition(|b| !is_ident_byte(b))
        .map_or(0, |i| i + 1);
    let head = declaration[..start].trim_end();
    // A lone type such as `int` or `const char *` names nothing.
    if head.is_empty() || head.ends_with("::") || start == end {
        param.ty = Some(declaration.to_string());
        return param;
    }
    let name = &declaration[start..end];
    param.name = name.to_string();
    param.name_at += start;
    let joined = |head: &str| {
        if head.ends_with(['*', '&']) {
            format!("{head}{name}")
        } else {
            format!("{head} {name}")
        }
    };
    param.ty = Some(match suffixes.split_first() {
        None => joined(head),
        Some((_, [])) => joined(&format!("{head} *")),
        Some((_, rest)) => format!("{head} (*{name}){}", rest.concat()),
    });
    param
}

/// A Swift parameter: an optional argument label before the name (`_ label`, `with name`), a
/// type after the colon, and a default after that; `Int...` takes several arguments.
pub fn parse_swift_param(mut param: Param) -> Param {
    let entry = param.raw.clone();
    let Some(colon) = entry.find(':') else {
        param.name = leading_ident(&entry).to_string();
        return param;
    };
    let names: Vec<(usize, &str)> = entry[..colon]
        .split_whitespace()
        .filter(|w| !w.starts_with('@'))
        .map(|w| (w.as_ptr() as usize - entry.as_ptr() as usize, w))
        .collect();
    let (label, (name_at, name)) = match names.as_slice() {
        [(_, label), name] => ((*label != "_").then(|| label.to_string()), *name),
        [name] => (Some(name.1.to_string()), *name),
        _ => (None, (0, "")),
    };
    param.label = label;
    param.name = name.to_string();
    param.name_at += name_at;
    let (typed, default) = split_default(entry[colon + 1..].trim(), Language::Swift);
    param.default = default.map(str::to_string);
    let ty = typed.trim();
    if ty.ends_with("...") {
        param.kind = Kind::Variadic;
    }
    param.ty = (!ty.is_empty()).then(|| ty.to_string());
    param
}

/// The receiver and the parameters of a parameter list in any language but Rust.
///
/// Python's receiver is a method's first parameter, `self` or `cls`, and TypeScript's is a
/// `this` parameter; both are kept verbatim and never bundled. Go's receiver is written before
/// the method's name, so it is not in this list at all, and C++ and Swift pass theirs
/// implicitly.
pub fn parse_params(list: &str, language: Language) -> (Option<String>, Vec<Param>) {
    let mut receiver = None;
    let mut out: Vec<Param> = Vec::new();
    // C writes an empty list as `(void)`.
    if matches!(language, Language::C | Language::Cpp) && list.trim() == "void" {
        return (None, out);
    }
    for (at, entry) in entries(list, language) {
        let param = parse_param(entry, at, language);
        let first = receiver.is_none() && out.is_empty();
        let is_receiver = first
            && param.kind == Kind::Plain
            && match language {
                Language::Python => param.name == "self" || param.name == "cls",
                Language::TypeScript => param.name == "this",
                _ => false,
            };
        if is_receiver {
            receiver = Some(entry.to_string());
            continue;
        }
        out.push(param);
    }
    if language == Language::Go {
        let mut group_type: Option<String> = None;
        for p in out.iter_mut().rev() {
            match &p.ty {
                Some(ty) => group_type = Some(ty.clone()),
                None => {
                    p.ty = group_type.clone();
                    p.shares_type = true;
                }
            }
        }
    }
    (receiver, out)
}
