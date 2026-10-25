/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A unique line alignment is insufficient when a statement moves into a different scope.
//! Reuse the Rust lexical mask so braces in comments and literals cannot invent ancestry.
use super::scope_tokens::{normalized_code, rust_whitespace};
use crate::extract_trait::lex::lexical_code;

/// `>` continues the header, but a comma may instead finish a match arm or field.
/// Only retain a comma continuation with an outstanding angle in this delimiter's
/// context. Ambiguous comparisons conservatively retain context rather than lose it.
fn continues_argument(text: &str, mask: &[bool], after: usize, angles: usize) -> bool {
    let Some(next) = text[after..].char_indices().find_map(|(offset, ch)| {
        (mask[after + offset] && !rust_whitespace(ch)).then_some(after + offset)
    }) else {
        return false;
    };
    let bytes = text.as_bytes();
    if bytes[next] != b'>' && !(bytes[next] == b',' && angles > 0) {
        return false;
    }
    // The structural mask skips literals too. Only actual comment/whitespace trivia
    // may separate the block and its continuation; a literal can start a new statement.
    let mut i = after;
    while i < next {
        let ch = text[i..].chars().next().unwrap();
        if rust_whitespace(ch) {
            i += ch.len_utf8();
            continue;
        }
        match (bytes[i], bytes.get(i + 1)) {
            (b'/', Some(b'/')) => {
                i += text[i..next].find('\n').unwrap_or(next - i);
            }
            (b'/', Some(b'*')) => {
                let mut depth = 1;
                i += 2;
                while i < next && depth > 0 {
                    match (bytes[i], bytes.get(i + 1)) {
                        (b'/', Some(b'*')) => {
                            depth += 1;
                            i += 2;
                        }
                        (b'*', Some(b'/')) => {
                            depth -= 1;
                            i += 2;
                        }
                        _ => i += 1,
                    }
                }
                if depth != 0 {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

#[derive(Clone, PartialEq, Eq)]
struct Scope {
    line: u32,
    byte_col: usize,
    header: Vec<String>,
}

impl Scope {
    fn is_declaration_header_macro(&self) -> bool {
        let [prefix @ .., name, bang, brace] = self.header.as_slice() else {
            return false;
        };
        let name = name.strip_prefix("r#").unwrap_or(name);
        bang == "!"
            && brace == "{"
            && prefix
                .iter()
                .any(|token| matches!(token.as_str(), "fn" | "impl"))
            && name
                .chars()
                .next()
                .is_some_and(|ch| ch == '_' || unicode_ident::is_xid_start(ch))
            && name.chars().all(unicode_ident::is_xid_continue)
    }
}

struct Region {
    end: usize,
    scopes: Vec<Scope>,
    code: Vec<String>,
}

pub(super) struct RustContexts<'a> {
    text: &'a str,
    starts: Vec<usize>,
    regions: Vec<Region>,
}

impl<'a> RustContexts<'a> {
    pub(super) fn new(text: &'a str) -> Option<Self> {
        let mask = lexical_code(text);
        let mut starts = vec![0];
        let mut regions = Vec::new();
        let mut scopes = Vec::new();
        let mut delimiters = Vec::new();
        let mut angles = 0usize;
        let mut start = 0;
        let mut line = 1;
        let mut line_start = 0;
        for (i, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(i + 1);
                line += 1;
                line_start = i + 1;
            }
            if !mask[i] {
                continue;
            }
            match byte {
                b'<' => {
                    angles += 1;
                    continue;
                }
                // Arrows do not close generic arguments. Comparisons/shifts inside
                // a const expression have their own delimiter context below.
                b'>' => {
                    if !matches!(
                        i.checked_sub(1).map(|j| text.as_bytes()[j]),
                        Some(b'-' | b'=')
                    ) {
                        angles = angles.saturating_sub(1);
                    }
                    continue;
                }
                b'(' | b'[' => {
                    delimiters.push((byte, start, angles));
                    angles = 0;
                    continue;
                }
                b')' | b']' => {
                    let (opening, header_start, header_angles) = delimiters.pop()?;
                    if !matches!((opening, byte), (b'(', b')') | (b'[', b']')) {
                        return None;
                    }
                    // A nested expression may contain statement boundaries or blocks.
                    // The enclosing declaration still starts before this delimiter.
                    start = header_start;
                    angles = header_angles;
                    continue;
                }
                b';' if delimiters.last().is_some_and(|(byte, ..)| *byte != b'{') => {
                    continue;
                }
                b'{' | b'}' | b';' => {}
                _ => continue,
            }
            // Ignore only trivia for identity; literals remain tokens, not structure.
            // Compare mapped openings too: identical-looking sibling scopes are distinct.
            let code = normalized_code(text, &mask, start, i + 1);
            regions.push(Region {
                end: i,
                scopes: scopes.clone(),
                code: code.clone(),
            });
            let mut next_start = i + 1;
            match byte {
                b'{' => {
                    delimiters.push((byte, start, angles));
                    angles = 0;
                    scopes.push(Scope {
                        line,
                        byte_col: i - line_start,
                        header: code,
                    });
                }
                b'}' => {
                    let (opening, header_start, header_angles) = delimiters.pop()?;
                    if opening != b'{' {
                        return None;
                    }
                    let scope = scopes.pop()?;
                    // A braced type macro ends its token tree, not the function or
                    // impl declaration. Keep the prefix for the eventual body scope.
                    // Completed bodies and item macros do not have this suffix.
                    if scope.is_declaration_header_macro()
                        || delimiters.last().is_some_and(|(byte, ..)| *byte != b'{')
                        || continues_argument(text, &mask, i + 1, header_angles)
                    {
                        next_start = header_start;
                        angles = header_angles;
                    } else {
                        angles = 0;
                    }
                }
                _ => angles = 0,
            }
            start = next_start;
        }
        if !delimiters.is_empty() {
            return None;
        }
        regions.push(Region {
            end: text.len(),
            scopes,
            code: normalized_code(text, &mask, start, text.len()),
        });
        Some(Self {
            text,
            starts,
            regions,
        })
    }

    fn region(&self, line: u32, col: u32) -> Option<&Region> {
        let start = *self.starts.get(line.checked_sub(1)? as usize)?;
        let row = self.text.get(start..)?.split('\n').next()?;
        let mut utf16 = 1;
        let mut byte = None;
        for (i, ch) in row.char_indices() {
            if utf16 == col {
                byte = Some(start + i);
                break;
            }
            utf16 += ch.len_utf16() as u32;
        }
        if utf16 == col && byte.is_none() {
            byte = Some(start + row.len());
        }
        let byte = byte?;
        self.regions
            .get(self.regions.partition_point(|region| region.end < byte))
    }

    /// Both the statement/declaration context and every enclosing opening must correspond.
    /// Unknown, changed or ambiguous scope ancestry is insufficient baseline evidence.
    pub(super) fn corresponds(
        &self,
        old: &Self,
        lines: &[Option<u32>],
        after: (u32, u32),
        before: (u32, u32),
    ) -> bool {
        let (Some(new), Some(old)) = (
            self.region(after.0, after.1),
            old.region(before.0, before.1),
        ) else {
            return false;
        };
        new.code == old.code
            && new.scopes.len() == old.scopes.len()
            && new.scopes.iter().zip(&old.scopes).all(|(new, old)| {
                new.header == old.header
                    && new.byte_col == old.byte_col
                    && lines.get(new.line as usize - 1).copied().flatten() == Some(old.line)
            })
    }
}
