/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::binding::{mask, parameter_close, preserve};
use crate::make_static::Language;
use anyhow::{Context, Result, ensure};

pub(super) fn braced(
    code: &str,
    line: usize,
    declaration: &str,
    name: &str,
    first: &str,
    lang: Language,
) -> Result<(String, usize)> {
    ensure!(
        lang != Language::Cpp || !super::cpp_lex::has_raw_string(code),
        "unsupported C++ raw string receiver body"
    );
    let start: usize = code.split_inclusive('\n').take(line).map(str::len).sum();
    let line_end = code[start..].find('\n').map_or(code.len(), |n| start + n);
    let carriage = if code[..line_end].ends_with('\r') && !declaration.ends_with('\r') {
        "\r"
    } else {
        ""
    };
    let mut out = format!(
        "{}{declaration}{carriage}{}",
        &code[..start],
        &code[line_end..]
    );
    let code_mask = mask(&out, lang);
    // Parameter defaults can contain braces, as can expressions in noexcept or
    // decltype suffixes. Only a brace outside the complete signature opens the body.
    let parameters_end = start + parameter_close(&out[start..], lang)?;
    let mut suffix_depth = Vec::new();
    let mut open = if lang == Language::TypeScript {
        Some(super::ts_return::body_open(&out, parameters_end + 1)?)
    } else {
        None
    };
    for (i, byte) in out.bytes().enumerate().skip(parameters_end + 1) {
        if open.is_some() {
            break;
        }
        if !code_mask[i] {
            continue;
        }
        match byte {
            b'(' => suffix_depth.push(b')'),
            b'[' => suffix_depth.push(b']'),
            b')' | b']' => ensure!(
                suffix_depth.pop() == Some(byte),
                "unsupported receiver signature delimiter"
            ),
            b'{' if suffix_depth.is_empty() => {
                open = Some(i);
                break;
            }
            b';' | b'}' if suffix_depth.is_empty() => {
                anyhow::bail!("unsupported receiver signature without a body")
            }
            _ => {}
        }
    }
    let open = open.context("unsupported receiver body: missing opening brace")?;
    let suffix = &out[parameters_end + 1..open];
    if lang == Language::Cpp {
        // The removed parameter is in scope in trailing return/noexcept queries;
        // an alias inside the body cannot repair those references. Member names
        // after . / -> / :: refer to other objects or types, not the parameter.
        let syntax = mask(suffix, lang);
        ensure!(
            !super::binding::words(suffix, lang)
                .iter()
                .any(|&(at, word)| {
                    let before: String = suffix[..at]
                        .char_indices()
                        .filter(|(i, ch)| syntax[*i] && !ch.is_whitespace())
                        .map(|(_, ch)| ch)
                        .collect();
                    word == name
                        && !before.ends_with('.')
                        && !before.ends_with("->")
                        && !before.ends_with("::")
                }),
            "unsupported C++ signature suffix referencing the removed receiver parameter"
        );
    }
    if lang == Language::TypeScript {
        let view = super::ts_queries::analysis_view(suffix);
        let suffix = view.as_str();
        let words = super::binding::words(suffix, lang);
        let target = words.first().map(|(_, word)| *word);
        let next = words.get(1).map(|(_, word)| *word);
        let suffix_mask = mask(suffix, lang);
        let adjacent = words.get(1).is_some_and(|(second, _)| {
            let (first, word) = words[0];
            (first + word.len()..*second)
                .all(|i| !suffix_mask[i] || suffix.as_bytes()[i].is_ascii_whitespace())
        });
        ensure!(
            !(adjacent
                && ((target == Some(name) && next == Some("is"))
                    || (target == Some("asserts") && next == Some(name)))),
            "unsupported TypeScript predicate on the removed receiver parameter"
        );
        // A body-local alias cannot bind a value query in the result type.
        // Nested callable parameters, however, retain their own value scope.
        ensure!(
            !super::ts_queries::depends_on_receiver(suffix, name),
            "unsupported TypeScript result type query on the removed receiver parameter"
        );
    }
    ensure!(
        lang != Language::Cpp
            || !super::binding::words(suffix, lang)
                .last()
                .is_some_and(|(_, word)| *word == "try"),
        "unsupported C++ function-try-block receiver: handler scopes are not preserved"
    );
    let mut depth = 1;
    let mut close = None;
    for (i, byte) in out.bytes().enumerate().skip(open + 1) {
        if code_mask[i] {
            if byte == b'{' {
                depth += 1;
            }
            if byte == b'}' {
                depth -= 1;
            }
            if depth == 0 {
                close = Some(i);
                break;
            }
        }
    }
    let close = close.context("unsupported receiver body: unbalanced braces")?;
    let swift_returns_void = lang == Language::Swift
        && super::swift::returns_void(suffix, code, start, &out[open + 1..close]);
    let (body, count) = preserve(&out[open + 1..close], name, first, lang, swift_returns_void)?;
    out.replace_range(open + 1..close, &body);
    Ok((out, count))
}

pub(super) fn python(
    code: &str,
    line: usize,
    declaration: &str,
    name: &str,
    first: &str,
    end: usize,
) -> Result<(String, usize)> {
    let mut lines: Vec<String> = code.lines().map(str::to_string).collect();
    lines[line] = declaration.to_string();
    let close = parameter_close(declaration, Language::Python)?;
    let syntax = mask(declaration, Language::Python);
    let colon = (close..declaration.len())
        .find(|i| syntax[*i] && declaration.as_bytes()[*i] == b':')
        .context("missing Python body delimiter")?;
    let tail = declaration[colon + 1..].trim();
    let inline = !tail.is_empty() && !tail.starts_with('#');
    let body = if inline {
        declaration[colon + 1..].to_string()
    } else {
        lines[line + 1..=end].join("\n")
    };
    complete_python_docstring(&body)?;
    let (replacement, count) = preserve(&body, name, first, Language::Python, false)?;
    if replacement
        .trim_start()
        .starts_with(&format!("{name} = self"))
    {
        if inline {
            // Python accepts raw and Unicode prefixes on docstrings. Keep their
            // entire literal statement first, just like an unprefixed docstring.
            let trimmed = body.trim_start();
            let trimmed = trimmed
                .strip_prefix(['r', 'R', 'u', 'U'])
                .unwrap_or(trimmed);
            if trimmed.starts_with(['\'', '"']) {
                let code_mask = mask(&body, Language::Python);
                let separator = body
                    .char_indices()
                    .find(|(i, c)| code_mask[*i] && !c.is_whitespace());
                let at = match separator {
                    Some((i, ';')) => i + 1,
                    None => body.len(),
                    _ => anyhow::bail!("unsupported inline Python docstring binding"),
                };
                let separator = if at == body.len() { ";" } else { "" };
                lines[line] = format!(
                    "{}{}{separator} {name} = self;{}",
                    &declaration[..=colon],
                    &body[..at],
                    &body[at..]
                );
            } else {
                lines[line] = format!("{} {name} = self;{}", &declaration[..=colon], body);
            }
        } else {
            let first_line = body
                .lines()
                .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .context("unsupported empty Python body")?;
            let indent = first_line[..first_line.len() - first_line.trim_start().len()].to_string();
            // Keep a leading docstring as the first statement. The existing lexer masks
            // complete string literals (including triple-quoted strings).
            let code_mask = mask(&body, Language::Python);
            let first_code = body
                .char_indices()
                .find(|(i, c)| code_mask[*i] && !c.is_whitespace())
                .map(|(i, _)| i)
                .unwrap_or(body.len());
            let mut body = body;
            if body.as_bytes().get(first_code) == Some(&b';') {
                // A multiline docstring may end on the same line as another
                // statement. Its line start is still inside the string literal.
                body.insert_str(first_code + 1, &format!(" {name} = self;"));
            } else {
                let insert = if first_code == body.len() {
                    body.len()
                } else {
                    body[..first_code].rfind('\n').map_or(0, |n| n + 1)
                };
                ensure!(
                    body[insert..first_code].trim().is_empty(),
                    "unsupported Python docstring statement boundary"
                );
                let separator = if insert == body.len() { "\n" } else { "" };
                body.insert_str(insert, &format!("{separator}{indent}{name} = self\n"));
            }
            lines.splice(line + 1..=end, body.lines().map(str::to_string));
        }
    } else if inline {
        lines[line] = format!("{}{}", &declaration[..=colon], replacement);
    } else {
        lines.splice(line + 1..=end, replacement.lines().map(str::to_string));
    }
    if line > 0 && lines[line - 1].trim() == "@staticmethod" {
        lines.remove(line - 1);
    }
    let mut out = lines.join("\n");
    if code.ends_with('\n') {
        out.push('\n');
    }
    Ok((out, count))
}

// A masked, unterminated leading literal must never receive an alias in its text.
fn complete_python_docstring(body: &str) -> Result<()> {
    let mut text = body.trim_start();
    while text.starts_with('#') {
        text = text
            .split_once('\n')
            .map_or("", |(_, rest)| rest.trim_start());
    }
    let text = text.strip_prefix(['r', 'R', 'u', 'U']).unwrap_or(text);
    let quote = if text.starts_with("\"\"\"") {
        "\"\"\""
    } else if text.starts_with("'''") {
        "'''"
    } else {
        return Ok(());
    };
    let mut i = 3;
    while i < text.len() {
        if text.as_bytes()[i] == b'\\' {
            i += 2;
        } else if text.as_bytes()[i..].starts_with(quote.as_bytes()) {
            return Ok(());
        } else {
            i += 1;
        }
    }
    anyhow::bail!("unsupported unterminated Python docstring receiver body")
}
