/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::is_ident;

/// The identifiers an expression reads, and why it may not be evaluated once for every place,
/// when it may not: a call (`f(x)`, `x.len()`), a macro, `?` or `.await`.
pub fn reads_and_effects(expr: &str) -> (Vec<String>, Option<String>) {
    let mut names = Vec::new();
    let mut effect = None;
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0;
    // The word after `as` is a type.
    let mut after_as = false;
    while i < chars.len() {
        let c = chars[i];
        if is_ident(c) && (i == 0 || !is_ident(chars[i - 1])) {
            let start = i;
            while i < chars.len() && is_ident(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let next = chars[i..].iter().find(|c| !c.is_whitespace()).copied();
            let after_dot = start > 0 && chars[start - 1] == '.';
            let is_type = std::mem::replace(&mut after_as, word == "as");
            if next == Some('(') {
                effect.get_or_insert(format!("it calls `{word}`"));
            } else if next == Some('!') && chars.get(i + 1) != Some(&'=') {
                effect.get_or_insert(format!("it expands the macro `{word}!`"));
            } else if !after_dot
                && !is_type
                && !word.chars().next().is_some_and(|c| c.is_ascii_digit())
                && !matches!(word.as_str(), "as" | "true" | "false")
                && !word.chars().next().is_some_and(char::is_uppercase)
            {
                names.push(word);
            }
            continue;
        }
        if c == '?' {
            effect.get_or_insert("it propagates an error with `?`".to_string());
        }
        i += 1;
    }
    if expr.contains(".await") {
        effect.get_or_insert("it awaits".to_string());
    }
    names.sort();
    names.dedup();
    (names, effect)
}

/// Every place `expr` occurs in `text[from..to]` as whole tokens, as byte offsets.
pub fn occurrences(text: &str, from: usize, to: usize, expr: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let region = &text[from..to];
    let mut at = 0;
    while let Some(i) = region[at..].find(expr) {
        let start = from + at + i;
        let end = start + expr.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        let first = expr.chars().next().unwrap_or(' ');
        let last = expr.chars().next_back().unwrap_or(' ');
        // `x(w + 1)` is a call's argument list, not `(w + 1)`: a name there would glue to it.
        let clean_start = !((is_ident(first) || first == '(') && before.is_some_and(is_ident));
        let clean_end = !((is_ident(last) || last == ')') && after.is_some_and(is_ident));
        if clean_start && clean_end {
            out.push(start);
        }
        at += i + expr.len().max(1);
    }
    out
}

/// Whether `name` is assigned (`name =`, `name +=`), mutably borrowed (`&mut name`) or bound
/// again (`let name`) in `text`.
pub fn changes(text: &str, name: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(i) = text[at..].find(name) {
        let start = at + i;
        let end = start + name.len();
        at = end;
        let whole = (start == 0 || !is_ident(bytes[start - 1] as char))
            && (end >= bytes.len() || !is_ident(bytes[end] as char));
        if !whole {
            continue;
        }
        let before = text[..start].trim_end();
        let mut after = text[end..].trim_start();
        // `name.field = ...` changes `name` too, and `name.method()` may: it can take `&mut self`.
        let mut segments = 0;
        while let Some(rest) = after.strip_prefix('.') {
            let field = rest.len() - rest.trim_start_matches(is_ident).len();
            if field == 0 {
                break;
            }
            segments += 1;
            after = rest[field..].trim_start();
        }
        let called = segments > 0 && after.starts_with('(');
        let assigned = (after.starts_with('=') && !after.starts_with("=="))
            || ["+=", "-=", "*=", "/=", "%=", "|=", "&=", "^=", "<<=", ">>="]
                .iter()
                .any(|op| after.starts_with(op));
        let borrowed = before.ends_with("&mut");
        let bound = before.ends_with("let") || before.ends_with("let mut");
        if assigned || borrowed || bound || called {
            return true;
        }
    }
    false
}

/// Whether evaluating `expr` can panic: a division or remainder, an index, or arithmetic that
/// overflows in a debug build.
pub fn can_panic(expr: &str) -> bool {
    expr.contains(['/', '%', '[', '+', '-', '*']) || expr.contains("<<")
}

/// Whether the occurrence at `at` is evaluated whenever the statement at `anchor` runs: it sits
/// at the anchor's own block level, nothing between them can leave early (`return`, `break`,
/// `continue`, `?`, a panicking macro), and nothing earlier in its own statement short-circuits
/// or defers it (`&&`, `||`, a closure).
pub fn surely_evaluated(text: &str, anchor: usize, statement: usize, at: usize) -> bool {
    let between = &text[anchor..at];
    let mut depth = 0i32;
    for c in between.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    let leaves = ["return", "break", "continue"].iter().any(|w| {
        between.match_indices(w).any(|(i, _)| {
            !between[..i].chars().next_back().is_some_and(is_ident)
                && !between[i + w.len()..].chars().next().is_some_and(is_ident)
        })
    }) || between.contains('?')
        || [
            "panic!",
            "unreachable!",
            "todo!",
            "unimplemented!",
            "assert",
        ]
        .iter()
        .any(|m| between.contains(m));
    let own = &text[statement.min(at)..at];
    depth == 0 && !leaves && !own.contains('|') && !own.contains("&&")
}

/// The span to replace for the occurrence `start..end`: with the parentheses around it when it
/// is written `(expr)` on its own, so `a + (s.x + 1)` becomes `a + x1`. The parentheses of a
/// call, an index or a method stay.
pub fn parenthesised(text: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let open = text[..start].trim_end();
    let close = text[end..].trim_start();
    if open.ends_with('(') && close.starts_with(')') {
        let before = open[..open.len() - 1].trim_end().chars().next_back();
        if !before.is_some_and(|c| is_ident(c) || matches!(c, ')' | ']' | '>' | '!')) {
            return open.len() - 1..text.len() - close.len() + 1;
        }
    }
    start..end
}

/// The bodies, as brace offsets, of the loops (`loop`, `while`, `for`) whose keyword lies in
/// `text[from..to]`.
pub fn loops_after(text: &str, from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for keyword in ["loop", "while", "for"] {
        let mut at = from;
        while let Some(i) = text[at..to].find(keyword) {
            let start = at + i;
            let end = start + keyword.len();
            at = end;
            let whole = !text[..start].chars().next_back().is_some_and(is_ident)
                && !text[end..].chars().next().is_some_and(is_ident);
            if !whole {
                continue;
            }
            if let Some(open) = text[end..].find('{').map(|i| end + i)
                && let Some(close) = crate::parameter_object::matching_bracket(text, open)
            {
                out.push((open, close));
            }
        }
    }
    out
}

/// The opening brace of the innermost block, from the function body `body_open` inward, that
/// holds both `first` and `last`.
pub fn innermost_block(text: &str, body_open: usize, first: usize, last: usize) -> usize {
    text[body_open..first]
        .match_indices('{')
        .map(|(i, _)| body_open + i)
        .rev()
        .find(|open| {
            crate::parameter_object::matching_bracket(text, *open).is_some_and(|close| close > last)
        })
        .unwrap_or(body_open)
}

/// Where the statement of the block `block_open` that holds `at` begins: after the last `;`, or
/// the last `}` that closes a statement (not one followed by `else`), at the block's own depth.
pub fn statement_start(text: &str, block_open: usize, at: usize) -> usize {
    let mut depth = 0i32;
    let mut start = block_open + 1;
    for (i, c) in text[block_open + 1..at].char_indices() {
        let i = block_open + 1 + i;
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' => depth -= 1,
            '}' => {
                depth -= 1;
                if depth == 0 && !text[i + 1..].trim_start().starts_with("else") {
                    start = i + 1;
                }
            }
            ';' if depth == 0 => start = i + 1,
            _ => {}
        }
    }
    start + (text[start..].len() - text[start..].trim_start().len())
}

/// The braces of the innermost function body that holds `at`: the nearest `fn ` before it whose
/// body, the first `{` after its parameter list, closes after it.
pub fn enclosing_body(text: &str, at: usize) -> Option<(usize, usize)> {
    let mut search = at;
    while let Some(fn_at) = text[..search].rfind("fn ") {
        search = fn_at;
        if fn_at > 0 && text[..fn_at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let name_at = fn_at + 3;
        let Some((_, _, close)) = crate::signature::param_span(text, name_at) else {
            continue;
        };
        let Some(open) = text[close..].find(['{', ';']).map(|i| close + i) else {
            continue;
        };
        if text.as_bytes()[open] != b'{' {
            continue;
        }
        if let Some(end) = crate::parameter_object::matching_bracket(text, open)
            && open < at
            && at < end
        {
            return Some((open, end));
        }
    }
    None
}
