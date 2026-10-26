/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::await_operand::{Token, can_end_primary};
use crate::parameter_object::Language;

/// Slash and JSX share expression-start classification in the operand lexer.
pub(super) fn can_start(content: &str, tokens: &[Token<'_>], lang: Language) -> bool {
    let Some(last) = tokens.last() else {
        return true;
    };
    // Keywords used as property names are operands, not expression introducers.
    if tokens.iter().rev().nth(1).is_some_and(|t| t.text == ".") {
        return false;
    }
    // A TypeScript postfix non-null assertion retains the operand's slash
    // context. Prefix logical negation still introduces an expression.
    if lang == Language::TypeScript && last.text == "!" {
        return can_start(content, &tokens[..tokens.len() - 1], lang);
    }
    if last.text == "default"
        && tokens
            .iter()
            .rev()
            .nth(1)
            .is_some_and(|t| t.text == "export")
    {
        return true;
    }
    if last.text == ")" {
        return last
            .pair
            .and_then(|open| open.checked_sub(1))
            .is_some_and(|i| {
                matches!(
                    tokens[i].text,
                    "if" | "while" | "for" | "with" | "switch" | "catch"
                )
            });
    }
    if last.text == "}" {
        let Some(open) = last.pair else {
            return false;
        };
        if let Some(declaration) = function_declaration(content, tokens, open) {
            return declaration;
        }
        if let Some(declaration) = class_declaration(content, tokens, open) {
            return declaration;
        }
        // A block may precede a regex statement; an object literal precedes
        // division. Keep the existing distinction at the block's opener.
        return statement_block(content, tokens, open);
    }
    if matches!(last.text, "++" | "--")
        || (last.text.starts_with('/') && last.text.len() > 1)
        || last.text.starts_with('`')
    {
        return false;
    }
    !can_end_primary(last)
        || matches!(
            last.text,
            "return"
                | "throw"
                | "case"
                | "yield"
                | "await"
                | "else"
                | "do"
                | "void"
                | "typeof"
                | "delete"
                | "instanceof"
                | "in"
                | "of"
                | "new"
        )
}

fn statement_block(content: &str, tokens: &[Token<'_>], open: usize) -> bool {
    // Substitution/JSX braces introduce expressions even when immediately
    // preceded by another container's closing brace. Never inherit statement
    // context through them when classifying an enclosed object literal.
    if tokens[open].expression_container {
        return false;
    }
    let Some(previous) = open.checked_sub(1) else {
        return true; // A bare block at the start of a program is a statement.
    };
    if content[tokens[previous].end..tokens[open].start]
        .contains(['\n', '\r', '\u{2028}', '\u{2029}'])
        && (can_end_primary(&tokens[previous])
            || matches!(tokens[previous].text, "++" | "--" | "return" | "yield")
            || tokens[previous].text.starts_with('`')
            || (tokens[previous].text.starts_with('/') && tokens[previous].text.len() > 1))
    {
        return true;
    }
    match tokens[previous].text {
        // An immediately nested brace inherits the enclosing context. In
        // particular, template/JSX expression braces must not turn an object
        // expression into a statement block (and division into a regex).
        "{" => statement_block(content, tokens, previous),
        ":" => statement_colon(content, tokens, previous),
        // Adjacent completed blocks/declarations can introduce a bare block.
        "}" | ")" | ">" | "else" | "try" | "catch" | "finally" | "do" | ";" => true,
        _ => false,
    }
}

fn statement_colon(content: &str, tokens: &[Token<'_>], colon: usize) -> bool {
    label_colon(content, tokens, colon) || case_colon(tokens, colon)
}

fn case_colon(tokens: &[Token<'_>], colon: usize) -> bool {
    let mut at = colon;
    let mut ternary_colons = 0usize;
    while let Some(previous) = at.checked_sub(1) {
        at = previous;
        let token = &tokens[at];
        if let Some(open) = token.pair.filter(|open| *open < at) {
            at = open;
            continue;
        }
        match token.text {
            ":" => ternary_colons += 1,
            "?" if !tokens
                .get(at + 1)
                .is_some_and(|next| next.text == "." && next.start == token.end) =>
            {
                if ternary_colons == 0 {
                    return false;
                }
                ternary_colons -= 1;
            }
            "case"
                if at
                    .checked_sub(1)
                    .is_none_or(|before| tokens[before].text != ".") =>
            {
                return at + 1 < colon && ternary_colons == 0;
            }
            ";" | "{" | "}" | "return" | "throw" | "yield" | "else" | "do" | "function"
            | "class" | "const" | "let" | "var" => return false,
            _ => {}
        }
    }
    false
}

fn label_colon(content: &str, tokens: &[Token<'_>], colon: usize) -> bool {
    let Some(label) = colon.checked_sub(1) else {
        return false;
    };
    // A label is an identifier at a statement boundary. Object properties and
    // conditional-expression colons must keep their operand/division context.
    let name = tokens[label].text;
    if !name.starts_with(|c: char| c.is_alphabetic() || matches!(c, '_' | '$'))
        || !name.chars().all(|c| super::syntax::is_ident(c) || c == '$')
    {
        return false;
    }
    let Some(previous) = label.checked_sub(1) else {
        return true;
    };
    // An identifier cannot continue a completed expression across a line
    // terminator. ASI ends that statement before this label, even without an
    // explicit semicolon. Restricted return/yield statements also end at that
    // line terminator (including one inside a comment). Property/conditional
    // colons still follow their own expression introducers.
    if content[tokens[previous].end..tokens[label].start]
        .contains(['\n', '\r', '\u{2028}', '\u{2029}'])
        && (can_end_primary(&tokens[previous])
            || matches!(tokens[previous].text, "++" | "--" | "return" | "yield")
            || tokens[previous].text.starts_with('`')
            || (tokens[previous].text.starts_with('/') && tokens[previous].text.len() > 1))
    {
        return true;
    }
    match tokens[previous].text {
        "{" => statement_block(content, tokens, previous),
        ":" => statement_colon(content, tokens, previous),
        ";" | "}" | ")" | "else" | "do" => true,
        _ => false,
    }
}

// Resolve the parameter pair rather than treating every body after `)` as a
// statement block: a function expression is an operand even without grouping.
fn function_declaration(content: &str, tokens: &[Token<'_>], body: usize) -> Option<bool> {
    let close = body.checked_sub(1)?;
    if tokens[close].text != ")" {
        return None;
    }
    let mut at = tokens[close].pair?.checked_sub(1)?;
    if !matches!(tokens[at].text, "function" | "*") {
        at = at.checked_sub(1)?; // Optional function name.
    }
    if tokens[at].text == "*" {
        at = at.checked_sub(1)?;
    }
    if tokens[at].text != "function" {
        return None;
    }
    if let Some(previous) = at.checked_sub(1)
        && tokens[previous].text == "async"
        && !content[tokens[previous].end..tokens[at].start].contains(['\n', '\r'])
    {
        at = previous;
    }
    Some(declaration_position(content, tokens, at))
}

fn declaration_position(content: &str, tokens: &[Token<'_>], at: usize) -> bool {
    at.checked_sub(1).is_none_or(|i| {
        let previous = &tokens[i];
        matches!(
            previous.text,
            ";" | "{" | "}" | "export" | "default" | "declare" | ")"
        ) || (content[previous.end..tokens[at].start].contains(['\n', '\r'])
            && previous.text != "new"
            && (can_end_primary(previous) || matches!(previous.text, "return" | "yield")))
    })
}

// Walk only the class header, skipping balanced extends expressions. A class
// expression remains an operand (including `class C {} / 2`), whereas a class
// declaration ends a statement and permits a following regex statement.
fn class_declaration(content: &str, tokens: &[Token<'_>], mut at: usize) -> Option<bool> {
    while let Some(previous) = at.checked_sub(1) {
        at = previous;
        let token = &tokens[at];
        if token.text == "class" {
            return Some(declaration_position(content, tokens, at));
        }
        // A function/method body cannot borrow a class from an earlier statement.
        if token.text == "function" {
            break;
        }
        if let Some(open) = token.pair.filter(|open| *open < at) {
            at = open;
        } else if matches!(token.text, ";" | "{" | "(" | "[" | "=" | ":" | ",") {
            break;
        }
    }
    None
}

pub(super) fn end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut at = start + 1;
    let mut in_class = false;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'\n' | b'\r' => return None,
            b'\\' => at += 2,
            b'[' => {
                in_class = true;
                at += 1;
            }
            b']' => {
                in_class = false;
                at += 1;
            }
            b'/' if !in_class => {
                at += 1;
                while bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
                    at += 1;
                }
                return Some(at);
            }
            _ => at += 1,
        }
    }
    None
}
