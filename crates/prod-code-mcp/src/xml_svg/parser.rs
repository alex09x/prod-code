/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod validation;
pub use validation::validate_xml;

use super::lexer::{XmlParser, is_known_xml_entity};

impl<'a> XmlParser<'a> {
    pub(crate) fn parse(&mut self) {
        let mut stack: Vec<(String, u32, u32)> = Vec::new();
        let mut root_count = 0;

        while self.idx < self.bytes.len() {
            if self.diagnostics.len() >= 10 {
                break;
            }

            if self.starts_with(b"<!--") {
                let start_line = self.line;
                let start_col = self.col;
                self.consume(b"<!--");
                let mut closed = false;
                while self.idx < self.bytes.len() {
                    if self.starts_with(b"-->") {
                        self.consume(b"-->");
                        closed = true;
                        break;
                    }
                    self.advance();
                }
                if !closed {
                    self.error(start_line, start_col, "unclosed XML comment `<!--`");
                    break;
                }
            } else if self.starts_with(b"<![CDATA[") {
                let start_line = self.line;
                let start_col = self.col;
                self.consume(b"<![CDATA[");
                let mut closed = false;
                while self.idx < self.bytes.len() {
                    if self.starts_with(b"]]>") {
                        self.consume(b"]]>");
                        closed = true;
                        break;
                    }
                    self.advance();
                }
                if !closed {
                    self.error(start_line, start_col, "unclosed CDATA section `<![CDATA[`");
                    break;
                }
            } else if self.starts_with(b"<?") {
                let start_line = self.line;
                let start_col = self.col;
                self.consume(b"<?");
                let mut closed = false;
                while self.idx < self.bytes.len() {
                    if self.starts_with(b"?>") {
                        self.consume(b"?>");
                        closed = true;
                        break;
                    }
                    self.advance();
                }
                if !closed {
                    self.error(
                        start_line,
                        start_col,
                        "unclosed XML processing instruction or declaration `<?`",
                    );
                    break;
                }
            } else if self.starts_with(b"<!DOCTYPE") || self.starts_with(b"<!doctype") {
                let start_line = self.line;
                let start_col = self.col;
                self.consume(b"<!");
                let mut depth = 0;
                let mut in_quote: Option<u8> = None;
                let mut closed = false;
                while let Some(b) = self.peek() {
                    if let Some(q) = in_quote {
                        if b == q {
                            in_quote = None;
                        }
                        self.advance();
                    } else if b == b'"' || b == b'\'' {
                        in_quote = Some(b);
                        self.advance();
                    } else if b == b'[' {
                        depth += 1;
                        self.advance();
                    } else if b == b']' {
                        if depth > 0 {
                            depth -= 1;
                        }
                        self.advance();
                    } else if b == b'>' && depth == 0 {
                        self.advance();
                        closed = true;
                        break;
                    } else {
                        self.advance();
                    }
                }
                if !closed {
                    self.error(start_line, start_col, "unclosed DOCTYPE declaration");
                    break;
                }
            } else if self.starts_with(b"</") {
                let tag_line = self.line;
                let tag_col = self.col;
                self.consume(b"</");
                self.skip_whitespace();
                let name = self.read_xml_name();
                if name.is_empty() {
                    self.error(
                        tag_line,
                        tag_col,
                        "expected element name in closing tag `</...>`",
                    );
                    self.skip_until(b'>');
                    if self.starts_with(b">") {
                        self.advance();
                    }
                    continue;
                }
                self.skip_whitespace();
                if self.peek() != Some(b'>') {
                    self.error(
                        self.line,
                        self.col,
                        format!("expected `>` after `</{name}`"),
                    );
                    self.skip_until(b'>');
                }
                if self.starts_with(b">") {
                    self.advance();
                }

                match stack.pop() {
                    Some((open_name, open_line, open_col)) => {
                        if open_name != name {
                            self.error(
                                tag_line,
                                tag_col,
                                format!(
                                    "mismatched closing tag `</{name}>`; expected `</{open_name}>` (opened at line {open_line}, col {open_col})"
                                ),
                            );
                        }
                    }
                    None => {
                        self.error(
                            tag_line,
                            tag_col,
                            format!(
                                "unexpected closing tag `</{name}>` without matching opening tag"
                            ),
                        );
                    }
                }
            } else if self.starts_with(b"<") {
                let tag_line = self.line;
                let tag_col = self.col;
                self.consume(b"<");
                let name = self.read_xml_name();
                if name.is_empty() {
                    self.error(tag_line, tag_col, "expected XML element name after `<`");
                    self.skip_until(b'>');
                    if self.starts_with(b">") {
                        self.advance();
                    }
                    continue;
                }

                if stack.is_empty() {
                    root_count += 1;
                    if root_count > 1 {
                        self.error(
                            tag_line,
                            tag_col,
                            format!(
                                "multiple root elements in XML document; extra root `<{name}>`"
                            ),
                        );
                    }
                    if self.is_svg && name != "svg" {
                        self.error(
                            tag_line,
                            tag_col,
                            format!("SVG document root element must be `<svg>`, found `<{name}>`"),
                        );
                    }
                }

                let mut seen_attrs = std::collections::HashSet::new();
                let mut self_closing = false;

                loop {
                    self.skip_whitespace();
                    if self.starts_with(b"/>") {
                        self.consume(b"/>");
                        self_closing = true;
                        break;
                    }
                    if self.starts_with(b">") {
                        self.consume(b">");
                        break;
                    }
                    if self.idx >= self.bytes.len() {
                        self.error(
                            tag_line,
                            tag_col,
                            format!("unclosed opening tag `<{name}>` at end of file"),
                        );
                        break;
                    }

                    let attr_line = self.line;
                    let attr_col = self.col;
                    let attr_name = self.read_xml_name();
                    if attr_name.is_empty() {
                        self.error(
                            attr_line,
                            attr_col,
                            format!("expected attribute name or `>` in tag `<{name}>`"),
                        );
                        self.skip_until(b'>');
                        if self.starts_with(b">") {
                            self.advance();
                        }
                        break;
                    }

                    if !seen_attrs.insert(attr_name.clone()) {
                        self.error(
                            attr_line,
                            attr_col,
                            format!("duplicate attribute `{attr_name}` in tag `<{name}>`"),
                        );
                    }

                    self.skip_whitespace();
                    if self.peek() != Some(b'=') {
                        self.error(
                            self.line,
                            self.col,
                            format!("expected `=` after attribute `{attr_name}` in tag `<{name}>`"),
                        );
                    } else {
                        self.advance();
                    }

                    self.skip_whitespace();
                    let quote = self.peek();
                    if quote != Some(b'"') && quote != Some(b'\'') {
                        self.error(
                            self.line,
                            self.col,
                            format!(
                                "expected quoted value ('\"' or '\'') for attribute `{attr_name}` in tag `<{name}>`"
                            ),
                        );
                    } else {
                        let q = quote.unwrap();
                        let q_line = self.line;
                        let q_col = self.col;
                        self.advance();
                        let mut unclosed_quote = true;
                        while let Some(b) = self.peek() {
                            if b == q {
                                self.advance();
                                unclosed_quote = false;
                                break;
                            }
                            if b == b'<' {
                                self.error(
                                    self.line,
                                    self.col,
                                    format!(
                                        "unescaped `<` inside attribute value for `{attr_name}` in tag `<{name}>`"
                                    ),
                                );
                            }
                            self.advance();
                        }
                        if unclosed_quote {
                            self.error(
                                q_line,
                                q_col,
                                format!(
                                    "unclosed quote for attribute `{attr_name}` in tag `<{name}>`"
                                ),
                            );
                            break;
                        }
                    }
                }

                if !self_closing {
                    stack.push((name, tag_line, tag_col));
                }
            } else {
                if stack.is_empty()
                    && let Some(b) = self.peek()
                    && !b.is_ascii_whitespace()
                    && root_count >= 1
                {
                    self.error(
                        self.line,
                        self.col,
                        "content is not allowed in trailing section after root element",
                    );
                    self.advance();
                    continue;
                }

                if self.starts_with(b"&") {
                    let ent_line = self.line;
                    let ent_col = self.col;
                    self.advance();
                    let mut entity = String::new();
                    let mut valid_entity = false;
                    while let Some(b) = self.peek() {
                        if b == b';' {
                            self.advance();
                            valid_entity = true;
                            break;
                        }
                        if b.is_ascii_alphanumeric() || b == b'#' || b == b'_' {
                            entity.push(b as char);
                            self.advance();
                            if entity.len() > 10 {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    if !valid_entity {
                        self.error(
                            ent_line,
                            ent_col,
                            "unescaped `&` in XML text; use `&amp;` instead",
                        );
                    } else if !is_known_xml_entity(&entity) {
                        self.error(
                            ent_line,
                            ent_col,
                            format!("unknown or unsupported XML entity `&{entity};`"),
                        );
                    }
                } else {
                    self.advance();
                }
            }
        }

        if root_count == 0 && self.diagnostics.is_empty() {
            self.error(
                1,
                1,
                if self.is_svg {
                    "empty SVG document; expected `<svg>` root element"
                } else {
                    "empty XML document; expected root element"
                },
            );
        }

        for (unclosed, u_line, u_col) in stack {
            self.error(
                u_line,
                u_col,
                format!("unclosed XML tag `<{unclosed}>` opened at line {u_line}, col {u_col}"),
            );
        }
    }
}
