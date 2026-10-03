//! Parser-backed validator for XML and SVG proposals (#778).

use crate::diagnostics::{DiagnosticsReport, DocDiagnostic};

/// Known standard XML entity names.
fn is_known_xml_entity(entity: &str) -> bool {
    if matches!(entity, "amp" | "lt" | "gt" | "quot" | "apos") {
        return true;
    }
    if let Some(rest) = entity.strip_prefix('#') {
        if let Some(hex) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X')) {
            return u32::from_str_radix(hex, 16).is_ok();
        }
        return rest.parse::<u32>().is_ok();
    }
    false
}

fn is_xml_name_start_char(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == ':'
}

fn is_xml_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '.' || c == '-' || c == '_' || c == ':'
}

struct XmlParser<'a> {
    bytes: &'a [u8],
    idx: usize,
    line: u32,
    col: u32,
    is_svg: bool,
    _shown: &'a str,
    diagnostics: Vec<DocDiagnostic>,
}

impl<'a> XmlParser<'a> {
    fn new(_shown: &'a str, text: &'a str, is_svg: bool) -> Self {
        Self {
            bytes: text.as_bytes(),
            idx: 0,
            line: 1,
            col: 1,
            is_svg,
            _shown,
            diagnostics: Vec::new(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.idx).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        if self.idx >= self.bytes.len() {
            return None;
        }
        let b = self.bytes[self.idx];
        self.idx += 1;
        if b == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(b)
    }

    fn starts_with(&self, s: &[u8]) -> bool {
        self.bytes[self.idx..].starts_with(s)
    }

    fn consume(&mut self, s: &[u8]) -> bool {
        if self.starts_with(s) {
            for _ in 0..s.len() {
                self.advance();
            }
            true
        } else {
            false
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_until(&mut self, target: u8) {
        while let Some(b) = self.peek() {
            if b == target {
                break;
            }
            self.advance();
        }
    }

    fn read_xml_name(&mut self) -> String {
        let mut name = String::new();
        if let Some(b) = self.peek() {
            let c = b as char;
            if is_xml_name_start_char(c) {
                name.push(c);
                self.advance();
                while let Some(b2) = self.peek() {
                    let c2 = b2 as char;
                    if is_xml_name_char(c2) {
                        name.push(c2);
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
        }
        name
    }

    fn error(&mut self, line: u32, col: u32, message: impl Into<String>) {
        let code = if self.is_svg { "svg-syntax" } else { "xml-syntax" };
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

    fn parse(&mut self) {
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
                    self.error(self.line, self.col, format!("expected `>` after `</{name}`"));
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
                            format!("unexpected closing tag `</{name}>` without matching opening tag"),
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
                            format!("multiple root elements in XML document; extra root `<{name}>`"),
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
                                format!("unclosed quote for attribute `{attr_name}` in tag `<{name}>`"),
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
                        && !b.is_ascii_whitespace() && root_count >= 1 {
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

/// Validates an XML document or SVG vector graphic (#778).
pub fn validate_xml(shown: &str, text: &str, is_svg: bool) -> DiagnosticsReport {
    let mut parser = XmlParser::new(shown, text, is_svg);
    parser.parse();
    DiagnosticsReport {
        file: shown.to_string(),
        errors: parser
            .diagnostics
            .iter()
            .filter(|d| d.severity == "error")
            .count(),
        warnings: parser
            .diagnostics
            .iter()
            .filter(|d| d.severity == "warning")
            .count(),
        items: parser.diagnostics,
        preexisting: Vec::new(),
        in_derive: Vec::new(),
        auto_trait: Vec::new(),
        hallucinations: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_svg_passes_with_zero_errors() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <circle cx="50" cy="50" r="40" fill="red" />
  <text x="10" y="20">Hello &amp; World</text>
</svg>"#;
        let report = validate_xml("test.svg", svg, true);
        assert_eq!(report.errors, 0, "{:?}", report.items);
        assert_eq!(report.warnings, 0);
    }

    #[test]
    fn non_svg_root_in_svg_file_is_error() {
        let xml = r#"<html><body></body></html>"#;
        let report = validate_xml("card.svg", xml, true);
        assert_eq!(report.errors, 1);
        assert!(report.items[0].message.contains("SVG document root element must be `<svg>`"));
    }

    #[test]
    fn mismatched_tags_report_error_with_line_and_col() {
        let xml = r#"<svg viewBox="0 0 100 100">
  <g>
    <circle cx="10" cy="10" r="5" />
  </path>
</svg>"#;
        let report = validate_xml("card.svg", xml, true);
        assert_eq!(report.errors, 1);
        assert!(report.items[0].message.contains("mismatched closing tag `</path>`; expected `</g>`"));
        assert_eq!(report.items[0].line, 4);
    }

    #[test]
    fn unclosed_quote_reports_error() {
        let xml = r#"<svg viewBox="0 0 100 100>
</svg>"#;
        let report = validate_xml("card.svg", xml, true);
        assert!(report.errors >= 1);
        assert!(report.items.iter().any(|d| d.message.contains("unclosed quote")));
    }

    #[test]
    fn duplicate_attributes_report_error() {
        let xml = r#"<svg viewBox="0 0 100 100" viewBox="0 0 50 50"></svg>"#;
        let report = validate_xml("card.svg", xml, true);
        assert_eq!(report.errors, 1);
        assert!(report.items[0].message.contains("duplicate attribute `viewBox`"));
    }
}
