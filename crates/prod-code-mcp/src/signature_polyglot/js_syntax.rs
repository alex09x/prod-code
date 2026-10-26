/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::await_operand::{Token, call_tokens, tokens};
use crate::parameter_object::Language;

/// Use the operand lexer's JSX/template/regex boundaries for both the outer
/// call span and its comma-separated arguments. Markup never supplies syntax.
pub(super) fn call_arguments(
    content: &str,
    after_name: usize,
    lang: Language,
    jsx: bool,
) -> Option<(usize, usize, Vec<String>)> {
    let tokens = call_tokens(content, after_name, lang, jsx)?;
    let open = 0;
    let close = tokens[open].pair?;
    let start = tokens[open].end;
    let end = tokens[close].start;
    let mut arguments = Vec::new();
    let mut last = start;
    let mut i = open + 1;
    let mut angles = 0usize;
    while i < close {
        let token = &tokens[i];
        if let Some(pair) = token.pair.filter(|pair| *pair > i) {
            i = pair + 1;
            continue;
        }
        match token.text {
            "<" => angles += 1,
            ">" => angles = angles.saturating_sub(1),
            "," if angles == 0 => {
                arguments.push(content[last..token.start].trim().to_string());
                last = token.end;
            }
            _ => {}
        }
        i += 1;
    }
    if !content[last..end].trim().is_empty() {
        arguments.push(content[last..end].trim().to_string());
    }
    Some((start, end, arguments))
}

/// Only module binding/specifier tokens are ignorable references. A string,
/// JSX text, dynamic import argument or exported expression is executable data.
pub(super) fn module_reference(content: &str, at: usize, lang: Language, jsx: bool) -> bool {
    let Some(tokens) = tokens(content, lang, usize::MAX, usize::MAX, jsx) else {
        return false;
    };
    for (i, token) in tokens.iter().enumerate() {
        let end = match token.text {
            "import" if module_keyword(content, &tokens, i) => import_end(&tokens, i + 1),
            "export" if module_keyword(content, &tokens, i) => export_end(&tokens, i + 1),
            _ => None,
        };
        if end.is_some_and(|end| token.end <= at && at < end) {
            return true;
        }
        if matches!(token.text, "const" | "let" | "var")
            && require_binding(&tokens, i)
                .is_some_and(|eq| token.end <= at && at < tokens[eq].start)
        {
            return true;
        }
    }
    false
}

/// `const m = require('y')` or `const { a } = require('y')`: the index of the `=` when the
/// declaration binds a CommonJS module and nothing else.
fn require_binding(tokens: &[Token], keyword: usize) -> Option<usize> {
    let mut i = keyword + 1;
    while i < tokens.len() && tokens[i].text != "=" {
        if matches!(tokens[i].text, ";" | ",") {
            return None;
        }
        i = tokens[i].pair.filter(|pair| *pair > i).unwrap_or(i) + 1;
    }
    let eq = i;
    let call = tokens.get(eq + 1..eq + 5)?;
    let quoted = |t: &str| t.len() >= 2 && (t.starts_with('\'') || t.starts_with('"'));
    (call[0].text == "require"
        && call[1].text == "("
        && quoted(call[2].text)
        && call[3].text == ")")
        .then_some(eq)
}

fn module_keyword(content: &str, tokens: &[Token<'_>], at: usize) -> bool {
    // Reserved words in property access (including optional access) or non-statement
    // contexts are not module statements. A following ASI block remains executable code.
    if tokens[..at].iter().any(|token| {
        matches!(token.text, "(" | "[" | "{") && token.pair.is_some_and(|close| close > at)
    }) {
        return false;
    }
    let Some(previous) = at.checked_sub(1) else {
        return true;
    };
    if tokens[previous].text == "." {
        return false;
    }
    tokens[previous].text == ";"
        || (tokens[previous].text == "}"
            && !tokens[previous]
                .pair
                .is_some_and(|open| tokens[open].expression_container))
        || content[tokens[previous].end..tokens[at].start]
            .contains(['\n', '\r', '\u{2028}', '\u{2029}'])
}

fn export_end(tokens: &[Token<'_>], mut at: usize) -> Option<usize> {
    // Type-only re-exports carry the same binding references as value exports.
    // Skip just the modifier, retaining the bounded named/namespace clause.
    if tokens.get(at)?.text == "type" {
        at += 1;
    }
    match tokens.get(at)?.text {
        "{" => tokens[at].pair.map(|close| tokens[close].end),
        "*" => namespace_end(tokens, at).and_then(|end| source_end(tokens, end)),
        _ => None,
    }
}

// Recognize a bounded module clause rather than looking ahead for the next
// quote/semicolon, which may belong to a completely different statement.
fn import_end(tokens: &[Token<'_>], mut at: usize) -> Option<usize> {
    if quoted(tokens.get(at)?) {
        return Some(tokens[at].start);
    }
    if tokens[at].text == "type"
        && tokens
            .get(at + 1)
            .is_some_and(|t| !matches!(t.text, "from" | "=" | ","))
    {
        at += 1;
    }
    if identifier(tokens.get(at)?) {
        at += 1;
        if tokens.get(at)?.text == "=" {
            at += 1;
            if tokens.get(at)?.text == "require" && tokens.get(at + 1)?.text == "(" {
                let close = tokens[at + 1].pair?;
                return (close == at + 3 && quoted(&tokens[at + 2])).then_some(tokens[close].end);
            }
            if !identifier(tokens.get(at)?) {
                return None;
            }
            at += 1;
            while tokens.get(at).is_some_and(|t| t.text == ".") {
                if !identifier(tokens.get(at + 1)?) {
                    return None;
                }
                at += 2;
            }
            return Some(tokens[at - 1].end);
        }
        if tokens.get(at)?.text != "," {
            return source_end(tokens, at);
        }
        at += 1;
    }
    let end = if tokens.get(at)?.text == "{" {
        tokens[at].pair? + 1
    } else {
        namespace_end(tokens, at)?
    };
    source_end(tokens, end)
}

fn namespace_end(tokens: &[Token<'_>], at: usize) -> Option<usize> {
    if tokens.get(at)?.text != "*" {
        return None;
    }
    if tokens.get(at + 1)?.text == "as" && identifier(tokens.get(at + 2)?) {
        Some(at + 3)
    } else {
        Some(at + 1)
    }
}

fn source_end(tokens: &[Token<'_>], at: usize) -> Option<usize> {
    (tokens.get(at)?.text == "from" && quoted(tokens.get(at + 1)?)).then_some(tokens[at + 1].start)
}

fn identifier(token: &Token<'_>) -> bool {
    token
        .text
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || matches!(c, '_' | '$'))
        && token
            .text
            .chars()
            .all(|c| super::syntax::is_ident(c) || c == '$')
}

fn quoted(token: &Token<'_>) -> bool {
    token.text.starts_with(['\'', '"'])
}
