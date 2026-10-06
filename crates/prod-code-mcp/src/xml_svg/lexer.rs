/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::diagnostics::DocDiagnostic;

pub(crate) fn is_known_xml_entity(s: &str) -> bool {
    matches!(s, "amp" | "lt" | "gt" | "quot" | "apos")
        || (s.starts_with('#') && {
            let num = &s[1..];
            if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                u32::from_str_radix(hex, 16).is_ok()
            } else {
                num.parse::<u32>().is_ok()
            }
        })
}

pub(crate) fn is_xml_name_start_char(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b':'
}

pub(crate) fn is_xml_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_' || b == b':'
}

pub(crate) struct XmlParser<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) idx: usize,
    pub(crate) line: u32,
    pub(crate) col: u32,
    pub(crate) is_svg: bool,
    pub(crate) _shown: &'a str,
    pub(crate) diagnostics: Vec<DocDiagnostic>,
}

impl<'a> XmlParser<'a> {
    pub(crate) fn new(shown: &'a str, text: &'a str, is_svg: bool) -> Self {
        Self {
            bytes: text.as_bytes(),
            idx: 0,
            line: 1,
            col: 1,
            is_svg,
            _shown: shown,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn peek(&self) -> Option<u8> {
        self.bytes.get(self.idx).copied()
    }

    pub(crate) fn advance(&mut self) -> Option<u8> {
        let b = self.bytes.get(self.idx).copied()?;
        self.idx += 1;
        if b == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(b)
    }

    pub(crate) fn starts_with(&self, prefix: &[u8]) -> bool {
        self.bytes
            .get(self.idx..)
            .is_some_and(|rest| rest.starts_with(prefix))
    }

    pub(crate) fn consume(&mut self, prefix: &[u8]) -> bool {
        if self.starts_with(prefix) {
            for _ in 0..prefix.len() {
                self.advance();
            }
            true
        } else {
            false
        }
    }

    pub(crate) fn skip_whitespace(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    pub(crate) fn skip_until(&mut self, target: u8) {
        while let Some(b) = self.peek() {
            if b == target {
                break;
            }
            self.advance();
        }
    }

    pub(crate) fn read_xml_name(&mut self) -> String {
        let mut name = String::new();
        if let Some(first) = self.peek() {
            if is_xml_name_start_char(first) {
                name.push(first as char);
                self.advance();
                while let Some(next) = self.peek() {
                    if is_xml_name_char(next) {
                        name.push(next as char);
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
        }
        name
    }

    pub(crate) fn error(&mut self, line: u32, col: u32, message: impl Into<String>) {
        let code = if self.is_svg {
            "svg-syntax"
        } else {
            "xml-syntax"
        };
        self.diagnostics.push(DocDiagnostic {
            severity: "error".to_string(),
            message: message.into(),
            code: Some(code.to_string()),
            line,
            col,
            source: None,
            end: None,
            note: None,
        });
    }
}
