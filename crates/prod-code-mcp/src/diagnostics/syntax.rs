/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::path::Path;

use crate::diagnostics::types::{
    DiagnosticsReport, DocDiagnostic, HallucinationInterception, HallucinationKind,
};

pub use crate::markdown::validate_markdown;
pub use crate::xml_svg::validate_xml;

/// These diagnostics come from the supported source-language engines, not a manifest or
/// document validator. Refuse the entire batch before querying any part of it (#465).
pub fn ensure_source_file(file: &Path) -> Result<()> {
    if is_parser_validated_file(file) {
        return Ok(());
    }
    let supported = matches!(
        crate::lang::language_id_for_path(file),
        "rust"
            | "go"
            | "python"
            | "typescript"
            | "typescriptreact"
            | "javascript"
            | "javascriptreact"
            | "c"
            | "cpp"
            | "objective-c"
            | "objective-cpp"
            | "swift"
            | "java"
            | "kotlin"
            | "csharp"
            | "scala"
            | "zig"
            | "nim"
            | "d"
            | "php"
            | "ruby"
            | "dart"
            | "lua"
            | "elixir"
    ) || crate::lang::is_header(file);
    anyhow::ensure!(
        supported,
        "semantic diagnostics are not supported for {}; no validation was performed. \
         For manifests and lockfiles, use prod-code shadow-run with the \
         appropriate parser or build command on the complete proposal (for Rust, cargo check \
         --workspace --all-targets)",
        file.display()
    );
    Ok(())
}

/// Whether `file` is a JSON manifest or document whose syntax is validated directly (#733).
pub fn is_json_file(file: &Path) -> bool {
    crate::lang::language_id_for_path(file) == "json"
}

/// Whether `file` is validated directly by an internal parser without an LSP session (#733, #778).
pub fn is_parser_validated_file(file: &Path) -> bool {
    matches!(
        crate::lang::language_id_for_path(file),
        "json" | "markdown" | "xml"
    )
}

/// SVG syntax validator (#778).
pub fn validate_svg(shown: &str, text: &str) -> DiagnosticsReport {
    validate_xml(shown, text, true)
}

/// Validates file content using built-in syntax parsers (#733, #778).
pub fn validate_file_content(shown: &str, file: &Path, text: &str) -> DiagnosticsReport {
    match crate::lang::language_id_for_path(file) {
        "json" => validate_json(shown, text),
        "markdown" => validate_markdown(shown, text),
        "xml" => {
            let is_svg = file.extension().and_then(|e| e.to_str()) == Some("svg");
            validate_xml(shown, text, is_svg)
        }
        _ => unreachable!("validate_file_content called on non-parser-validated file"),
    }
}

/// JSON syntax validator for manifests and JSON configuration files (#733).
pub fn validate_json(shown: &str, text: &str) -> DiagnosticsReport {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(_) => DiagnosticsReport {
            file: shown.to_string(),
            errors: 0,
            warnings: 0,
            items: Vec::new(),
            preexisting: Vec::new(),
            in_derive: Vec::new(),
            auto_trait: Vec::new(),
            hallucinations: Vec::new(),
        },
        Err(err) => {
            let line = (err.line() as u32).max(1);
            let col = (err.column() as u32).max(1);
            let diag = DocDiagnostic {
                severity: "error".to_string(),
                message: format!("JSON syntax error: {err}"),
                code: Some("json-syntax".to_string()),
                line,
                col,
                source: None,
                end: None,
                note: None,
            };
            let hallucinations = vec![HallucinationInterception {
                kind: HallucinationKind::SyntaxError,
                symbol_or_target: None,
                message: diag.message.clone(),
                line,
                col,
                suggestion: Some("Correct invalid JSON syntax".to_string()),
            }];
            DiagnosticsReport {
                file: shown.to_string(),
                errors: 1,
                warnings: 0,
                items: vec![diag],
                preexisting: Vec::new(),
                in_derive: Vec::new(),
                auto_trait: Vec::new(),
                hallucinations,
            }
        }
    }
}
