/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::is_ident;

/// Bytes that belong to Rust structure rather than comments or literals. The extraction parser
/// only needs delimiters and identifiers, but those may appear harmlessly inside all of Rust's
/// comment and string forms.
pub fn lexical_code(text: &str) -> Vec<bool> {
    let bytes = text.as_bytes();
    let mut code = vec![true; bytes.len()];
    let mut i = 0usize;
    while i < bytes.len() {
        let end = if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            text[i..].find('\n').map_or(bytes.len(), |n| i + n)
        } else if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let mut depth = 1usize;
            let mut j = i + 2;
            while j < bytes.len() && depth > 0 {
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
            j
        } else if let Some(end) = raw_string_end(bytes, i) {
            end
        } else {
            let quote = match (bytes[i], bytes.get(i + 1)) {
                (b'b' | b'c', Some(b'"')) => i + 1,
                (b'"', _) => i,
                _ => {
                    if bytes[i] == b'b' && bytes.get(i + 1) == Some(&b'\'') {
                        if let Some(end) = char_literal_end(text, i + 1) {
                            code[i..end].fill(false);
                            i = end;
                            continue;
                        }
                    } else if bytes[i] == b'\''
                        && let Some(end) = char_literal_end(text, i)
                    {
                        code[i..end].fill(false);
                        i = end;
                        continue;
                    }
                    i += 1;
                    continue;
                }
            };
            let mut j = quote + 1;
            while j < bytes.len() {
                if bytes[j] == b'\\' {
                    j = (j + 2).min(bytes.len());
                } else if bytes[j] == b'"' {
                    j += 1;
                    break;
                } else {
                    j += 1;
                }
            }
            j
        };
        code[i..end].fill(false);
        i = end;
    }
    code
}

pub fn raw_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let r = match (bytes[start], bytes.get(start + 1)) {
        (b'r', _) => start,
        (b'b' | b'c', Some(b'r')) => start + 1,
        _ => return None,
    };
    let mut quote = r + 1;
    while bytes.get(quote) == Some(&b'#') {
        quote += 1;
    }
    if bytes.get(quote) != Some(&b'"') {
        return None;
    }
    let hashes = quote - r - 1;
    let mut i = quote + 1;
    while i < bytes.len() {
        if bytes[i] == b'"'
            && bytes
                .get(i + 1..i + 1 + hashes)
                .is_some_and(|tail| tail.iter().all(|b| *b == b'#'))
        {
            return Some(i + 1 + hashes);
        }
        i += 1;
    }
    Some(bytes.len())
}

pub fn char_literal_end(text: &str, quote: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = quote + 1;
    if bytes.get(i) == Some(&b'\\') {
        i += 1;
        match bytes.get(i)? {
            b'x' => i += 3,
            b'u' if bytes.get(i + 1) == Some(&b'{') => {
                i += 2;
                i += bytes.get(i..)?.iter().position(|b| *b == b'}')? + 1;
            }
            _ => i += 1,
        }
    } else {
        i += text.get(i..)?.chars().next()?.len_utf8();
    }
    (bytes.get(i) == Some(&b'\'')).then_some(i + 1)
}

pub fn previous_code(text: &str, code: &[bool], before: usize) -> Option<usize> {
    (0..before)
        .rev()
        .find(|i| code[*i] && !text.as_bytes()[*i].is_ascii_whitespace())
}

pub fn next_code(text: &str, code: &[bool], from: usize) -> Option<usize> {
    (from..text.len()).find(|i| code[*i] && !text.as_bytes()[*i].is_ascii_whitespace())
}

pub fn matching_open_square(text: &str, code: &[bool], close: usize) -> Option<usize> {
    let mut depth = 0usize;
    for i in (0..=close).rev() {
        if !code[i] {
            continue;
        }
        match text.as_bytes()[i] {
            b']' => depth += 1,
            b'[' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn matching_close_brace(text: &str, open: usize) -> Option<usize> {
    let code = lexical_code(text);
    let mut depth = 0usize;
    for (i, is_code) in code.iter().enumerate().skip(open) {
        if !*is_code {
            continue;
        }
        match text.as_bytes()[i] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn has_outer_attribute_before(text: &str, at: usize) -> bool {
    let code = lexical_code(text);
    let Some(close) = previous_code(text, &code, at).filter(|i| text.as_bytes()[*i] == b']') else {
        return false;
    };
    let Some(open) = matching_open_square(text, &code, close) else {
        return false;
    };
    previous_code(text, &code, open).is_some_and(|i| text.as_bytes()[i] == b'#')
}

pub fn attribute_end(text: &str) -> Option<usize> {
    let code = lexical_code(text);
    let hash = next_code(text, &code, 0).filter(|i| text.as_bytes()[*i] == b'#')?;
    let open = next_code(text, &code, hash + 1).filter(|i| text.as_bytes()[*i] == b'[')?;
    let mut depth = 0usize;
    for (i, is_code) in code.iter().enumerate().skip(open) {
        if !*is_code {
            continue;
        }
        match text.as_bytes()[i] {
            b'[' => depth += 1,
            b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn attribute_name(attribute: &str) -> Option<&str> {
    let code = lexical_code(attribute);
    let hash = next_code(attribute, &code, 0)?;
    let open = next_code(attribute, &code, hash + 1)?;
    let start = next_code(attribute, &code, open + 1)?;
    let end = attribute[start..]
        .char_indices()
        .take_while(|(offset, c)| code[start + offset] && is_ident(*c))
        .last()
        .map_or(start, |(offset, c)| start + offset + c.len_utf8());
    (end > start).then_some(&attribute[start..end])
}

pub fn contains_code_word(text: &str, wanted: &str) -> bool {
    let code = lexical_code(text);
    text.match_indices(wanted).any(|(i, _)| {
        code[i..i + wanted.len()].iter().all(|is_code| *is_code)
            && !text[..i].chars().next_back().is_some_and(is_ident)
            && !text[i + wanted.len()..]
                .chars()
                .next()
                .is_some_and(is_ident)
    })
}
