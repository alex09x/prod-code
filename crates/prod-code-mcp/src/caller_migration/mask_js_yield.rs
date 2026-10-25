/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// A masked regex after a contextual `yield` must retain possible caller evidence
/// when the enclosing function grammar cannot be proven by this lexical view.
pub(crate) fn ambiguous_regex_prefix(text: &[u8], code: &[bool]) -> bool {
    let tokens = tokens(text, code);
    let Some(&(start, word)) = tokens.last() else {
        return false;
    };
    word == b"yield"
        && !tokens
            .iter()
            .rev()
            .nth(1)
            .is_some_and(|(_, token)| matches!(*token, b"." | b"#"))
        && is_operator(&text[..start], &code[..start]).is_none()
}

/// `yield` is an operator in a generator body, but an identifier in a sloppy
/// ordinary function or script. Walk only executable tokens and stop at the
/// nearest function boundary. Unknown headers remain ambiguous, never proof.
pub(super) fn is_operator(text: &[u8], code: &[bool]) -> Option<bool> {
    let tokens = tokens(text, code);
    let arrow = expression_arrow(text, &tokens);
    let arrow_grammar = |arrow: usize| {
        // Before an arrow's first operand there is no possible completed body
        // or ASI boundary. Literal text must not masquerade as an empty body.
        // Later operands can outlive a lexical arrow scope (especially across
        // template delimiters); keep those cases ambiguous instead of inheriting
        // the outer generator's grammar.
        text[tokens[arrow].0 + 1..]
            .iter()
            .all(|b| b.is_ascii_whitespace() || *b == b'(')
            .then_some(false)
    };
    let mut scopes = Vec::new();
    for (i, (_, token)) in tokens.iter().enumerate() {
        match *token {
            b"{" | b"(" | b"[" => scopes.push(i),
            b"}" | b")" | b"]" => {
                let open = scopes.pop()?;
                if !matches!(
                    (tokens[open].1, *token),
                    (b"{", b"}") | (b"(", b")") | (b"[", b"]")
                ) {
                    return None;
                }
            }
            _ => {}
        }
    }
    for boundary in scopes.into_iter().rev() {
        if let Some(arrow) = arrow.filter(|&arrow| arrow > boundary) {
            return arrow_grammar(arrow);
        }
        if tokens[boundary].1 == b"(" && function_parameters(&tokens[..boundary]) {
            // A nested function's parameters precede its body boundary and do
            // not inherit an outer generator's yield grammar. Body-based caller
            // ownership cannot prove this initializer safe to await: retain the
            // possible division caller as unmatched evidence before any writes.
            return None;
        }
        if tokens[boundary].1 != b"{" {
            continue;
        }
        let before = &tokens[..boundary];
        let Some(&(_, last)) = before.last() else {
            continue;
        };
        if last == b">" && before.iter().rev().nth(1).is_some_and(|(_, t)| *t == b"=") {
            return Some(false); // A block-bodied arrow is not a generator.
        }
        if last != b")" {
            // These delimiters and statement headers prove an ordinary nested
            // block. Object/class bodies and other headers are not scope proof.
            if matches!(
                last,
                b"{" | b"}" | b";" | b"else" | b"try" | b"finally" | b"do"
            ) {
                continue;
            }
            return None;
        }
        let mut depth = 0;
        let open = before.iter().rposition(|(_, t)| match *t {
            b")" => {
                depth += 1;
                false
            }
            b"(" => {
                depth -= 1;
                depth == 0
            }
            _ => false,
        })?;
        let header: Vec<_> = before[..open].iter().map(|(_, t)| *t).collect();
        let last = *header.last()?;
        let previous = header.iter().rev().nth(1).copied();
        let keyword = |at: usize| {
            header[at] == b"function"
                && !at
                    .checked_sub(1)
                    .is_some_and(|i| matches!(header[i], b"." | b"#"))
        };
        if last == b"*" && header.len() >= 2 && keyword(header.len() - 2) {
            return Some(true); // Anonymous generator function.
        }
        if previous == Some(&b"*"[..]) {
            let mut prefix = header.len().checked_sub(3)?;
            if keyword(prefix) {
                return Some(true);
            }
            // Method modifiers need a member boundary, not just a matching
            // spelling in a possible multiplication expression.
            while matches!(header[prefix], b"async" | b"static") {
                prefix = prefix.checked_sub(1)?;
            }
            if generator_member(text, &tokens, prefix) {
                return Some(true);
            }
            return None;
        }
        if keyword(header.len() - 1) || (header.len() >= 2 && keyword(header.len() - 2)) {
            return Some(false);
        }
        if matches!(
            last,
            b"if" | b"for" | b"while" | b"switch" | b"catch" | b"with"
        ) {
            if previous.is_some_and(|t| matches!(t, b"." | b"#")) {
                return None;
            }
            continue;
        }
        return None; // Method, computed name, or a call followed by a block.
    }
    if let Some(arrow) = arrow {
        return arrow_grammar(arrow);
    }
    Some(false) // Script-level identifier; strict/module uses are syntax errors.
}

/// Recognize function parameter boundaries, including anonymous/named generators.
/// Member property spellings do not establish a new function grammar.
pub(super) fn function_parameters(header: &[(usize, &[u8])]) -> bool {
    let Some(mut at) = header.len().checked_sub(1) else {
        return false;
    };
    if header[at].1 != b"function" {
        let Some(previous) = at.checked_sub(1) else {
            return false;
        };
        at = previous;
        if header[at].1 == b"*" {
            let Some(previous) = at.checked_sub(1) else {
                return false;
            };
            at = previous;
        }
    }
    header[at].1 == b"function"
        && !at
            .checked_sub(1)
            .is_some_and(|previous| matches!(header[previous].1, b"." | b"#"))
}

/// A modifier spelling and a statement delimiter are also valid multiplication
/// operands followed by an ASI block. Prove the enclosing member container and
/// a boundary at its own delimiter depth before recognizing a generator method.
fn generator_member(text: &[u8], tokens: &[(usize, &[u8])], boundary: usize) -> bool {
    let mut containers = Vec::new();
    for (i, (_, token)) in tokens[..=boundary].iter().enumerate() {
        match *token {
            b"{" | b"(" | b"[" => containers.push(i),
            b"}" | b")" | b"]" => {
                let Some(open) = containers.pop() else {
                    return false;
                };
                if !matches!(
                    (tokens[open].1, *token),
                    (b"{", b"}") | (b"(", b")") | (b"[", b"]")
                ) {
                    return false;
                }
            }
            _ => {}
        }
    }
    let Some(&open) = containers.last() else {
        return false;
    };
    if tokens[open].1 != b"{" || open == 0 {
        return false;
    }
    let before = open - 1;
    // Tokens exclude literal text. An intervening string/template/regex can
    // end an expression before an ordinary ASI block; it is not trivia proof.
    if !trivia_only(&text[tokens[before].0 + tokens[before].1.len()..tokens[open].0]) {
        return false;
    }
    let class_keyword = if tokens[before].1 == b"class" {
        Some(before)
    } else {
        before.checked_sub(1).filter(|&at| {
            tokens[at].1 == b"class" && trivia_only(&text[tokens[at].0 + 5..tokens[before].0])
        })
    };
    let class = class_keyword.is_some_and(|at| {
        at.checked_sub(1)
            .is_none_or(|previous| !matches!(tokens[previous].1, b"." | b"#"))
    });
    let object = tokens[before].1 == b"=";
    match tokens[boundary].1 {
        b"{" => boundary == open && (class || object),
        b"," => object,
        b";" | b"}" => class,
        _ => false,
    }
}

fn trivia_only(mut text: &[u8]) -> bool {
    loop {
        text = text.trim_ascii_start();
        if text.is_empty() {
            return true;
        }
        if text.starts_with(b"//") {
            text = &text[text.iter().position(|&b| b == b'\n').unwrap_or(text.len())..];
        } else if text.starts_with(b"/*") {
            let Some(end) = text.windows(2).position(|pair| pair == b"*/") else {
                return false;
            };
            text = &text[end + 2..];
        } else {
            return false;
        }
    }
}

/// Track possible expression arrows until a definite enclosing delimiter or
/// statement terminates them. Commas are deliberately not proof of termination:
/// interpolation delimiters are absent from the executable mask, so a comma in
/// `${a, b}` must not end an arrow outside that interpolation.
pub(super) fn expression_arrow(text: &[u8], tokens: &[(usize, &[u8])]) -> Option<usize> {
    let mut depth = 0usize;
    let mut arrows = Vec::new();
    for (i, (_, token)) in tokens.iter().enumerate() {
        match *token {
            b"(" | b"[" | b"{" => depth += 1,
            b")" | b"]" | b"}" => {
                depth = depth.saturating_sub(1);
                arrows.retain(|&(_, at_depth)| at_depth <= depth);
            }
            b";" => arrows.retain(|&(_, at_depth)| at_depth < depth),
            b">" if i > 0
                && tokens[i - 1].1 == b"="
                && tokens[i - 1].0 + 1 == tokens[i].0
                && tokens.get(i + 1).is_none_or(|&(next, t)| {
                    // A brace inside a following template is an expression,
                    // not the arrow's block body. Require a raw trivia-only
                    // gap before treating that brace as the body boundary.
                    t != b"{"
                        || !text[tokens[i].0 + 1..next]
                            .iter()
                            .all(u8::is_ascii_whitespace)
                }) =>
            {
                arrows.push((i, depth));
            }
            _ => {}
        }
    }
    arrows.last().map(|&(arrow, _)| arrow)
}

pub(super) fn tokens<'a>(text: &'a [u8], code: &[bool]) -> Vec<(usize, &'a [u8])> {
    let identifier = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$' | 0x80..=0xff);
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if !code[i] || text[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        if identifier(text[start]) {
            while i < text.len() && code[i] && identifier(text[i]) {
                i += 1;
            }
        }
        tokens.push((start, &text[start..i]));
    }
    tokens
}
