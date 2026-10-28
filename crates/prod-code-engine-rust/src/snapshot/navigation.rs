/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Code navigation queries: hover, definitions, implementations, references, and symbols.

use anyhow::Result;
use ra_ap_ide::{
    FileRange, FileStructureConfig, FindAllRefsConfig, GotoDefinitionConfig,
    GotoImplementationConfig, HoverConfig, HoverDocFormat, RaFixtureConfig, StructureNodeKind,
    TextRange,
};
use std::path::Path;

use super::RustEngineSnapshot;
use crate::types::{DefinitionTarget, ReferenceTarget, SymbolTarget};
use crate::vfs::{is_rust_source, offset_to_line_col};

impl RustEngineSnapshot {
    /// Retrieve symbol type, docs, and signature at (line, col).
    pub fn hover(&self, path: &Path, line: u32, col: u32) -> Result<Option<String>> {
        let position = self.file_position(path, line, col)?;

        let file_range = FileRange {
            file_id: position.file_id,
            range: TextRange::empty(position.offset),
        };

        let config = HoverConfig {
            links_in_hover: true,
            memory_layout: None,
            documentation: true,
            keywords: true,
            format: HoverDocFormat::Markdown,
            max_trait_assoc_items_count: None,
            max_fields_count: Some(10),
            max_enum_variants_count: Some(10),
            max_subst_ty_len: ra_ap_ide::SubstTyLen::Unlimited,
            show_drop_glue: true,
            ra_fixture: RaFixtureConfig::default(),
        };

        let res = self.analysis()?.hover(&config, file_range)?;
        Ok(res.map(|h| h.info.markup.to_string()))
    }

    /// Jump to symbol definition from (line, col).
    pub fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<DefinitionTarget>> {
        let overlay = self
            .overlay_for_path(path)
            .or_else(|| self.overlay_for_path(&self.workspace_root));
        let file_pos = self.file_position(path, line, col)?;
        let config = GotoDefinitionConfig {
            ra_fixture: RaFixtureConfig::default(),
        };

        let targets = match self.analysis()?.goto_definition(file_pos, &config)? {
            Some(range_info) => range_info.info,
            None => return Ok(vec![]),
        };

        let mut results = Vec::new();
        for target in targets {
            if !self.is_in_view(target.file_id, overlay.as_ref()) {
                continue;
            }
            if let Some(target_path) =
                self.path_for_file_id_in_view(target.file_id, overlay.as_ref())
            {
                let target_text = self.analysis()?.file_text(target.file_id)?;
                let focus = target.focus_range.unwrap_or(target.full_range);
                let (target_line, target_col) = offset_to_line_col(&target_text, focus.start());
                results.push(DefinitionTarget {
                    path: target_path,
                    line: target_line,
                    col: target_col,
                    name: target.name.to_string(),
                });
            }
        }

        Ok(results)
    }

    /// All implementations of the trait, or the impl blocks of the type, at (line, col).
    pub fn goto_implementation(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<DefinitionTarget>> {
        let overlay = self
            .overlay_for_path(path)
            .or_else(|| self.overlay_for_path(&self.workspace_root));
        let pos = self.file_position(path, line, col)?;
        let config = GotoImplementationConfig {
            filter_adjacent_derive_implementations: false,
        };
        let targets = match self.analysis()?.goto_implementation(&config, pos)? {
            Some(info) => info.info,
            None => return Ok(vec![]),
        };
        Ok(targets
            .iter()
            .filter(|t| self.is_in_view(t.file_id, overlay.as_ref()))
            .filter_map(|t| {
                let item = self.hierarchy_item_in_view(t, overlay.as_ref())?;
                Some(DefinitionTarget {
                    path: item.path,
                    line: item.line,
                    col: item.col,
                    name: item.name,
                })
            })
            .collect())
    }

    /// Find all references to symbol at (line, col) across entire workspace.
    pub fn find_all_refs(&self, path: &Path, line: u32, col: u32) -> Result<Vec<ReferenceTarget>> {
        let overlay = self
            .overlay_for_path(path)
            .or_else(|| self.overlay_for_path(&self.workspace_root));
        let file_pos = self.file_position(path, line, col)?;
        let config = FindAllRefsConfig {
            search_scope: None,
            ra_fixture: RaFixtureConfig::default(),
            exclude_imports: false,
            exclude_tests: false,
        };

        let search_res = match self.analysis()?.find_all_refs(file_pos, &config)? {
            Some(res) => res,
            None => return Ok(vec![]),
        };

        let mut results = Vec::new();
        for res in search_res {
            for (ref_file_id, refs) in res.references {
                if !self.is_in_view(ref_file_id, overlay.as_ref()) {
                    continue;
                }
                if let (Some(ref_path), Ok(ref_text)) = (
                    self.path_for_file_id_in_view(ref_file_id, overlay.as_ref()),
                    self.analysis()?.file_text(ref_file_id),
                ) {
                    for (range, _) in refs {
                        let (ref_line, ref_col) = offset_to_line_col(&ref_text, range.start());
                        results.push(ReferenceTarget {
                            path: ref_path.clone(),
                            line: ref_line,
                            col: ref_col,
                        });
                    }
                }
            }
        }

        Ok(results)
    }

    /// Generate outline / document symbols for a file.
    pub fn document_symbols(&self, path: &Path) -> Result<Vec<SymbolTarget>> {
        drop(self.analysis()?);
        let file_id = self.file_id_for_path(path).ok_or_else(|| {
            anyhow::anyhow!(
                "{} is not a Rust file, and no language server of this workspace outlines it",
                path.display()
            )
        })?;
        if !is_rust_source(path) && self.analysis()?.crates_for(file_id)?.is_empty() {
            anyhow::bail!(
                "{} is not a Rust file, and no language server of this workspace outlines it",
                path.display()
            );
        }

        let text = self.analysis()?.file_text(file_id)?;
        let config = FileStructureConfig {
            exclude_locals: false,
        };
        let nodes = self.analysis()?.file_structure(&config, file_id)?;

        let labels: Vec<String> = nodes.iter().map(|n| n.label.clone()).collect();
        let parents: Vec<Option<usize>> = nodes.iter().map(|n| n.parent).collect();
        let mut results = Vec::new();
        for node in nodes {
            let mut containers = Vec::new();
            let mut parent = node.parent;
            while let Some(index) = parent {
                if let Some(label) = labels.get(index) {
                    containers.push(label.clone());
                }
                parent = parents.get(index).copied().flatten();
                if containers.len() > 16 {
                    break;
                }
            }
            containers.reverse();
            // The navigation range is the item's name; the node range would start at its
            // doc comments and attributes.
            let (sym_line, sym_col) = offset_to_line_col(&text, node.navigation_range.start());
            let (end_line, _) = offset_to_line_col(&text, node.node_range.end());
            results.push(SymbolTarget {
                name: node.label,
                kind: match node.kind {
                    StructureNodeKind::SymbolKind(kind) => format!("{kind:?}"),
                    other => format!("{other:?}"),
                },
                line: sym_line,
                col: sym_col,
                end_line: end_line.max(sym_line),
                detail: node.detail,
                containers,
            });
        }

        Ok(results)
    }
}
