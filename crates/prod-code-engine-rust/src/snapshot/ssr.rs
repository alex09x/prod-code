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
        let file_id = self.require_file_id(context)?;
        let text = self.analysis()?.file_text(file_id)?;
        // The pattern's paths are resolved as if they appeared at this position, so it has to
        // be inside an item: at offset 0 a file has no scope and nothing resolves, which looks
        // exactly like "no matches". When the caller has no position of its own, use the body
        // of the file's first function, receiver method, or other scoped item.
        let requested_offset = line_col_to_offset(&text, line, col)
            .with_context(|| format!("Invalid position {line}:{col} in {}", context.display()))?;
        let offset = if line > 1 || col > 1 {
            requested_offset
        } else {
            self.first_body_offset(file_id, &text)
                .unwrap_or(TextSize::from(0))
        };
        let position = FilePosition { file_id, offset };
        // Without a scope the search covers the workspace and every crate it depends on,
        // which means a usage search with type inference per crate: minutes on a large
        // workspace. A scope restricts it to one file and makes it interactive.
        let selections = match scope {
            None => Vec::new(),
            Some(path) => {
                let scope_id = self.require_file_id(path)?;
                let scope_text = self.analysis()?.file_text(scope_id)?;
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
            .analysis()?
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

    /// An offset inside the first function, receiver method, or scoped item of `file_id`,
    /// for resolving a pattern's paths when the caller did not specify a position.
    fn first_body_offset(&self, file_id: FileId, text: &str) -> Option<TextSize> {
        use ra_ap_ide::SymbolKind;
        use ra_ap_syntax::{AstNode, ast};

        let structure = self
            .analysis()
            .ok()?
            .file_structure(
                &FileStructureConfig {
                    exclude_locals: true,
                },
                file_id,
            )
            .ok()?;
        let ends_with_brace = |range: TextRange| {
            text.get(usize::from(range.start())..usize::from(range.end()))
                .is_some_and(|slice| slice.trim_end().ends_with('}'))
        };
        let node = structure
            .iter()
            .find(|n| {
                matches!(
                    n.kind,
                    StructureNodeKind::SymbolKind(SymbolKind::Function | SymbolKind::Method)
                ) && ends_with_brace(n.node_range)
            })
            .or_else(|| {
                structure.iter().find(|n| {
                    matches!(
                        n.kind,
                        StructureNodeKind::SymbolKind(SymbolKind::Function | SymbolKind::Method)
                    )
                })
            })
            .or_else(|| {
                structure.iter().find(|n| {
                    matches!(
                        n.kind,
                        StructureNodeKind::SymbolKind(SymbolKind::Const | SymbolKind::Static)
                    )
                })
            })
            .or_else(|| {
                structure.iter().find(|n| {
                    matches!(
                        n.kind,
                        StructureNodeKind::SymbolKind(
                            SymbolKind::Impl
                                | SymbolKind::Trait
                                | SymbolKind::Struct
                                | SymbolKind::Enum
                                | SymbolKind::Union
                                | SymbolKind::TypeAlias
                        )
                    )
                })
            })
            .or_else(|| {
                structure.iter().find(|n| {
                    matches!(n.kind, StructureNodeKind::SymbolKind(SymbolKind::Module))
                        && ends_with_brace(n.node_range)
                })
            })?;
        if matches!(
            node.kind,
            StructureNodeKind::SymbolKind(SymbolKind::Const | SymbolKind::Static)
        ) {
            let file = self.analysis().ok()?.parse(file_id).ok()?;
            let initializer = file.syntax().descendants().find_map(|syntax| {
                if syntax.text_range() != node.node_range {
                    return None;
                }
                ast::Const::cast(syntax.clone())
                    .and_then(|item| item.body())
                    .or_else(|| ast::Static::cast(syntax).and_then(|item| item.body()))
            });
            if let Some(initializer) = initializer {
                // Trivia before the item's semicolon belongs to the item, outside a block
                // initializer's local import scope. Select the expression's last token.
                return Some(initializer.syntax().text_range().end());
            }
        }
        // `ide_ssr::ResolutionScope::new` resolves `token_at_offset(offset).left_biased().parent()`.
        // For brace-delimited items (`fn`, `impl`, `trait`, `mod`, braced ADTs), `node_range.end()`
        // selects the closing `}` whose parent is the inner `StmtList`/`AssocItemList`/`ItemList`
        // (entering `DefWithBodyId`/`BlockScope` and keeping `current_function()` on `SyntaxKind::FN`).
        // Initializers were handled above. For remaining semicolon-terminated declarations,
        // use the token before the final semicolon.
        if ends_with_brace(node.node_range) {
            Some(node.node_range.end())
        } else if node.node_range.len() > TextSize::from(1) {
            Some(node.node_range.end() - TextSize::from(1))
        } else {
            Some(node.node_range.start())
        }
    }
}
