/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::borrow::Cow;
use std::ops::Range;

use crate::caller_migration::mask::{ambiguous_regex_prefix, lexical_code_mask};
use crate::parameter_object::Language;

/// Keep trivia transparent without turning a literal into whitespace between tokens.
/// The lexical mask remains separate from the syntax view: literal names are ignored,
/// but their delimiters must still prevent a tagged template from becoming a call.
pub(super) struct SyntaxView<'a> {
    pub code: Cow<'a, str>,
    executable: Vec<bool>,
    uncertain: Vec<Range<usize>>,
}

impl<'a> SyntaxView<'a> {
    pub fn new(text: &'a str, lang: Language) -> Self {
        if !matches!(lang, Language::TypeScript | Language::JavaScript) {
            return Self {
                code: Cow::Borrowed(text),
                executable: Vec::new(),
                uncertain: Vec::new(),
            };
        }
        let executable = lexical_code_mask(text, lang);
        let bytes = text.as_bytes();
        let mut code: Vec<_> = bytes
            .iter()
            .zip(&executable)
            .map(|(&b, &yes)| if yes || b == b'\n' { b } else { b'~' })
            .collect();
        let mut uncertain = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if executable[i] {
                i += 1;
                continue;
            }
            let start = i;
            if text[i..].starts_with("//") || text[i..].starts_with("/*") {
                let end = if bytes[i + 1] == b'/' {
                    text[i..].find('\n').map_or(bytes.len(), |n| i + n)
                } else {
                    text[i + 2..].find("*/").map_or(bytes.len(), |n| i + n + 4)
                };
                for b in &mut code[i..end] {
                    if *b != b'\n' {
                        *b = b' ';
                    }
                }
                i = end;
            } else if matches!(bytes[i], b'\'' | b'"' | b'/') {
                let delimiter = bytes[i];
                let mut in_class = false;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                        continue;
                    }
                    if delimiter == b'/' {
                        if bytes[i] == b'[' {
                            in_class = true;
                        }
                        if bytes[i] == b']' {
                            in_class = false;
                        }
                    }
                    let closes = bytes[i] == delimiter && !in_class;
                    i += 1;
                    if closes {
                        break;
                    }
                }
                code[start] = delimiter;
                if i > start + 1 {
                    code[i - 1] = delimiter;
                }
                if delimiter == b'/' {
                    while i < bytes.len() && !executable[i] && bytes[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    // A closing brace or unproven contextual `yield`/`await` can leave
                    // the regex/division grammar ambiguous. Keep possible division
                    // operands as evidence rather than silently discarding them.
                    if code[..start]
                        .iter()
                        .rev()
                        .find(|b| !b.is_ascii_whitespace())
                        == Some(&b'}')
                        || ambiguous_regex_prefix(&bytes[..start], &executable[..start])
                    {
                        uncertain.push(start..i);
                    }
                }
            } else {
                // Template text ends at a backtick or an executable interpolation.
                // Retain both literal boundaries, including across interpolation.
                if bytes[i] == b'`' {
                    code[i] = b'`';
                    i += 1;
                }
                while i < bytes.len() && !executable[i] {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == b'`' {
                        code[i] = b'`';
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
        }
        Self {
            code: Cow::Owned(String::from_utf8(code).expect("mask preserves code characters")),
            executable,
            uncertain,
        }
    }

    pub fn is_code(&self, at: usize) -> bool {
        self.executable.get(at).copied().unwrap_or(true)
    }

    pub fn is_uncertain(&self, at: usize) -> bool {
        self.uncertain.iter().any(|span| span.contains(&at))
    }
}
