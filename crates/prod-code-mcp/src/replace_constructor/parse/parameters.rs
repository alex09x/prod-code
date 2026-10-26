/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::is_ident_str;
use crate::replace_constructor::types::FieldDecl;
use anyhow::{Context, Result};

/// Comments are trivia, but a masked literal still completes a field initializer.
/// Retain its end so an ASI field cannot expose its earlier `=` as method context.
fn previous_javascript_token_end(body: &str, mask: &[bool], end: usize) -> usize {
    let bytes = body.as_bytes();
    let mut at = 0;
    let mut previous_end = 0;
    while at < end {
        if mask[at] {
            if !bytes[at].is_ascii_whitespace() {
                previous_end = at + 1;
            }
            at += 1;
        } else if bytes[at..].starts_with(b"//") {
            at += bytes[at..end]
                .iter()
                .position(|&b| b == b'\n')
                .unwrap_or(end - at);
        } else if bytes[at..].starts_with(b"/*") {
            at += bytes[at..end]
                .windows(2)
                .position(|pair| pair == b"*/")
                .map_or(end - at, |offset| offset + 2);
        } else {
            while at < end && !mask[at] {
                at += 1;
            }
            previous_end = at;
        }
    }
    previous_end
}

fn javascript_word_start(body: &str, mask: &[bool], end: usize) -> usize {
    let mut start = end;
    let boundary = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$') || b >= 128;
    while start > 0 && mask[start - 1] && boundary(body.as_bytes()[start - 1]) {
        start -= 1;
    }
    start
}

/// Match a constructor method of this class, not a prefix, nested class, named
/// function expression or a call in a field initializer. Masked data has no depth.
fn javascript_constructor(body: &str, mask: &[bool]) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut depth = 0usize;
    let mut field_initializer = false;
    for (at, &byte) in bytes.iter().enumerate() {
        if !mask[at] {
            continue;
        }
        if depth == 0 && bytes[at..].starts_with(b"constructor") {
            let end = at + "constructor".len();
            let boundary =
                |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$') || b >= 128;
            if (at == 0 || !boundary(bytes[at - 1]))
                && !bytes.get(end).is_some_and(|&b| boundary(b))
            {
                let previous_end = previous_javascript_token_end(body, mask, at);
                let previous_start = javascript_word_start(body, mask, previous_end);
                let token = &body[previous_start..previous_end];
                let line_boundary =
                    body[previous_end..at].contains(['\n', '\r', '\u{2028}', '\u{2029}']);
                let before_word = previous_javascript_token_end(body, mask, previous_start);
                let access_end = if before_word.checked_sub(1).is_some_and(|i| bytes[i] == b'#') {
                    previous_javascript_token_end(body, mask, before_word - 1)
                } else {
                    before_word
                };
                // Keyword-named properties complete an initializer just like
                // other names. A real static modifier or function/new keyword
                // has no preceding access dot, even after a field's ASI break.
                let completed_property = field_initializer
                    && line_boundary
                    && access_end.checked_sub(1).is_some_and(|i| bytes[i] == b'.');
                // A named generator expression puts a star between `function`
                // and its name; the name is still inside the field expression.
                let generator_name = token.is_empty()
                    && previous_end
                        .checked_sub(1)
                        .is_some_and(|i| bytes[i] == b'*')
                    && {
                        let end = previous_javascript_token_end(body, mask, previous_end - 1);
                        let start = javascript_word_start(body, mask, end);
                        let before = previous_javascript_token_end(body, mask, start);
                        &body[start..end] == "function"
                            && !before.checked_sub(1).is_some_and(|i| bytes[i] == b'.')
                    };
                // A field initializer ending in get/set is complete before the
                // next line's constructor. Async also requires no line break
                // before a method name; static permits one and stays excluded.
                let contextual_modifier = match token {
                    "get" | "set" => !(field_initializer && line_boundary),
                    "async" => !line_boundary,
                    _ => false,
                };
                let excluded = previous_end
                    .checked_sub(1)
                    .is_some_and(|i| matches!(bytes[i], b'.' | b'=' | b'#'))
                    || (matches!(token, "function" | "new" | "static") && !completed_property)
                    || generator_name
                    || contextual_modifier;
                let next = |from: usize| {
                    (from..bytes.len()).find(|&i| mask[i] && !bytes[i].is_ascii_whitespace())
                };
                if !excluded
                    && let Some(open) = next(end)
                    && bytes[open] == b'('
                    && let Some(close) = super::matching_arguments(
                        body,
                        open,
                        crate::parameter_object::Language::JavaScript,
                    )
                    && next(close + 1).is_some_and(|i| bytes[i] == b'{')
                {
                    return Some(open);
                }
            }
        }
        match byte {
            b'=' if depth == 0 => field_initializer = true,
            b';' if depth == 0 => field_initializer = false,
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// Constructor parameters are the calling convention; class property order is not.
/// Keep rest/default markers on these non-Rust fields (which have no visibility).
pub(super) fn constructor_parameters(body: &str, python: bool) -> Result<Option<Vec<FieldDecl>>> {
    let keyword = if python {
        "def __init__"
    } else {
        "constructor"
    };
    let language = if python {
        crate::parameter_object::Language::Python
    } else {
        crate::parameter_object::Language::JavaScript
    };
    let mask = crate::caller_migration::lexical_code_mask(body, language);
    let open = if python {
        let indent = python_member_indent(body, &mask);
        let Some((at, _)) = body.match_indices(keyword).find(|(at, _)| {
            let start = body[..*at].rfind('\n').map_or(0, |i| i + 1);
            mask[*at] && body[start..*at].trim().is_empty() && Some(*at - start) == indent
        }) else {
            return Ok(None);
        };
        body[at..]
            .find('(')
            .map(|i| at + i)
            .context("constructor parameter list is missing")?
    } else {
        let Some(open) = javascript_constructor(body, &mask) else {
            return Ok(None);
        };
        open
    };
    let close = super::matching_arguments(body, open, language)
        .context("unclosed constructor parameters")?;
    let mut fields = Vec::new();
    let parameter_language = if python {
        language
    } else {
        crate::parameter_object::Language::TypeScript
    };
    for (_, raw) in crate::parameter_object::entries(&body[open + 1..close], parameter_language) {
        let mut parameter = raw.trim();
        if python && parameter == "self" {
            continue;
        }
        if !python {
            loop {
                let stripped = ["public ", "private ", "protected ", "readonly "]
                    .iter()
                    .find_map(|prefix| parameter.strip_prefix(prefix));
                match stripped {
                    Some(rest) => parameter = rest.trim_start(),
                    None => break,
                }
            }
        }
        let rest = !python && parameter.starts_with("...");
        parameter = parameter.trim_start_matches("...");
        let (head, default) = crate::parameter_object::split_default(parameter, language);
        let (name, ty) = head
            .split_once(':')
            .unwrap_or((head, if python { "Any" } else { "any" }));
        let optional = name.trim().ends_with('?');
        let name = name.trim().trim_end_matches('?');
        anyhow::ensure!(
            is_ident_str(name),
            "unsupported constructor parameter grammar `{raw}`; nothing was written"
        );
        anyhow::ensure!(
            !fields.iter().any(|f: &FieldDecl| f.vis == "rest"),
            "rest parameter must be last; nothing was written"
        );
        fields.push(FieldDecl {
            name: name.into(),
            ty: if python || head.contains(':') {
                ty.trim().into()
            } else {
                String::new()
            },
            vis: if rest {
                "rest"
            } else if default.is_some() {
                "default"
            } else if optional {
                "optional"
            } else {
                ""
            }
            .into(),
        });
    }
    Ok(Some(fields))
}

/// The first executable member establishes the selected class's suite level;
/// nested class/function constructors occur at deeper indentation.
pub(super) fn python_member_indent(body: &str, mask: &[bool]) -> Option<usize> {
    body.lines().find_map(|line| {
        if line.trim().is_empty() {
            return None;
        }
        let indent = line.len() - line.trim_start().len();
        let at = line.as_ptr() as usize - body.as_ptr() as usize + indent;
        mask.get(at).copied().unwrap_or(false).then_some(indent)
    })
}
