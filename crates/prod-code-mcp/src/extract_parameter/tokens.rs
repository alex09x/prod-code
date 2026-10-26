/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::Syntax;
use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub(super) struct Token<'a> {
    pub text: &'a str,
    pub start: usize,
    pub end: usize,
    pub literal: bool,
}

/// Lossless token boundaries; trivia is omitted, never searched for replacements.
pub(super) fn tokens(text: &str, syntax: Syntax) -> Result<Vec<Token<'_>>> {
    scan_tokens(text, syntax, 0)
}

/// Call spans omit their enclosing parentheses. Inside that implicit delimiter,
/// Python physical newlines are trivia rather than logical statement boundaries.
pub(super) fn argument_tokens(text: &str, syntax: Syntax) -> Result<Vec<Token<'_>>> {
    scan_tokens(text, syntax, 1)
}

fn scan_tokens(text: &str, syntax: Syntax, mut nesting: usize) -> Result<Vec<Token<'_>>> {
    scan_until(text, syntax, &mut nesting, false).map(|(tokens, _)| tokens)
}

pub(super) fn template_expression_end(text: &str) -> Result<usize> {
    scan_until(text, Syntax::JavaScript, &mut 0, true).map(|(_, end)| end)
}

fn scan_until<'s>(
    text: &'s str,
    syntax: Syntax,
    nesting: &mut usize,
    interpolation: bool,
) -> Result<(Vec<Token<'s>>, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        let c = rest.chars().next().unwrap();
        if interpolation && c == '}' && *nesting == 0 {
            return Ok((out, at + 1));
        }
        if syntax == Syntax::Python && (rest.starts_with("\\\n") || rest.starts_with("\\\r\n")) {
            at += if rest.starts_with("\\\r\n") { 3 } else { 2 };
            continue;
        }
        if c.is_whitespace() {
            // Python NEWLINE terminates a logical statement; only delimiter nesting
            // and an explicit backslash join physical lines into one expression.
            if syntax == Syntax::Python && c == '\n' && *nesting == 0 {
                out.push(Token {
                    text: &text[at..at + 1],
                    start: at,
                    end: at + 1,
                    literal: false,
                });
            }
            at += c.len_utf8();
            continue;
        }
        let start = at;
        let mut literal = false;
        let opaque = match syntax {
            Syntax::Cpp => super::literals::cpp_string_end(rest)?,
            Syntax::Python => super::literals::python_prefixed_end(rest)?,
            Syntax::JavaScript | Syntax::TypeScript => super::javascript::template_end(rest)?,
            _ => None,
        };
        if let Some(end) = opaque {
            at += end;
            literal = true;
        }
        if syntax == Syntax::Rust {
            // The signature scanner understands nested comments, raw/byte strings and lifetimes.
            let prefix = usize::from(
                rest.starts_with("br")
                    || rest.starts_with("cr")
                    || rest.starts_with("b\"")
                    || rest.starts_with("c\"")
                    || rest.starts_with("b'"),
            );
            if let Some((end, comment)) =
                crate::signature::call_sites::opaque_at(text, at + prefix, at)
                    .map_err(|_| anyhow::anyhow!("unclosed literal or comment"))?
            {
                at = end;
                if comment {
                    continue;
                }
                literal = true;
            }
        }
        if at == start {
            if (syntax == Syntax::Python && c == '#')
                || (syntax != Syntax::Python && rest.starts_with("//"))
            {
                at += if syntax.is_c_family() {
                    c_line_comment_end(rest)
                } else {
                    rest.find('\n').unwrap_or(rest.len())
                };
                continue;
            }
            if syntax != Syntax::Python && rest.starts_with("/*") {
                let mut depth = 1;
                at += 2;
                while at < text.len() && depth > 0 {
                    if text[at..].starts_with("/*")
                        && matches!(syntax, Syntax::Rust | Syntax::Swift)
                    {
                        depth += 1;
                        at += 2;
                    } else if text[at..].starts_with("*/") {
                        depth -= 1;
                        at += 2;
                    } else {
                        at += text[at..].chars().next().unwrap().len_utf8();
                    }
                }
                anyhow::ensure!(depth == 0, "unclosed comment");
                continue;
            }
            let regex = matches!(syntax, Syntax::JavaScript | Syntax::TypeScript)
                && c == '/'
                && !(interpolation && super::javascript::ends_with_initial_object(&out))
                && regex_context(&out)?;
            if regex {
                at += 1;
                let mut class = false;
                while at < text.len() {
                    match text.as_bytes()[at] {
                        b'\\' => {
                            at += 2;
                            continue;
                        }
                        b'[' => class = true,
                        b']' => class = false,
                        b'/' if !class => break,
                        _ => {}
                    }
                    at += 1;
                }
                anyhow::ensure!(at < text.len(), "unclosed regular expression");
                at += 1;
                while text.as_bytes().get(at).is_some_and(u8::is_ascii_alphabetic) {
                    at += 1;
                }
                literal = true;
            } else if matches!(c, '"' | '\'' | '`') && syntax != Syntax::Rust {
                let triple = syntax == Syntax::Python
                    && (rest.starts_with("\"\"\"") || rest.starts_with("'''"));
                let width = if triple { 3 } else { 1 };
                let quote = &rest[..width];
                at += width;
                while at < text.len() && !text[at..].starts_with(quote) {
                    at += if text.as_bytes()[at] == b'\\' && !(c == '`' && syntax == Syntax::Go) {
                        1
                    } else {
                        0
                    };
                    at += text
                        .get(at..)
                        .and_then(|s| s.chars().next())
                        .context("unclosed string")?
                        .len_utf8();
                }
                anyhow::ensure!(at < text.len(), "unclosed string");
                at += width;
                literal = true;
            } else if c.is_ascii_digit()
                || (c == '.' && rest.as_bytes().get(1).is_some_and(u8::is_ascii_digit))
            {
                at += 1;
                while at < text.len() {
                    let ch = text.as_bytes()[at];
                    if ch.is_ascii_alphanumeric()
                        || ch == b'_'
                        || (ch == b'.'
                            && text.as_bytes().get(at + 1).is_some_and(u8::is_ascii_digit))
                        || (matches!(ch, b'+' | b'-')
                            && matches!(text.as_bytes()[at - 1], b'e' | b'E' | b'p' | b'P'))
                    {
                        at += 1;
                    } else {
                        break;
                    }
                }
                literal = true;
            } else if ident(c) || (syntax == Syntax::Rust && rest.starts_with("r#")) {
                at += if rest.starts_with("r#") {
                    2
                } else {
                    c.len_utf8()
                };
                while let Some(ch) = text[at..]
                    .chars()
                    .next()
                    .filter(|c| ident(*c) || c.is_ascii_digit())
                {
                    at += ch.len_utf8();
                }
            } else if at == start {
                let op = [
                    "..=", "===", "!==", "<<=", ">>=", "::", "->", "=>", "==", "!=", "<=", ">=",
                    "&&", "||", "+=", "-=", "*=", "/=", ":=", "??", "?.", "<<", ">>", "**", "//",
                    "..", "++", "--", "&^", "<-",
                ]
                .into_iter()
                .find(|op| {
                    rest.starts_with(op)
                        && (!matches!(*op, "&^" | "<-") || syntax == Syntax::Go)
                        && (!matches!(*op, "++" | "--")
                            || syntax.is_c_family()
                            || matches!(
                                syntax,
                                Syntax::Go | Syntax::JavaScript | Syntax::TypeScript
                            ))
                });
                at += op.map_or(c.len_utf8(), str::len);
            }
        }
        if (syntax == Syntax::Python || interpolation) && !literal {
            match &text[start..at] {
                "{" => *nesting += 1,
                "}" => *nesting = nesting.saturating_sub(1),
                "(" | "[" if !interpolation => *nesting += 1,
                ")" | "]" if !interpolation => *nesting = nesting.saturating_sub(1),
                _ => {}
            }
        }
        out.push(Token {
            text: &text[start..at],
            start,
            end: at,
            literal,
        });
    }
    anyhow::ensure!(!interpolation, "unclosed template interpolation");
    Ok((out, at))
}

/// Slash uses the expression lexical goal after operators and at statement starts.
/// A closing control-condition parenthesis also starts a statement; an ordinary call
/// parenthesis ends a value. Brace endings need a full statement/object parser, so fail
/// before planning edits when that distinction is ambiguous.
fn regex_context(ts: &[Token<'_>]) -> Result<bool> {
    let Some(last) = ts.last() else {
        return Ok(true);
    };
    anyhow::ensure!(
        last.text != "}",
        "ambiguous JavaScript slash after a closing brace; cannot safely classify the expression"
    );
    // Reserved words are also legal property names: obj.return / divisor.
    if ts.len() > 1 && matches!(ts[ts.len() - 2].text, "." | "?." | "#") {
        return Ok(false);
    }
    if last.text == ")" {
        let mut depth = 0;
        for (i, t) in ts.iter().enumerate().rev() {
            match t.text {
                ")" => depth += 1,
                "(" => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(i > 0
                            && (i < 2 || !matches!(ts[i - 2].text, "." | "?." | "#"))
                            && matches!(
                                ts[i - 1].text,
                                "if" | "while" | "for" | "with" | "switch" | "catch"
                            ));
                    }
                }
                _ => {}
            }
        }
        anyhow::bail!("unmatched JavaScript parenthesis before slash");
    }
    Ok(matches!(
        last.text,
        ";" | "{"
            | "("
            | "["
            | ","
            | ":"
            | "?"
            | "="
            | "=>"
            | "return"
            | "throw"
            | "yield"
            | "await"
            | "else"
            | "do"
            | "delete"
            | "void"
            | "typeof"
            | "new"
            | "in"
            | "instanceof"
            | "break"
            | "continue"
            | "debugger"
            | "!"
            | "~"
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "**"
            | "&&"
            | "||"
            | "??"
            | "&"
            | "|"
            | "^"
            | "=="
            | "!="
            | "==="
            | "!=="
            | "<"
            | ">"
            | "<="
            | ">="
            | "<<"
            | ">>"
            | "+="
            | "-="
            | "*="
            | "/="
            | "<<="
            | ">>="
    ))
}

/// Where a C or C++ `//` comment at the start of `rest` ends: at the first line break that a
/// backslash does not splice to the next line (LF or CRLF), since splicing happens before
/// comments are recognised (#1408).
fn c_line_comment_end(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    rest.match_indices('\n')
        .map(|(at, _)| at)
        .find(|&at| {
            let before = if at > 0 && bytes[at - 1] == b'\r' {
                at - 1
            } else {
                at
            };
            before == 0 || bytes[before - 1] != b'\\'
        })
        .unwrap_or(rest.len())
}

pub(super) fn ident(c: char) -> bool {
    c == '_' || c == '$' || unicode_ident::is_xid_continue(c)
}

pub(super) fn pairs(tokens: &[Token<'_>]) -> Result<Vec<Option<usize>>> {
    let mut pairs = vec![None; tokens.len()];
    let mut stack = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if t.literal {
            continue;
        }
        match t.text {
            "(" | "[" | "{" => stack.push(i),
            ")" | "]" | "}" => {
                let open = stack.pop().context("unbalanced declaration delimiters")?;
                anyhow::ensure!(
                    matches!(
                        (tokens[open].text, t.text),
                        ("(", ")") | ("[", "]") | ("{", "}")
                    ),
                    "unbalanced declaration delimiters"
                );
                pairs[open] = Some(i);
                pairs[i] = Some(open);
            }
            _ => {}
        }
    }
    anyhow::ensure!(stack.is_empty(), "unbalanced declaration delimiters");
    Ok(pairs)
}
