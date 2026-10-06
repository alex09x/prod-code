/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Structural search and replace (SSR) on syntax trees with name resolution.

use anyhow::{Context, Result};
use ra_ap_ide::{
    FileId, FilePosition, FileRange, FileStructureConfig, StructureNodeKind, TextRange, TextSize,
};
use std::path::Path;

use super::RustEngineSnapshot;
use crate::types::RefactorOutcome;
use crate::vfs::line_col_to_offset;

impl RustEngineSnapshot {
    /// Structural search and replace across the workspace (roadmap 8.7).
    ///
    /// `rule` is rust-analyzer's own SSR syntax, `pattern ==>> replacement`, where `$name`
    /// is a placeholder bound by the match: `$a.unwrap() ==>> $a.expect("...")`. Matching is
    /// on the syntax tree with name resolution, not on text, so a call written across three
    /// lines matches and a comment that looks like the pattern does not.
    ///
    /// A position is needed to resolve the paths the pattern mentions; any file of the
    /// workspace will do, and the caller passes the one the agent was looking at.
    pub fn structural_replace(
        &self,
        rule: &str,
        context: &Path,
        line: u32,
        col: u32,
        scope: Option<&Path>,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        let file_id = self
            .file_id_for_path(context)
            .with_context(|| format!("File not found in VFS: {:?}", context))?;
        let text = self.analysis.file_text(file_id)?;
        // The pattern's paths are resolved as if they appeared at this position, so it has to
        // be inside an item: at offset 0 a file has no scope and nothing resolves, which looks
        // exactly like "no matches". When the caller has no position of its own, use the body
        // of the file's first function.
        let requested_offset = line_col_to_offset(&text, line, col)
            .with_context(|| format!("Invalid position {line}:{col} in {}", context.display()))?;
        let offset = if line > 1 || col > 1 {
            requested_offset
        } else {
            self.first_body_offset(file_id).unwrap_or(TextSize::from(0))
        };
        let position = FilePosition { file_id, offset };
        // Without a scope the search covers the workspace and every crate it depends on,
        // which means a usage search with type inference per crate: minutes on a large
        // workspace. A scope restricts it to one file and makes it interactive.
        let selections = match scope {
            None => Vec::new(),
            Some(path) => {
                let scope_id = self
                    .file_id_for_path(path)
                    .with_context(|| format!("File not found in VFS: {:?}", path))?;
                let scope_text = self.analysis.file_text(scope_id)?;
                vec![FileRange {
                    file_id: scope_id,
                    range: TextRange::new(
                        TextSize::from(0),
                        TextSize::from(scope_text.len() as u32),
                    ),
                }]
            }
        };
        tracing::info!(
            rule,
            context = %context.display(),
            offset = u32::from(offset),
            scoped = selections.len(),
            "🔧 [SSR] resolving"
        );
        let change = match self
            .analysis
            .structural_search_replace(rule, false, position, selections)?
        {
            Ok(change) => change,
            Err(e) => {
                tracing::info!(rule, error = %e, "🔧 [SSR] refused");
                return Ok(Err(e.to_string()));
            }
        };
        let outcome = self.outcome_from_change(&change)?;
        tracing::info!(
            rule,
            files = outcome.files.len(),
            edits = outcome.total_edits(),
            "🔧 [SSR] done"
        );
        Ok(Ok(outcome))
    }

    /// An offset inside the first function of `file_id`, for resolving a pattern's paths.
    fn first_body_offset(&self, file_id: FileId) -> Option<TextSize> {
        let structure = self
            .analysis
            .file_structure(
                &FileStructureConfig {
                    exclude_locals: true,
                },
                file_id,
            )
            .ok()?;
        let node = structure.iter().find(|n| {
            matches!(
                n.kind,
                StructureNodeKind::SymbolKind(ra_ap_ide::SymbolKind::Function)
            )
        })?;
        // Just inside the item: past its name, where its body scope is in effect.
        Some(node.node_range.start() + TextSize::from(1))
    }
}
