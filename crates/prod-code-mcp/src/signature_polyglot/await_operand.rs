/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::await_regex::{can_start as regex_can_start, end as regex_end};
use crate::parameter_object::Language;

pub(super) struct Token<'a> {
    pub text: &'a str,
    pub start: usize,
    pub end: usize,
    pub pair: Option<usize>,
    pub expression_container: bool,
}

/// Token positions retain trivia in the source. Remove only the await keyword;
/// comments and whitespace between it and the operand belong to the user.
pub fn edits(
    content: &str,
    name: usize,
    args_start: usize,
    call_end: usize,
    lang: Language,
    enable: bool,
    jsx: bool,
) -> Option<Vec<(usize, usize, String)>> {
    let tokens = tokens(content, lang, args_start, call_end - 1, jsx)?;
    let name_index = tokens.iter().position(|t| t.start == name)?;
    let mut start = primary_start(&tokens, name_index)?;
    let mut end = tokens.iter().position(|t| t.end == call_end)?;
    let receiver_start = tokens[start].start;
    // Expand only grouping parentheses, never the argument list of another call.
    while start > 0 && tokens[start - 1].text == "(" && tokens[start - 1].pair == Some(end + 1) {
        if start > 1 && can_end_primary(&tokens[start - 2]) {
            break;
        }
        start -= 1;
        end += 1;
    }
    let chained = tokens
        .get(end + 1)
        .is_some_and(|t| matches!(t.text, "." | "[" | "(" | "?"));
    let awaited = start.checked_sub(1).filter(|i| tokens[*i].text == "await");
    if !enable {
        return Some(match awaited.filter(|_| !chained) {
            Some(index) => {
                let end = tokens[index].end;
                // Consume the keyword's ordinary separator, retaining all comments.
                let end = if content[end..].starts_with(' ') {
                    end + 1
                } else {
                    end
                };
                vec![(tokens[index].start, end, String::new())]
            }
            None => Vec::new(),
        });
    }
    if awaited.is_some() && !chained {
        return Some(Vec::new());
    }
    // Put await before the complete receiver; wrapping also preserves chaining.
    Some(vec![
        (receiver_start, receiver_start, "(await ".into()),
        (call_end, call_end, ")".into()),
    ])
}

pub(super) fn can_end_primary(token: &Token<'_>) -> bool {
    matches!(token.text, ")" | "]" | "`" | "jsx")
        || (token
            .text
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '\'' | '"'))
            && !matches!(
                token.text,
                "await"
                    | "return"
                    | "yield"
                    | "throw"
                    | "not"
                    | "and"
                    | "or"
                    | "if"
                    | "else"
                    | "in"
            ))
}

fn primary_start(tokens: &[Token<'_>], end: usize) -> Option<usize> {
    let mut start = if matches!(tokens[end].text, ")" | "]" | "`") {
        let open = tokens[end].pair?;
        if open > 0 && can_end_primary(&tokens[open - 1]) {
            primary_start(tokens, open - 1)?
        } else {
            open
        }
    } else if can_end_primary(&tokens[end]) {
        end
    } else {
        return None;
    };
    while start >= 2 && tokens[start - 1].text == "." {
        start = primary_start(tokens, start - 2)?;
    }
    Some(start)
}

pub(super) fn tokens(
    content: &str,
    lang: Language,
    args_start: usize,
    args_end: usize,
    jsx: bool,
) -> Option<Vec<Token<'_>>> {
    tokenize(content, lang, args_start, args_end, jsx, None)
}

/// Semantic references already identify the callee. Resolve only its argument
/// group, so unrelated statements need not pass the await operand lexer.
pub(super) fn call_tokens(
    content: &str,
    after_name: usize,
    lang: Language,
    jsx: bool,
) -> Option<Vec<Token<'_>>> {
    tokenize(content, lang, usize::MAX, usize::MAX, jsx, Some(after_name))
}

fn tokenize(
    content: &str,
    lang: Language,
    args_start: usize,
    args_end: usize,
    jsx: bool,
    call_start: Option<usize>,
) -> Option<Vec<Token<'_>>> {
    let bytes = content.as_bytes();
    let is_ident = |c| {
        super::syntax::is_ident(c)
            || (matches!(lang, Language::JavaScript | Language::TypeScript) && c == '$')
    };
    let mut result: Vec<Token<'_>> = Vec::new();
    let mut stack = Vec::new();
    let mut jsx_context = super::await_jsx::Context::default();
    let mut templates = super::await_template::Context::default();
    let mut at = call_start.unwrap_or(0);
    while at < bytes.len() {
        let mut expression_container = false;
        // All consumers share the argument boundary; skip before entering JSX
        // so markup in an already resolved operand cannot change that span.
        if at == args_start && at < args_end {
            at = args_end;
            continue;
        }
        if let Some(step) = templates.skip(content, at, stack.len()) {
            match step {
                super::await_template::Step::Text(end, opening) => {
                    // A template's final fragment belongs to the complete
                    // literal (and, for a tagged template, to its callee).
                    if let Some(opening) = opening {
                        result[opening].pair = Some(result.len());
                    }
                    result.push(Token {
                        text: "`",
                        start: at,
                        end,
                        pair: opening,
                        expression_container: false,
                    });
                    at = end;
                    continue;
                }
                super::await_template::Step::Expression(brace) => {
                    at = brace;
                    expression_container = true;
                }
            }
        }
        if jsx {
            if let Some(end) = jsx_context.skip(content, at, stack.len()) {
                result.push(Token {
                    text: "jsx",
                    start: at,
                    end,
                    pair: None,
                    expression_container: false,
                });
                at = end;
                continue;
            }
            expression_container |= jsx_context.expression_start(stack.len());
            if bytes[at] == b'<'
                && regex_can_start(content, &result, lang)
                && jsx_context.start(&content[at..])
            {
                continue;
            }
        }
        // Only physical newlines outside grouping terminate Python statements.
        // Explicit continuations are trivia even at grouping depth zero.
        if lang == Language::Python && bytes[at] == b'\\' {
            if content[at + 1..].starts_with("\r\n") {
                at += 3;
                continue;
            }
            if bytes.get(at + 1) == Some(&b'\n') {
                at += 2;
                continue;
            }
        }
        if lang == Language::Python && matches!(bytes[at], b'\n' | b'\r') && stack.is_empty() {
            result.push(Token {
                text: "\n",
                start: at,
                end: at + 1,
                pair: None,
                expression_container: false,
            });
        }
        if bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        let rest = &content[at..];
        if (lang == Language::Python && rest.starts_with('#'))
            || (lang != Language::Python && rest.starts_with("//"))
        {
            at += rest.find('\n').unwrap_or(rest.len());
            continue;
        }
        if lang != Language::Python && rest.starts_with("/*") {
            at += 2 + rest[2..].find("*/")? + 2;
            continue;
        }
        let start = at;
        if bytes[at] == b'/'
            && matches!(lang, Language::JavaScript | Language::TypeScript)
            && regex_can_start(content, &result, lang)
        {
            // Regex character classes and escaped slashes are literal data, not
            // grouping delimiters. In particular /[)]/ must not unbalance a file.
            at = regex_end(bytes, at)?;
        } else if bytes[at] == b'`' && matches!(lang, Language::JavaScript | Language::TypeScript) {
            templates.start(result.len());
            at += 1;
        } else if matches!(bytes[at], b'\'' | b'"' | b'`') {
            let quote = bytes[at];
            let triple = lang == Language::Python && bytes.get(at..at + 3) == Some(&[quote; 3]);
            at += if triple { 3 } else { 1 };
            loop {
                let byte = *bytes.get(at)?;
                if byte == b'\\' {
                    at += 2;
                } else if byte == quote && (!triple || bytes.get(at..at + 3) == Some(&[quote; 3])) {
                    at += if triple { 3 } else { 1 };
                    break;
                } else {
                    at += 1;
                }
            }
        } else if rest.starts_with("++") || rest.starts_with("--") {
            at += 2;
        } else if content[at..].starts_with(is_ident) {
            while at < bytes.len() && content[at..].starts_with(is_ident) {
                at += content[at..].chars().next()?.len_utf8();
            }
        } else {
            at += content[at..].chars().next()?.len_utf8();
        }
        let text = &content[start..at];
        if call_start.is_some() && result.is_empty() && text != "(" {
            return None;
        }
        let mut pair = None;
        if matches!(text, ")" | "]" | "}") {
            let open: usize = stack.pop()?;
            if !matches!(
                (result[open].text, text),
                ("(", ")") | ("[", "]") | ("{", "}")
            ) {
                return None;
            }
            result[open].pair = Some(result.len());
            pair = Some(open);
        }
        if matches!(text, "(" | "[" | "{") {
            stack.push(result.len());
        }
        result.push(Token {
            text,
            start,
            end: at,
            pair,
            expression_container,
        });
        if call_start.is_some() && stack.is_empty() {
            return Some(result);
        }
    }
    (call_start.is_none() && stack.is_empty() && templates.is_empty()).then_some(result)
}
