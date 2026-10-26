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

use crate::make_static::Language;

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The receiver a first parameter becomes, when its type is the `impl`'s own type: `c: &mut
/// Counter` → `&mut self`, `c: &'a Self` → `&'a self`, `mut c: Counter` → `mut self`. The binding's
/// name comes back with it.
pub fn receiver_for(param: &str, owner: &str) -> Option<(String, String)> {
    let (pattern, ty) = param.split_once(':')?;
    let pattern = pattern.trim();
    let (binding_mut, name) = match pattern.strip_prefix("mut ") {
        Some(rest) => (true, rest.trim()),
        None => (false, pattern),
    };
    if name.is_empty() || !name.chars().all(is_ident) || name == "self" {
        return None;
    }
    let ty = ty.trim();
    let (reference, rest) = match ty.strip_prefix('&') {
        None => (String::new(), ty),
        Some(rest) => {
            let rest = rest.trim_start();
            let (lifetime, rest) = if rest.starts_with('\'') {
                let end = rest.find(char::is_whitespace)?;
                (format!("{} ", &rest[..end]), rest[end..].trim_start())
            } else {
                (String::new(), rest)
            };
            match rest.strip_prefix("mut ") {
                Some(after) => (format!("&{lifetime}mut "), after.trim_start()),
                None => (format!("&{lifetime}"), rest),
            }
        }
    };
    // Paths and all generic arguments are part of the receiver's identity.
    let referent = type_spelling(rest)?;
    let is_own = Some(&referent) == type_spelling("Self").as_ref()
        || Some(&referent) == type_spelling(owner).as_ref();
    if !is_own {
        return None;
    }
    let receiver = if reference.is_empty() && binding_mut {
        "mut self".to_string()
    } else {
        format!("{reference}self")
    };
    Some((receiver, name.to_string()))
}

/// The complete self type of an inherent impl, rather than its display/base name.
pub(crate) fn impl_self_type(header: &str) -> Option<String> {
    let original = header;
    let header = crate::signature::blank_comments(header)?;
    let mut rest = header.trim().strip_prefix("impl")?.trim_start();
    if rest.starts_with('<') {
        let mut depth = 0;
        let end = rest.char_indices().find_map(|(i, c)| {
            match c {
                '<' => depth += 1,
                '>' if !rest[..i].ends_with('-') => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(i + c.len_utf8())
        })?;
        rest = rest[end..].trim_start();
    }
    let end = rest
        .match_indices("where")
        .find_map(|(i, _)| {
            (!rest[..i].chars().next_back().is_some_and(is_ident)
                && !rest[i + 5..].chars().next().is_some_and(is_ident))
            .then_some(i)
        })
        .unwrap_or(rest.len());
    let ty = rest[..end].trim();
    let start = ty.as_ptr() as usize - header.as_ptr() as usize;
    // Blanking is only for locating the header. Preserve its original tokens,
    // especially doc comments that are attributes in const-argument macros.
    (!ty.is_empty()).then(|| original[start..start + ty.len()].to_string())
}

/// Compare token spellings, never concatenated text: `dyn Trait` is not `dynTrait`.
/// The shared lexer keeps raw identifiers, lifetimes and prefixed literals whole.
/// Retain jointness where its intentionally split tokens could change Rust meaning
/// (literal suffixes/exponents, Unicode continuations and macro punctuation).
pub(super) fn type_spelling(text: &str) -> Option<Vec<(&str, bool)>> {
    use crate::fixture::builder::lexer::{Kind, lex};

    let tokens = lex(text).ok()?;
    let mut out = Vec::new();
    let mut end = 0;
    let mut groups: Vec<(&str, bool)> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        safe_type_trivia(&text[end..token.start])?;
        let spelling = &text[token.start..token.end];
        let joint = if i == 0 {
            false
        } else {
            let prev = &text[tokens[i - 1].start..end];
            let last = prev.chars().next_back()?;
            let first = spelling.chars().next()?;
            let xid = unicode_ident::is_xid_continue;
            let punct = |kind, c: char| kind == Kind::Punct && !"()[]{}".contains(c);
            let numeric = tokens[i - 1].kind == Kind::Literal
                && prev.starts_with(|c: char| c.is_ascii_digit());
            let exponent_sign = matches!(prev, "+" | "-")
                && i >= 2
                && tokens[i - 2].kind == Kind::Literal
                && text[tokens[i - 2].start..tokens[i - 2].end].ends_with(['e', 'E']);
            // Ordinary type grammar consumes punctuation separately, including
            // generic brackets and references (`<&T`, `>>`, `&&T`). Only path
            // separators, function arrows and variadics need compound punctuation.
            // Macros and expressions can observe every punctuation joint.
            let punct_sensitive = groups.last().is_some_and(|(_, expression)| *expression)
                || matches!((prev, spelling), (":", ":") | ("-", ">") | (".", "."));
            let sensitive = (xid(last) && (xid(first) || matches!(first, '\"' | '\'')))
                || (matches!(last, '\"' | '\'' | '#') && xid(first))
                || (punct(tokens[i - 1].kind, last) && punct(token.kind, first) && punct_sensitive)
                || (numeric && prev.ends_with(['e', 'E']) && matches!(first, '+' | '-'))
                || (numeric && first == '.')
                || (exponent_sign && first.is_ascii_digit());
            sensitive && end == token.start
        };
        out.push((spelling, joint));
        if token.kind == Kind::Punct {
            match spelling {
                "(" | "[" | "{" => {
                    let expression = groups.last().is_some_and(|(_, expression)| *expression)
                        || spelling == "{"
                        || (i > 0 && &text[tokens[i - 1].start..end] == "!");
                    groups.push((spelling, expression));
                }
                // Brackets begin with a slice/array element type. Only an array's
                // length after `;` is an expression; macros inherit token context.
                ";" => {
                    if let Some(("[", expression)) = groups.last_mut() {
                        *expression = true;
                    }
                }
                ")" | "]" | "}" => {
                    groups.pop()?;
                }
                _ => {}
            }
        }
        end = token.end;
    }
    if !groups.is_empty() {
        return None;
    }
    safe_type_trivia(&text[end..])?;
    Some(out)
}

/// Doc comments expand into attributes, including inside const-argument macros.
/// The shared lexer drops comments; refuse those rather than mistake them for trivia.
fn safe_type_trivia(text: &str) -> Option<()> {
    let mut at = 0;
    while at < text.len() {
        if let Some((end, comment)) = crate::signature::opaque_at(text, at, 0).ok()? {
            let rest = &text[at..];
            if !comment
                || rest.starts_with("//!")
                || rest.starts_with("/*!")
                || (rest.starts_with("///") && !rest.starts_with("////"))
                || (rest.starts_with("/**") && !rest.starts_with("/***"))
            {
                return None;
            }
            at = end;
        } else {
            let c = text[at..].chars().next()?;
            if !c.is_whitespace() {
                return None;
            }
            at += c.len_utf8();
        }
    }
    Some(())
}

/// The receiver a first argument becomes: `&mut c` and `&c` lose the borrow, which method-call
/// syntax takes by itself, and anything but a path or a call chain is parenthesized.
pub fn receiver_of(argument: &str) -> String {
    let a = argument.trim();
    let a = a
        .strip_prefix("&mut ")
        .or_else(|| a.strip_prefix('&'))
        .map(str::trim_start)
        .unwrap_or(a);
    let mut depth = 0i32;
    let mut simple = !a.is_empty() && a.chars().next().is_some_and(is_ident);
    for c in a.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if depth > 0 => {}
            c if is_ident(c) || c == '.' || c == ':' => {}
            _ => simple = false,
        }
    }
    if simple {
        a.to_string()
    } else {
        format!("({a})")
    }
}

/// Where the path in front of a call's name begins: `Counter::` or `crate::m::Counter::` or `Self::`.
pub(crate) fn path_start(text: &str, name_at: usize) -> usize {
    let mut at = name_at;
    loop {
        let before = &text[..at];
        let Some(stripped) = before.strip_suffix("::") else {
            return at;
        };
        let segment = stripped
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_ident(*c))
            .last()
            .map_or(stripped.len(), |(i, _)| i);
        if segment == stripped.len() {
            return at;
        }
        at = segment;
    }
}

pub fn split_call_arguments(args_str: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth_p = 0i32;
    let mut depth_b = 0i32;
    let mut depth_c = 0i32;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;

    for c in args_str.chars() {
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                in_quote = None;
            }
            current.push(c);
            continue;
        }
        match c {
            '"' | '\'' | '`' => {
                in_quote = Some(c);
                current.push(c);
            }
            '(' => {
                depth_p += 1;
                current.push(c);
            }
            ')' => {
                depth_p -= 1;
                current.push(c);
            }
            '[' => {
                depth_b += 1;
                current.push(c);
            }
            ']' => {
                depth_b -= 1;
                current.push(c);
            }
            '{' => {
                depth_c += 1;
                current.push(c);
            }
            '}' => {
                depth_c -= 1;
                current.push(c);
            }
            ',' if depth_p == 0 && depth_b == 0 && depth_c == 0 => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        args.push(current.trim().to_string());
    }
    args
}

pub(crate) fn receiver_argument<'a>(argument: &'a str, language: Language) -> &'a str {
    let argument = argument.trim();
    let label_separator = match language {
        Language::Swift => Some(':'),
        Language::Python => Some('='),
        _ => None,
    };
    let Some(separator) = label_separator else {
        return argument;
    };
    let Some((label, value)) = argument.split_once(separator) else {
        return argument;
    };
    let label = label.trim();
    let mut chars = label.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(is_ident);
    if valid { value.trim() } else { argument }
}
