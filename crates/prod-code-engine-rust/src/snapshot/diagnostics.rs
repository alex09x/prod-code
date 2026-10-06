/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Diagnostic computation, unresolved path checks, and unused import warnings.

use anyhow::{Context, Result};
use ra_ap_ide::{
    AssistResolveStrategy, DiagnosticsConfig, FileId, FileRange, HighlightConfig, HlTag,
    RaFixtureConfig, SymbolKind, TextRange, TextSize,
};
use std::collections::HashMap;
use std::path::Path;

use super::RustEngineSnapshot;
use crate::types::FileDiagnostic;
use crate::vfs::{imported_in_scope, offset_to_line_col, use_path_start};

impl RustEngineSnapshot {
    /// All diagnostics rust-analyzer computes for `path` from the in-memory database: syntax
    /// errors, unresolved names, type mismatches, unused items and the like. This is what a
    /// proposed edit can be validated against before it touches disk.
    pub fn diagnostics(&self, path: &Path) -> Result<Vec<FileDiagnostic>> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;
        let text = self.analysis.file_text(file_id)?;
        let config = DiagnosticsConfig::test_sample();
        let started = std::time::Instant::now();
        let diagnostics =
            self.analysis
                .full_diagnostics(&config, AssistResolveStrategy::None, file_id)?;
        let analyzer = started.elapsed();
        let unused_imports = self.unused_imports(file_id, &text);
        let imports = started.elapsed() - analyzer;
        let covered: Vec<TextRange> = diagnostics
            .iter()
            .filter(|d| d.range.file_id == file_id)
            .map(|d| d.range.range)
            .collect();
        let unresolved = self.unresolved_paths(file_id, &text, &covered);
        tracing::debug!(
            file = %path.display(),
            engine = self.label,
            changes = self.changes,
            analyzer_ms = analyzer.as_millis() as u64,
            unused_imports_ms = imports.as_millis() as u64,
            unresolved_paths_ms = (started.elapsed() - analyzer - imports).as_millis() as u64,
            "diagnostics pass by part"
        );
        Ok(diagnostics
            .into_iter()
            .filter(|d| d.range.file_id == file_id)
            .map(|d| {
                let (line, col) = offset_to_line_col(&text, d.range.range.start());
                let (end_line, end_col) = offset_to_line_col(&text, d.range.range.end());
                let severity = format!("{:?}", d.severity).to_ascii_lowercase();
                FileDiagnostic {
                    code: d.code.as_str().to_string(),
                    message: d.message,
                    severity: match severity.as_str() {
                        "weakwarning" => "weak".to_string(),
                        other => other.to_string(),
                    },
                    line,
                    col,
                    end_line,
                    end_col,
                    unused: d.unused,
                }
            })
            .chain(unused_imports)
            .chain(unresolved)
            .collect())
    }

    /// Paths in `file_id` that name nothing (#181): rustc's "cannot find type" (E0412) and
    /// "failed to resolve" (E0433). rust-analyzer computes no diagnostic for them; it only
    /// highlights a name it cannot resolve as an unresolved reference. Such a path segment is
    /// reported when it starts its path, or when what qualifies it is a module or a crate,
    /// whose contents are known without inference. A segment after a type (`T::Item`) is left
    /// alone, and so is anything inside an attribute, a macro call (tokens, not paths) or a
    /// range another diagnostic already covers, such as code a `#[cfg]` turns off.
    fn unresolved_paths(
        &self,
        file_id: FileId,
        text: &str,
        covered: &[TextRange],
    ) -> Vec<FileDiagnostic> {
        use ra_ap_syntax::AstNode;
        use ra_ap_syntax::ast::{self, PathSegmentKind};
        let config = HighlightConfig {
            strings: false,
            comments: false,
            punctuation: false,
            specialize_punctuation: false,
            operator: false,
            specialize_operator: false,
            inject_doc_comment: false,
            macro_bang: false,
            syntactic_name_ref_highlighting: false,
            ra_fixture: RaFixtureConfig::default(),
        };
        let (Ok(highlights), Ok(file)) = (
            self.analysis.highlight(config, file_id),
            self.analysis.parse(file_id),
        ) else {
            return Vec::new();
        };
        let tags: HashMap<TextRange, HlTag> = highlights
            .into_iter()
            .map(|h| (h.range, h.highlight.tag))
            .collect();
        let unresolved = |name: &ast::NameRef| {
            tags.get(&name.syntax().text_range()) == Some(&HlTag::UnresolvedReference)
        };
        let mut out = Vec::new();
        for segment in file
            .syntax()
            .descendants()
            .filter_map(ast::PathSegment::cast)
        {
            let Some(PathSegmentKind::Name(name)) = segment.kind() else {
                continue;
            };
            let range = name.syntax().text_range();
            let name_text = name.text().to_string();
            // A workspace loaded without the sysroot resolves no `std`; that is its setting,
            // not the edit's error.
            if !unresolved(&name)
                || matches!(name_text.as_str(), "std" | "core" | "alloc")
                || covered.iter().any(|c| c.intersect(range).is_some())
                || segment
                    .syntax()
                    .ancestors()
                    .any(|n| n.kind() == ra_ap_syntax::SyntaxKind::ATTR)
            {
                continue;
            }
            if imported_in_scope(segment.syntax(), &name_text) {
                continue;
            }
            let path = segment.parent_path();
            let qualified_by_module = match path.qualifier() {
                None => true,
                Some(qualifier) => match qualifier.segment().and_then(|s| s.kind()) {
                    Some(
                        PathSegmentKind::CrateKw
                        | PathSegmentKind::SelfKw
                        | PathSegmentKind::SuperKw,
                    ) => true,
                    Some(PathSegmentKind::Name(outer)) => matches!(
                        tags.get(&outer.syntax().text_range()),
                        Some(HlTag::Symbol(SymbolKind::Module | SymbolKind::CrateRoot))
                    ),
                    _ => false,
                },
            };
            if !qualified_by_module {
                continue;
            }
            // Another crate can hold code the analyzer does not see (a source a build script
            // writes and `include!`s), so a name missing from it is only a warning.
            let in_other_crate =
                path.first_segment()
                    .and_then(|s| s.name_ref())
                    .is_some_and(|first| {
                        first.syntax() != name.syntax()
                            && tags.get(&first.syntax().text_range())
                                == Some(&HlTag::Symbol(SymbolKind::CrateRoot))
                    });
            let (line, col) = offset_to_line_col(text, range.start());
            let (end_line, end_col) = offset_to_line_col(text, range.end());
            out.push(FileDiagnostic {
                code: "unresolved-path".to_string(),
                message: if in_other_crate {
                    format!(
                        "cannot find `{}` in `{}`: the analyzer sees no such item (run code_check to be sure)",
                        name.text(),
                        path.qualifier().map(|q| q.syntax().text().to_string()).unwrap_or_default()
                    )
                } else {
                    format!(
                        "cannot find `{}` in this scope: no item of that name resolves here",
                        name.text()
                    )
                },
                severity: if in_other_crate { "warning" } else { "error" }.to_string(),
                line,
                col,
                end_line,
                end_col,
                unused: false,
            });
        }
        out
    }

    /// The `use` items of `file_id` that import something unused (#134). rust-analyzer computes
    /// no diagnostic for an unused import — rustc does, and `-D warnings` rejects it — but it
    /// offers `remove_unused_imports` exactly on a `use` item that has one. So each `use` item is
    /// asked for its assists, and the ones that offer it are reported as rustc would.
    fn unused_imports(&self, file_id: FileId, text: &str) -> Vec<FileDiagnostic> {
        let (assist_config, diagnostics_config) = Self::assist_configs();
        let mut out = Vec::new();
        let mut offset = 0usize;
        for line in text.split_inclusive('\n') {
            let item_start = offset + (line.len() - line.trim_start().len());
            offset += line.len();
            let Some(path_start) = use_path_start(&text[item_start..]) else {
                continue;
            };
            let at = TextSize::from((item_start + path_start) as u32);
            let range = FileRange {
                file_id,
                range: TextRange::empty(at),
            };
            let Ok(assists) = self.analysis.assists_with_fixes(
                &assist_config,
                &diagnostics_config,
                AssistResolveStrategy::None,
                range,
            ) else {
                continue;
            };
            if !assists.iter().any(|a| a.id.0 == "remove_unused_imports") {
                continue;
            }
            let end = text[item_start..]
                .find(';')
                .map_or(text.len(), |i| item_start + i + 1);
            let (line_no, col) = offset_to_line_col(text, TextSize::from(item_start as u32));
            let (end_line, end_col) = offset_to_line_col(text, TextSize::from(end as u32));
            out.push(FileDiagnostic {
                code: "unused_imports".to_string(),
                message: "unused import".to_string(),
                severity: "warning".to_string(),
                line: line_no,
                col,
                end_line,
                end_col,
                unused: true,
            });
        }
        out
    }
}
