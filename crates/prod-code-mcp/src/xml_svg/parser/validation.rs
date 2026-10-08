/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::lexer::XmlParser;
use crate::diagnostics::DiagnosticsReport;

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
