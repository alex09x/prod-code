/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;

/// Computes a boolean mask indicating executable code bytes (`true`) versus comments, string
/// literal text, and interpolation delimiters (`false`). Executable expressions inside JS/TS
/// template literals (`${...}`), Python f-strings (`f"...{...}..."`), and Swift string
/// interpolations (`"\(...)"`) remain `true`.
pub fn lexical_code_mask(text: &str, lang: Language) -> Vec<bool> {
    let mut code = vec![false; text.len()];
    scan_code_into_mask(text, 0, text.len(), lang, None, &mut code);
    code
}

/// Scans a `{ ... }` template expression starting at `open_brace` (`text.as_bytes()[open_brace] == b'{'`),
/// marking executable TypeScript/JavaScript bytes inside `mask` as `true` and returning the byte
/// index immediately after the matching closing `}` (or `open_brace + 1` if unclosed).
pub(crate) fn scan_ts_template_expr(text: &str, open_brace: usize, mask: &mut [bool]) -> usize {
    if let Some(close) = scan_code_into_mask(
        text,
        open_brace + 1,
        text.len(),
        Language::TypeScript,
        Some(b'}'),
        mask,
    ) {
        if text.as_bytes().get(open_brace + 1) == Some(&b'{')
            && close > open_brace + 1
            && text.as_bytes().get(close - 1) == Some(&b'}')
        {
            mask[open_brace + 1] = false;
            mask[close - 1] = false;
        }
        close + 1
    } else {
        if open_brace + 1 < mask.len() {
            mask[open_brace + 1..].fill(false);
        }
        open_brace + 1
    }
}

fn scan_code_into_mask(
    text: &str,
    start: usize,
    end: usize,
    lang: Language,
    stop_at: Option<u8>,
    code: &mut [bool],
) -> Option<usize> {
    let bytes = text.as_bytes();
    let end = end.min(bytes.len());
    let mut i = start;
    let mut brace_depth = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;

    while i < end {
        if lang == Language::Python && bytes[i] == b'#' {
            i = text[i..end].find('\n').map_or(end, |n| i + n);
            continue;
        }

        if lang != Language::Python && bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            i = text[i..end].find('\n').map_or(end, |n| i + n);
            continue;
        }

        if lang != Language::Python && bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let mut j = i + 2;
            let mut depth = 1usize;
            while j < end && depth > 0 {
                if bytes[j] == b'/' && bytes.get(j + 1) == Some(&b'*') {
                    depth += 1;
                    j += 2;
                } else if bytes[j] == b'*' && bytes.get(j + 1) == Some(&b'/') {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            i = j;
            continue;
        }

        if lang == Language::Python && (bytes[i] == b'"' || bytes[i] == b'\'') {
            i = scan_python_string(text, start, i, end, code);
            continue;
        }

        if matches!(lang, Language::TypeScript | Language::JavaScript) && bytes[i] == b'`' {
            i = scan_ts_template_literal(text, i, end, lang, code);
            continue;
        }

        let is_quote = if matches!(lang, Language::TypeScript | Language::JavaScript) {
            bytes[i] == b'"' || bytes[i] == b'\''
        } else if lang == Language::Go {
            bytes[i] == b'"' || bytes[i] == b'`'
        } else {
            bytes[i] == b'"'
        };

        if is_quote {
            let quote = bytes[i];
            let escapes = !(quote == b'`' && lang == Language::Go);
            let mut j = i + 1;
            while j < end {
                if escapes && bytes[j] == b'\\' {
                    if lang == Language::Swift && bytes.get(j + 1) == Some(&b'(') {
                        if let Some(close) =
                            scan_code_into_mask(text, j + 2, end, Language::Swift, Some(b')'), code)
                        {
                            j = close + 1;
                            continue;
                        }
                        j = end;
                        break;
                    }
                    j = (j + 2).min(end);
                } else if bytes[j] == quote {
                    j += 1;
                    break;
                } else {
                    j += 1;
                }
            }
            i = j;
            continue;
        }

        if let Some(stop) = stop_at {
            if lang == Language::Python
                && stop == b'}'
                && brace_depth == 0
                && paren_depth == 0
                && bracket_depth == 0
                && ((bytes[i] == b'!' && bytes.get(i + 1) != Some(&b'='))
                    || (bytes[i] == b':' && bytes.get(i + 1) != Some(&b'=')))
            {
                return scan_python_format_spec(text, i + 1, end, code);
            }
            match bytes[i] {
                b'{' => brace_depth += 1,
                b'}' => {
                    if stop == b'}' && brace_depth == 0 {
                        return Some(i);
                    }
                    brace_depth = brace_depth.saturating_sub(1);
                }
                b'(' => paren_depth += 1,
                b')' => {
                    if stop == b')' && paren_depth == 0 {
                        return Some(i);
                    }
                    paren_depth = paren_depth.saturating_sub(1);
                }
                b'[' => bracket_depth += 1,
                b']' => bracket_depth = bracket_depth.saturating_sub(1),
                _ => {}
            }
        }

        code[i] = true;
        i += 1;
    }

    None
}

fn scan_ts_template_literal(
    text: &str,
    i: usize,
    end: usize,
    lang: Language,
    code: &mut [bool],
) -> usize {
    let bytes = text.as_bytes();
    let mut j = i + 1;
    while j < end {
        if bytes[j] == b'\\' {
            j = (j + 2).min(end);
        } else if bytes[j] == b'`' {
            return j + 1;
        } else if bytes[j] == b'$' && bytes.get(j + 1) == Some(&b'{') {
            if let Some(close) = scan_code_into_mask(text, j + 2, end, lang, Some(b'}'), code) {
                j = close + 1;
            } else {
                return end;
            }
        } else {
            j += 1;
        }
    }
    end
}

fn scan_python_string(text: &str, start: usize, i: usize, end: usize, code: &mut [bool]) -> usize {
    let bytes = text.as_bytes();
    let mut p = i;
    while p > start && bytes[p - 1].is_ascii_alphabetic() {
        p -= 1;
    }
    let valid_prefix = (p == start
        || !(bytes[p - 1].is_ascii_alphanumeric() || bytes[p - 1] == b'_'))
        && (1..=2).contains(&(i - p))
        && bytes[p..i]
            .iter()
            .all(|b| matches!(b.to_ascii_lowercase(), b'f' | b'r' | b'b' | b'u'));
    let is_fstring = valid_prefix && bytes[p..i].iter().any(|b| matches!(*b, b'f' | b'F'));
    if valid_prefix {
        code[p..i].fill(false);
    }

    let q = bytes[i];
    let delim_len = if bytes[i..end].starts_with(if q == b'"' { b"\"\"\"" } else { b"'''" }) {
        3
    } else {
        1
    };
    let delim = &bytes[i..i + delim_len];
    let mut j = i + delim_len;
    while j < end {
        if bytes[j..end].starts_with(delim) {
            return j + delim_len;
        } else if bytes[j] == b'\\' {
            j = (j + 2).min(end);
        } else if is_fstring && bytes[j] == b'{' {
            if bytes.get(j + 1) == Some(&b'{') {
                j += 2;
            } else if let Some(close) =
                scan_code_into_mask(text, j + 1, end, Language::Python, Some(b'}'), code)
            {
                j = close + 1;
            } else {
                return end;
            }
        } else if is_fstring && bytes[j] == b'}' && bytes.get(j + 1) == Some(&b'}') {
            j += 2;
        } else {
            j += 1;
        }
    }
    end
}

fn scan_python_format_spec(
    text: &str,
    mut j: usize,
    end: usize,
    code: &mut [bool],
) -> Option<usize> {
    let bytes = text.as_bytes();
    while j < end {
        if bytes[j] == b'}' {
            return Some(j);
        } else if bytes[j] == b'{' {
            j = scan_code_into_mask(text, j + 1, end, Language::Python, Some(b'}'), code)? + 1;
        } else {
            j += 1;
        }
    }
    None
}
