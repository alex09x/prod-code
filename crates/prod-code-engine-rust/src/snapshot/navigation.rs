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

use anyhow::{Context, Result};
use ra_ap_ide::{
    FileId, FilePosition, FileRange, FileStructureConfig, FindAllRefsConfig, GotoDefinitionConfig,
    GotoImplementationConfig, HoverConfig, HoverDocFormat, MonikerResult, NavigationTarget,
    RaFixtureConfig, StructureNodeKind, TextRange,
};
use ra_ap_load_cargo::worktree::Overlay;
use std::collections::HashMap;
use std::path::Path;

use super::RustEngineSnapshot;
use crate::types::{DefinitionTarget, ReferenceTarget, SymbolTarget, WorkspaceSymbol};
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

        let res = self.analysis.hover(&config, file_range)?;
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

        let targets = match self.analysis.goto_definition(file_pos, &config)? {
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
                let target_text = self.analysis.file_text(target.file_id)?;
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
        let targets = match self.analysis.goto_implementation(&config, pos)? {
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

        let search_res = match self.analysis.find_all_refs(file_pos, &config)? {
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
                    self.analysis.file_text(ref_file_id),
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
        anyhow::ensure!(
            is_rust_source(path),
            "{} is not a Rust file, and no language server of this workspace outlines it",
            path.display()
        );
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;

        let text = self.analysis.file_text(file_id)?;
        let config = FileStructureConfig {
            exclude_locals: false,
        };
        let nodes = self.analysis.file_structure(&config, file_id)?;

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

    /// Workspace-wide symbol search by name (what `workspace/symbol` answers): fuzzy on the
    /// name, associated items included. Positions point at the name.
    ///
    /// The workspace's own crates are searched first, and their hits come alone: an agent
    /// asking for a name almost always means its own code, and library matches would crowd it
    /// out of the limit. Only when the workspace has no match does the search fall back to the
    /// dependency (library) crates, so a type the code uses from a dependency can still be
    /// found by name. A library hit's container is its module path, starting with its crate,
    /// because its file path alone (a registry checkout on the node) does not say which
    /// dependency it came from.
    pub fn workspace_symbols(&self, query: &str, limit: usize) -> Result<Vec<WorkspaceSymbol>> {
        let overlay = self.overlay_for_path(&self.workspace_root);
        let limit = limit.max(1);
        // The name itself first. The fuzzy search walks the index in name order and stops at
        // the limit, so in a large workspace the names that only hold the query's letters and
        // sort before it (`a_dry_run_…` before `run`) fill the limit and leave the symbol that
        // has the very name out (#348).
        let mut exact_local = ra_ap_ide::Query::new(query.to_string());
        exact_local.exact();
        let mut out: Vec<WorkspaceSymbol> = self
            .analysis
            .symbol_search(exact_local, limit)?
            .into_iter()
            .filter(|target| self.is_in_view(target.file_id, overlay.as_ref()))
            .filter_map(|target| self.workspace_symbol(target, None))
            .collect();
        let local = ra_ap_ide::Query::new(query.to_string());
        for symbol in self
            .analysis
            .symbol_search(local, limit)?
            .into_iter()
            .filter(|target| self.is_in_view(target.file_id, overlay.as_ref()))
            .filter_map(|target| self.workspace_symbol(target, None))
        {
            if out.len() >= limit {
                break;
            }
            let listed = out
                .iter()
                .any(|s| s.path == symbol.path && (s.line, s.col) == (symbol.line, symbol.col));
            if !listed {
                out.push(symbol);
            }
        }
        if out.iter().any(|s| s.name.eq_ignore_ascii_case(query)) {
            return Ok(out);
        }
        // Nothing in the workspace has the name, only names like it: a dependency's symbol of
        // that very name comes first (#328). `ra_ap_ide::Analysis` was never found, since the
        // workspace's `RustAnalysisOptions` kept the libraries from being searched at all.
        let mut exact = ra_ap_ide::Query::new(query.to_string());
        exact.libs();
        exact.exact();
        let mut found = self.library_symbols(exact, limit, overlay.as_ref())?;
        if found.is_empty() && out.is_empty() {
            let mut libs = ra_ap_ide::Query::new(query.to_string());
            libs.libs();
            found = self.library_symbols(libs, limit, overlay.as_ref())?;
        }
        found.extend(out);
        found.truncate(limit);
        Ok(found)
    }

    /// Library symbols matching `query`, each with the module path it is declared in.
    fn library_symbols(
        &self,
        query: ra_ap_ide::Query,
        limit: usize,
        overlay: Option<&Overlay>,
    ) -> Result<Vec<WorkspaceSymbol>> {
        let out = Vec::new();
        let targets = self.analysis.symbol_search(query, limit)?;
        if targets.is_empty() {
            return Ok(out);
        }
        // Crate names keyed by root file; a hit's file maps to its crate through that root.
        let crate_names: HashMap<FileId, String> = self
            .analysis
            .fetch_crates()?
            .into_iter()
            .filter_map(|info| Some((info.root_file_id, info.name?.replace('-', "_"))))
            .collect();
        Ok(targets
            .into_iter()
            .filter(|target| self.is_in_view(target.file_id, overlay))
            .filter_map(|target| {
                let module = self.module_path(&target).or_else(|| {
                    self.analysis
                        .crates_for(target.file_id)
                        .ok()?
                        .into_iter()
                        .find_map(|k| crate_names.get(&self.analysis.crate_root(k).ok()?))
                        .cloned()
                });
                self.workspace_symbol(target, module)
            })
            .collect())
    }

    /// `crate::module::path` a library hit is declared in, from its moniker. The library
    /// symbol index keeps no container name, so this is what tells the caller where the hit
    /// lives.
    fn module_path(&self, target: &NavigationTarget) -> Option<String> {
        let focus = target.focus_range.unwrap_or(target.full_range);
        let position = FilePosition {
            file_id: target.file_id,
            offset: focus.start(),
        };
        let monikers = self.analysis.moniker(position).ok()??.info;
        monikers.into_iter().find_map(|result| {
            let MonikerResult::Moniker(moniker) = result else {
                return None;
            };
            let identifier = moniker.identifier;
            let mut path = identifier.crate_name;
            // Modules, and the type a method belongs to (`ra_ap_ide::Analysis` for
            // `completions`, #328); not the item itself, nor the `impl` block between.
            for descriptor in identifier
                .description
                .iter()
                .take(identifier.description.len().saturating_sub(1))
                .filter(|d| d.name != "impl")
            {
                path.push_str("::");
                path.push_str(&descriptor.name);
            }
            Some(path)
        })
    }

    /// One search hit as a `WorkspaceSymbol`; `None` when its file is not on disk. `module`
    /// replaces the container, for hits outside the workspace.
    fn workspace_symbol(
        &self,
        target: NavigationTarget,
        module: Option<String>,
    ) -> Option<WorkspaceSymbol> {
        let path = self.path_for_file_id(target.file_id)?;
        let text = self.analysis.file_text(target.file_id).ok()?;
        let focus = target.focus_range.unwrap_or(target.full_range);
        let (line, col) = offset_to_line_col(&text, focus.start());
        let (end_line, _) = offset_to_line_col(&text, target.full_range.end());
        Some(WorkspaceSymbol {
            path,
            name: target.name.to_string(),
            kind: target
                .kind
                .map(|k| format!("{k:?}"))
                .unwrap_or_else(|| "Symbol".to_string()),
            line,
            col,
            end_line: end_line.max(line),
            container: module.or_else(|| target.container_name.map(|c| c.to_string())),
        })
    }
}
