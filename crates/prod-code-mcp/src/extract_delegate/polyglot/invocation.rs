/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::lexical::trivia;
use crate::extract_delegate::common::is_ident;
use crate::parameter_object::{
    Language,
    syntax::{close_in, walk_code},
};

/// Parentheses and erased TS assertions retain a member's receiver. Follow these
/// wrappers, but reject only when the resulting member is actually called/tagged.
pub(super) fn invoked(text: &str, offset: usize, len: usize, lang: Language) -> bool {
    let mut tail = &text[offset + len..];
    loop {
        tail = trivia(tail, lang);
        if tail.starts_with('(') || tail.starts_with('`') {
            return true;
        }
        if let Some(rest) = tail.strip_prefix("?.") {
            tail = trivia(rest, lang);
            return tail.starts_with('(') || generic_call(tail, lang);
        }
        if generic_call(tail, lang) {
            return true;
        }
        if let Some(rest) = tail.strip_prefix(')') {
            let close = tail.as_ptr() as usize - text.as_ptr() as usize;
            if matches!(lang, Language::JavaScript | Language::TypeScript)
                && !transparent_close(text, offset, close, lang)
            {
                return false;
            }
            tail = rest;
            continue;
        }
        if lang == Language::TypeScript {
            if let Some(rest) = tail.strip_prefix('!').filter(|r| !r.starts_with('=')) {
                tail = rest;
                continue;
            }
            if let Some(rest) = assertion(tail) {
                let rest = trivia(rest, lang);
                tail = &rest[type_end(rest, lang)..];
                continue;
            }
        }
        return false;
    }
}

/// A scalar argument in `f(a.city)()` is not the callee. Only unwrap a closing
/// parenthesis whose opener groups this member, rather than calling a function.
fn transparent_close(text: &str, field: usize, close: usize, lang: Language) -> bool {
    let Some((open, _)) = text[..field].rmatch_indices('(').find(|(open, _)| {
        super::lexical::reference_is_code(text, *open, lang)
            && close_in(text, *open, lang) == Some(close)
    }) else {
        return false;
    };
    let previous = previous_code(text, open, lang);
    let Some((i, c)) = previous else {
        return true;
    };
    if is_ident(c) {
        let prefix = &text[..i + c.len_utf8()];
        let word = prefix.rsplit(|c| !is_ident(c)).next().unwrap_or("");
        return matches!(
            word,
            "return"
                | "throw"
                | "await"
                | "yield"
                | "typeof"
                | "void"
                | "delete"
                | "new"
                | "case"
                | "default"
                | "extends"
                | "in"
                | "of"
                | "instanceof"
                | "else"
                | "do"
        );
    }
    if c == ')' {
        // An unbraced control body starts a statement, unlike a chained call's
        // argument list (`factory()(a.city)()`). Its grouping retains `a`.
        return control_condition(text, i, lang);
    }
    if c == '}' {
        return statement_block(text, i, lang);
    }
    !matches!(c, ')' | ']' | '}' | '.' | '\'' | '"' | '`')
}

fn matching_open(text: &str, close: usize, delimiter: char, lang: Language) -> Option<usize> {
    text[..close].rmatch_indices(delimiter).find_map(|(i, _)| {
        (super::lexical::reference_is_code(text, i, lang) && close_in(text, i, lang) == Some(close))
            .then_some(i)
    })
}

fn previous_word(text: &str, end: usize, lang: Language) -> Option<(usize, &str)> {
    let (i, c) = previous_code(text, end, lang)?;
    if !is_ident(c) {
        return None;
    }
    let word = text[..i + c.len_utf8()].rsplit(|c| !is_ident(c)).next()?;
    Some((i + c.len_utf8() - word.len(), word))
}

fn control_condition(text: &str, close: usize, lang: Language) -> bool {
    matching_open(text, close, '(', lang).is_some_and(|open| {
        previous_word(text, open, lang).is_some_and(|(start, word)| {
            matches!(word, "if" | "while" | "for" | "with" | "switch" | "catch")
                && !matches!(previous_code(text, start, lang), Some((_, '.')))
        })
    })
}

/// A statement's completed block allows a new grouped expression. A function
/// expression's body can instead be followed by its argument list, so retain
/// that distinction (`const f = function(v) { return () => v; }(a.city)()`).
fn statement_block(text: &str, close: usize, lang: Language) -> bool {
    let Some(open) = matching_open(text, close, '{', lang) else {
        return false;
    };
    if let Some(statement) = declaration_header(text, open, lang) {
        return statement;
    }
    let Some((i, c)) = previous_code(text, open, lang) else {
        return true;
    };
    if c == ')' {
        return control_condition(text, i, lang);
    }
    if let Some((_, word)) = previous_word(text, open, lang) {
        if matches!(word, "else" | "do" | "try" | "catch" | "finally") {
            return true;
        }
        return false;
    }
    matches!(c, ';' | '{' | '}' | ':')
}

/// Find the declaration owning this body, including function/class expressions
/// with object return annotations and erased TypeScript declarations.
/// A prior declaration body or statement cannot be a header.
fn declaration_header(text: &str, open: usize, lang: Language) -> Option<bool> {
    [
        "function",
        "class",
        "interface",
        "enum",
        "namespace",
        "module",
        "type",
    ]
    .into_iter()
    .filter(|keyword| lang == Language::TypeScript || matches!(*keyword, "function" | "class"))
    .find_map(|keyword| {
        text[..open].rmatch_indices(keyword).find_map(|(start, _)| {
            if !super::lexical::reference_is_code(text, start, lang)
                || text[..start].chars().next_back().is_some_and(is_ident)
                || text[start + keyword.len()..]
                    .chars()
                    .next()
                    .is_some_and(is_ident)
                || matches!(previous_code(text, start, lang), Some((_, '.')))
            {
                return None;
            }
            let header = &text[start + keyword.len()..open];
            let mut stack = Vec::new();
            let mut return_type = false;
            let mut valid = true;
            walk_code(header, 0, lang, |i, c| {
                if stack.is_empty() {
                    match c {
                        b':' if keyword == "function" => return_type = true,
                        b'{' => {
                            // An object return type can begin after a type
                            // separator; a completed body cannot continue it.
                            valid = return_type
                                && previous_code(header, i, lang)
                                    .is_some_and(|(_, c)| matches!(c, ':' | '=' | '|' | '&'));
                        }
                        b';' | b'}' => valid = false,
                        b'=' if !header[i..].starts_with("=>") => {
                            if keyword == "type" && !return_type {
                                return_type = true;
                            } else {
                                valid = false;
                            }
                        }
                        _ => {}
                    }
                }
                match c {
                    b'(' | b'[' | b'{' => stack.push(c),
                    // Parameter expressions may contain comparison operators.
                    // Their enclosing brackets already protect the header;
                    // only outer type parameters need angle balancing here.
                    b'<' if stack.iter().all(|c| *c == b'<') => stack.push(c),
                    b')' | b']' | b'}' => {
                        let expected = match c {
                            b')' => b'(',
                            b']' => b'[',
                            _ => b'{',
                        };
                        valid &= stack.pop() == Some(expected);
                    }
                    b'>' if (i == 0 || header.as_bytes()[i - 1] != b'=')
                        && stack.last() == Some(&b'<') =>
                    {
                        stack.pop();
                    }
                    _ => {}
                }
                valid
            });
            (valid && stack.is_empty()).then(|| statement_start(text, start, lang))
        })
    })
}

fn statement_start(text: &str, start: usize, lang: Language) -> bool {
    match previous_code(text, start, lang) {
        None | Some((_, ';' | '{' | '}')) => true,
        Some((i, c)) => {
            if c == '/'
                && let Some(end) = super::js_code::regex_literal_end(text, i)
                && end <= start
            {
                let gap = &text[end..start];
                return trivia(gap, lang).is_empty()
                    && gap.contains(['\n', '\r', '\u{2028}', '\u{2029}']);
            }
            let word = previous_word(text, start, lang);
            if word.is_some_and(|(i, word)| {
                (matches!(word, "export" | "default" | "async")
                    || (lang == Language::TypeScript
                        && matches!(word, "declare" | "abstract" | "const")))
                    && statement_start(text, i, lang)
            }) {
                return true;
            }
            // A declaration can follow an expression terminated by ASI, but a
            // newline after `=`, `(`, `new`, etc. still belongs to an expression.
            // Keep function/class expressions' argument lists non-transparent.
            text[i + c.len_utf8()..start].contains(['\n', '\r', '\u{2028}', '\u{2029}'])
                && (matches!(c, ')' | ']' | '\'' | '"' | '`')
                    || postfix_end(text, i, c, lang)
                    || (is_ident(c)
                        && word.is_some_and(|(_, word)| {
                            !matches!(
                                word,
                                "new"
                                    | "typeof"
                                    | "void"
                                    | "delete"
                                    | "await"
                                    | "yield"
                                    | "throw"
                                    | "instanceof"
                                    | "in"
                                    | "of"
                                    | "extends"
                                    | "const"
                                    | "let"
                                    | "var"
                            )
                        })))
        }
    }
}

fn postfix_end(text: &str, end: usize, c: char, lang: Language) -> bool {
    matches!(c, '+' | '-')
        && text[..end].ends_with(c)
        && previous_code(text, end - 1, lang)
            .is_some_and(|(_, c)| is_ident(c) || matches!(c, ')' | ']'))
}

fn previous_code(text: &str, end: usize, lang: Language) -> Option<(usize, char)> {
    text[..end].char_indices().rev().find(|(i, c)| {
        !c.is_whitespace()
            && (super::lexical::reference_is_code(text, *i, lang)
                || (*c == '{' && *i > 0 && text.as_bytes()[*i - 1] == b'$'))
    })
}

fn assertion(tail: &str) -> Option<&str> {
    ["as", "satisfies"].into_iter().find_map(|word| {
        tail.strip_prefix(word).filter(|rest| {
            rest.chars().next().is_some_and(char::is_whitespace)
                || rest.starts_with("/*")
                || rest.starts_with("//")
        })
    })
}

fn generic_call(tail: &str, lang: Language) -> bool {
    if lang == Language::JavaScript || !tail.starts_with('<') {
        return false;
    }
    let mut stack = Vec::new();
    let mut end = None;
    walk_code(tail, 0, lang, |i, c| {
        match c {
            b'(' | b'[' | b'{' | b'<' => stack.push(c),
            b')' | b']' | b'}' => {
                stack.pop();
            }
            // Function-type arrows are not generic delimiters.
            b'>' if (i == 0 || tail.as_bytes()[i - 1] != b'=') && stack.last() == Some(&b'<') => {
                stack.pop();
                if stack.is_empty() {
                    end = Some(i);
                    return false;
                }
            }
            _ => {}
        }
        true
    });
    end.is_some_and(|end| {
        let rest = trivia(&tail[end + 1..], lang);
        rest.starts_with('(') || rest.starts_with('`')
    })
}

/// Find the end of the erased type, keeping its own nested function/tuple/object
/// types separate from closing expression parentheses. Delimiters stay unconsumed
/// so the caller can distinguish an invocation from a scalar read.
fn type_end(text: &str, lang: Language) -> usize {
    let mut stack = Vec::new();
    let mut end = text.len();
    walk_code(text, 0, lang, |i, c| {
        let Some(rest) = text.get(i..) else {
            return true;
        };
        let next_assertion = i > 0
            && !text[..i]
                .chars()
                .next_back()
                .is_some_and(crate::extract_delegate::common::is_ident)
            && assertion(rest).is_some();
        if stack.is_empty()
            && (matches!(c, b')' | b',' | b';' | b'`')
                || (c == b'=' && !rest.starts_with("=>"))
                || next_assertion)
        {
            end = i;
            return false;
        }
        match c {
            b'(' | b'[' | b'{' | b'<' => stack.push(c),
            b')' | b']' | b'}' => {
                stack.pop();
            }
            b'>' if i == 0 || text.as_bytes()[i - 1] != b'=' => {
                stack.pop();
            }
            _ => {}
        }
        true
    });
    end
}
