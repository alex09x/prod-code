/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::extract_trait::lex::{lexical_code, next_code, previous_code};

/// Delimiters over the lexical mask, so literal bytes can never become structure.
pub(super) fn close(text: &str, code: &[bool], open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut stack = Vec::new();
    for i in open..bytes.len() {
        if !code[i] {
            continue;
        }
        match bytes[i] {
            b'(' => stack.push(b')'),
            b'[' => stack.push(b']'),
            b'{' => stack.push(b'}'),
            b')' | b']' | b'}' => {
                if stack.pop()? != bytes[i] {
                    return None;
                }
                if stack.is_empty() {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Skip type arguments, including arrows, const blocks and nested type delimiters.
fn after_angles(text: &str, code: &[bool], open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        if !code[i] {
            i += 1;
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => i = close(text, code, i)?,
            b'<' => depth += 1,
            b'>' if i > 0 && bytes[i - 1] == b'=' => return None,
            b'>' if i == 0 || bytes[i - 1] != b'-' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b';' => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// The selected call's complete argument list, past an optional Rust turbofish.
/// A function value still has no call boundary and must remain unmatched.
pub(super) fn call_args(text: &str, after_name: usize) -> Option<(usize, usize)> {
    let code = lexical_code(text);
    let mut i = next_code(text, &code, after_name)?;
    if text[i..].starts_with("::") {
        i = next_code(text, &code, i + 2)?;
        if text.as_bytes()[i] != b'<' {
            return None;
        }
        i = next_code(text, &code, after_angles(text, &code, i)?)?;
    }
    if text.as_bytes()[i] != b'(' {
        return None;
    }
    close(text, &code, i).map(|end| (i + 1, end))
}

pub(super) fn call_continues(text: &str, after: usize) -> bool {
    let code = lexical_code(text);
    next_code(text, &code, after)
        .is_some_and(|at| matches!(text.as_bytes()[at], b'.' | b'?' | b'['))
}

fn word_end(text: &str, from: usize) -> usize {
    text[from..]
        .char_indices()
        .take_while(|(_, c)| super::syntax::is_ident(*c))
        .last()
        .map_or(from, |(i, c)| from + i + c.len_utf8())
}

/// Locate an item's body across its complete signature, never by the preceding line.
fn function_body(text: &str, code: &[bool], after_fn: usize) -> Option<usize> {
    let i = next_code(text, code, after_fn)?;
    // `fn(...)` in a type is not a nested function declaration.
    let end = word_end(text, i);
    if end == i {
        return None;
    }
    signature_body(text, code, end)
}

/// A type macro's token tree is part of the signature, including brace trees.
/// A bare `!` never type has no preceding identifier and is not a macro.
fn signature_body(text: &str, code: &[bool], mut i: usize) -> Option<usize> {
    while let Some(at) = next_code(text, code, i) {
        match text.as_bytes()[at] {
            b'<' => i = after_angles(text, code, at)?,
            b'(' | b'[' => i = close(text, code, at)? + 1,
            b'!' if previous_word(text, code, at).is_some() => {
                let open = next_code(text, code, at + 1)?;
                if !matches!(text.as_bytes()[open], b'(' | b'[' | b'{') {
                    return None;
                }
                i = close(text, code, open)? + 1;
            }
            b'{' => return Some(at),
            b';' | b'=' | b'}' => return None,
            _ => i = at + text[at..].chars().next()?.len_utf8(),
        }
    }
    None
}

/// The preceding identifier token; the mask may return its final UTF-8 byte.
fn previous_word<'a>(text: &'a str, code: &[bool], before: usize) -> Option<(usize, &'a str)> {
    let end = previous_code(text, code, before)? + 1;
    let mut start = end;
    for (i, c) in text[..end].char_indices().rev() {
        if !super::syntax::is_ident(c) {
            break;
        }
        start = i;
    }
    (start < end).then_some((start, &text[start..end]))
}

/// A turbofish can finish a value (`S::<1>`), rather than an operator prefix.
/// Match its whole angle group: a later `>` in `S::<1> >> || ...` is an operator.
fn generic_value_end(text: &str, code: &[bool], end: usize) -> bool {
    (0..end).any(|at| {
        code[at]
            && text.as_bytes()[at] == b'<'
            && previous_code(text, code, at)
                .is_some_and(|prev| prev > 0 && &text.as_bytes()[prev - 1..=prev] == b"::")
            && after_angles(text, code, at) == Some(end)
    })
}

/// An operand may follow any operator, but never a completed operand. The caller
/// visits whole pipe operators so the second byte of `a || b` is not a prefix.
fn operand_start(text: &str, code: &[bool], at: usize) -> bool {
    let previous = previous_code(text, code, at);
    // The mask skips literals as well as comments. A literal is an operand:
    // `&"a" == &"b" || ...` must not look like `&|| ...`. Only trivia may
    // separate the expression prefix from the opening pipe.
    let mut trivia = previous.map_or(0, |prev| prev + 1);
    while trivia < at {
        if let Ok(Some((end, true))) = crate::signature::opaque_at(text, trivia, 0) {
            trivia = end;
        } else if text.as_bytes()[trivia].is_ascii_whitespace() {
            trivia += 1;
        } else {
            return false;
        }
    }
    let Some(prev) = previous else {
        return true;
    };
    if text.as_bytes()[prev] == b'>' && generic_value_end(text, code, prev + 1) {
        return false;
    }
    if let Some((start, word)) = previous_word(text, code, at) {
        // `break 'label || ...` has a label token between the keyword and value.
        // Follow tokens rather than lines, including Unicode labels and comments.
        if let Some(quote) = previous_code(text, code, start)
            && text.as_bytes()[quote] == b'\''
            && previous_word(text, code, quote).is_some_and(|(_, word)| word == "break")
        {
            return true;
        }
        return matches!(
            word,
            "move" | "async" | "return" | "mut" | "break" | "yield"
        );
    }
    !matches!(text.as_bytes()[prev], b')' | b']' | b'}' | b'.' | b'?')
}

/// The end of a return value or expression closure at its own delimiter level.
fn expression_end(text: &str, code: &[bool], from: usize) -> usize {
    let mut i = from;
    while i < text.len() {
        if !code[i] {
            i += 1;
            continue;
        }
        match text.as_bytes()[i] {
            b'<' => {
                let turbofish = previous_code(text, code, i).is_some_and(|prev| {
                    text.as_bytes()[prev] == b':' && prev > 0 && text.as_bytes()[prev - 1] == b':'
                });
                // Turbofish / qualified paths own their type-argument commas.
                // Qualified paths also occur after unary and binary operators.
                // Require their `::` suffix, and never scan across a match arrow.
                if (turbofish || operand_start(text, code, i))
                    && let Some(end) = after_angles(text, code, i)
                    && (turbofish
                        || next_code(text, code, end)
                            .is_some_and(|at| text[at..].starts_with("::")))
                {
                    i = end;
                    continue;
                }
            }
            b'(' | b'[' | b'{' => {
                let Some(end) = close(text, code, i) else {
                    return text.len();
                };
                i = end;
            }
            b';' | b'}' | b')' | b']' | b',' => return i,
            _ => {}
        }
        i += 1;
    }
    text.len()
}

fn after_closure(text: &str, code: &[bool], at: usize) -> Option<usize> {
    let mut i = at + 1;
    // Closure parameter types can themselves contain delimiters and function types.
    while i < text.len() {
        if code[i] {
            match text.as_bytes()[i] {
                b'|' => break,
                b'(' | b'[' | b'{' => i = close(text, code, i)?,
                // Parameter lists cannot cross statement or enclosing-body boundaries.
                // Never pair independent bitwise pipes in successive statements.
                b';' | b'}' => return None,
                _ => {}
            }
        }
        i += 1;
    }
    i = next_code(text, code, i + 1)?;
    if text[i..].starts_with("->") {
        return close(text, code, signature_body(text, code, i + 2)?).map(|end| end + 1);
    }
    Some(expression_end(text, code, i))
}

/// Return ranges belonging to this Rust function, excluding nested owners.
pub(super) fn returns(text: &str) -> Vec<(usize, usize)> {
    let code = lexical_code(text);
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if !code[i] {
            i += 1;
            continue;
        }
        // Only ASCII structure is examined bytewise; token slices use UTF-8 boundaries.
        if text.as_bytes()[i] == b'|' {
            if operand_start(text, &code, i)
                && let Some(end) = after_closure(text, &code, i)
            {
                i = end;
                continue;
            }
            // Consume the logical operator as a token; its second pipe cannot
            // introduce a closure. A later pipe in `builder | || ...` can.
            i += if text[i..].starts_with("||") { 2 } else { 1 };
            continue;
        }
        let end = word_end(text, i);
        if end == i {
            i += text[i..].chars().next().unwrap().len_utf8();
            continue;
        }
        match &text[i..end] {
            "fn" => {
                if let Some(open) = function_body(text, &code, end)
                    && let Some(close) = close(text, &code, open)
                {
                    i = close + 1;
                    continue;
                }
            }
            "async" => {
                if let Some(mut at) = next_code(text, &code, end) {
                    if &text[at..word_end(text, at)] == "move" {
                        at = next_code(text, &code, at + 4).unwrap_or(at);
                    }
                    if text.as_bytes()[at] == b'{'
                        && let Some(close) = close(text, &code, at)
                    {
                        i = close + 1;
                        continue;
                    }
                }
            }
            "return" => ranges.push((i, expression_end(text, &code, end))),
            _ => {}
        }
        i = end;
    }
    ranges
}
