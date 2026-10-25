/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::modifiers::trivia_end;
use crate::parameter_object::matching_bracket;

/// Find the actual body/declaration boundary, not punctuation in suffix trivia,
/// quoted operands, attributes, or balanced expressions such as `noexcept(...)`.
pub(super) fn cpp_member_delimiter(text: &str, mut offset: usize) -> Option<usize> {
    loop {
        offset = trivia_end(text, offset)?;
        match *text.as_bytes().get(offset)? {
            b'{' | b';' => return Some(offset),
            b'(' | b'[' => offset = group_end(text, offset)?,
            quote @ (b'"' | b'\'') => {
                offset += 1;
                loop {
                    match *text.as_bytes().get(offset)? {
                        b'\\' => offset += 2,
                        byte if byte == quote => {
                            offset += 1;
                            break;
                        }
                        _ => offset += 1,
                    }
                }
            }
            _ => offset += text[offset..].chars().next()?.len_utf8(),
        }
    }
}

/// Recognize a bounded method declarator. Defaults belong to the balanced parameter
/// list, so neither `=` nor quoted text there can hide a trailing `final` modifier.
/// Unsupported headers return None: relocation must refuse, comparison keeps the bytes.
pub(super) fn strip_cpp_suffix(text: &str) -> Option<String> {
    let mut offset = trivia_end(text, 0)?;
    let open = loop {
        offset = trivia_end(text, offset)?;
        match text.as_bytes().get(offset)? {
            b'(' => break offset,
            b'[' => offset = group_end(text, offset)?,
            b'{' | b';' | b'=' | b'"' | b'\'' => return None,
            _ => offset += text[offset..].chars().next()?.len_utf8(),
        }
    };
    offset = group_end(text, open)?;
    let mut removals = Vec::new();
    loop {
        offset = trivia_end(text, offset)?;
        match text.as_bytes().get(offset)? {
            b'{' | b';' => break,
            b'=' => {
                // Defaulted/deleted destructors and pure declarations have no body.
                let tail = text[offset + 1..].trim().strip_suffix(';')?.trim();
                if !matches!(tail, "default" | "delete" | "0") {
                    return None;
                }
                break;
            }
            b'&' => offset += 1,
            b'[' if text[offset..].starts_with("[[") => offset = group_end(text, offset)?,
            _ => {
                let end = offset
                    + text[offset..]
                        .find(|c: char| !c.is_alphanumeric() && c != '_')
                        .unwrap_or(text.len() - offset);
                let word = &text[offset..end];
                match word {
                    "override" | "final" => {
                        let start = text[..offset].trim_end_matches([' ', '\t']).len();
                        removals.push(start..end);
                    }
                    "const" | "volatile" => {}
                    "noexcept" => {
                        let next = trivia_end(text, end)?;
                        offset = if text.as_bytes().get(next) == Some(&b'(') {
                            group_end(text, next)?
                        } else {
                            end
                        };
                        continue;
                    }
                    _ => return None,
                }
                offset = end;
            }
        }
    }
    let mut result = text.to_owned();
    for range in removals.into_iter().rev() {
        result.replace_range(range, "");
    }
    Some(result)
}

fn group_end(text: &str, open: usize) -> Option<usize> {
    let close = matching_bracket(text, open)?;
    // The shared bracket scanner handles ordinary quoted defaults and comments but
    // has no C++ raw-string delimiter model. Refuse such headers rather than guess.
    if text[open..=close].contains("R\"") {
        return None;
    }
    Some(close + 1)
}
