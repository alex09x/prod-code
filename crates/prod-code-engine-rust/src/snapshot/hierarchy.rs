/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Call hierarchy and workspace symbol queries.

use anyhow::Result;
use ra_ap_ide::{
    CallHierarchyConfig, FileId, FilePosition, MonikerResult, NavigationTarget, RaFixtureConfig,
};
use ra_ap_load_cargo::worktree::Overlay;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::RustEngineSnapshot;
use crate::types::{CallEdge, HierarchyItem, WorkspaceSymbol};
use crate::vfs::offset_to_line_col;

impl RustEngineSnapshot {
    /// The call-hierarchy item(s) at (line, col): the enclosing or referenced function.
    pub fn prepare_call_hierarchy(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<HierarchyItem>> {
        let overlay = self
            .overlay_for_path(path)
            .or_else(|| self.overlay_for_path(&self.workspace_root));
        let pos = self.file_position(path, line, col)?;
        let config = CallHierarchyConfig {
            exclude_tests: false,
            ra_fixture: RaFixtureConfig::default(),
        };
        let targets = match self.analysis()?.call_hierarchy(pos, &config)? {
            Some(info) => info.info,
            None => return Ok(vec![]),
        };
        Ok(targets
            .iter()
            .filter(|t| self.is_in_view(t.file_id, overlay.as_ref()))
            .filter_map(|t| self.hierarchy_item_in_view(t, overlay.as_ref()))
            .collect())
    }

    /// Everything that calls the function whose name is at (line, col).
    pub fn incoming_calls(&self, path: &Path, line: u32, col: u32) -> Result<Vec<CallEdge>> {
        let overlay = self
            .overlay_for_path(path)
            .or_else(|| self.overlay_for_path(&self.workspace_root));
        let pos = self.file_position(path, line, col)?;
        let config = CallHierarchyConfig {
            exclude_tests: false,
            ra_fixture: RaFixtureConfig::default(),
        };
        let calls = self
            .analysis()?
            .incoming_calls(&config, pos)?
            .unwrap_or_default();
        let mut edges = self.call_edges(calls, overlay.as_ref());
        // Callers that disappear when tests are excluded are the tests.
        let without_tests = CallHierarchyConfig {
            exclude_tests: true,
            ra_fixture: RaFixtureConfig::default(),
        };
        let calls_without_tests = self
            .analysis()?
            .incoming_calls(&without_tests, pos)?
            .unwrap_or_default();
        let non_test: HashSet<(PathBuf, u32, u32)> = self
            .call_edges(calls_without_tests, overlay.as_ref())
            .into_iter()
            .map(|e| (e.item.path, e.item.line, e.item.col))
            .collect();
        for edge in &mut edges {
            edge.is_test =
                !non_test.contains(&(edge.item.path.clone(), edge.item.line, edge.item.col));
        }
        Ok(edges)
    }

    /// Everything the function whose name is at (line, col) calls.
    pub fn outgoing_calls(&self, path: &Path, line: u32, col: u32) -> Result<Vec<CallEdge>> {
        let overlay = self
            .overlay_for_path(path)
            .or_else(|| self.overlay_for_path(&self.workspace_root));
        let pos = self.file_position(path, line, col)?;
        let config = CallHierarchyConfig {
            exclude_tests: false,
            ra_fixture: RaFixtureConfig::default(),
        };
        let calls = self
            .analysis()?
            .outgoing_calls(&config, pos)?
            .unwrap_or_default();
        Ok(self.call_edges(calls, overlay.as_ref()))
    }

    fn call_edges(
        &self,
        calls: Vec<ra_ap_ide::CallItem>,
        overlay: Option<&Overlay>,
    ) -> Vec<CallEdge> {
        calls
            .iter()
            .filter(|call| self.is_in_view(call.target.file_id, overlay))
            .filter_map(|call| {
                let item = self.hierarchy_item_in_view(&call.target, overlay)?;
                let call_sites = call
                    .ranges
                    .iter()
                    .filter(|range| self.is_in_view(range.file_id, overlay))
                    .filter_map(|range| {
                        let text = self.analysis().ok()?.file_text(range.file_id).ok()?;
                        Some(offset_to_line_col(&text, range.range.start()))
                    })
                    .collect();
                Some(CallEdge {
                    item,
                    call_sites,
                    is_test: false,
                })
            })
            .collect()
    }

    /// Workspace-wide symbol search by name (what `workspace/symbol` answers): fuzzy on the
    /// name, associated items included. Positions point at the name.
    pub fn workspace_symbols(&self, query: &str, limit: usize) -> Result<Vec<WorkspaceSymbol>> {
        let overlay = self.overlay_for_path(&self.workspace_root);
        let limit = limit.max(1);
        let mut exact_local = ra_ap_ide::Query::new(query.to_string());
        exact_local.exact();
        let exact_targets = self.analysis()?.symbol_search(exact_local, limit)?;
        let mut out: Vec<WorkspaceSymbol> = exact_targets
            .into_iter()
            .filter(|target| self.is_in_view(target.file_id, overlay.as_ref()))
            .filter_map(|target| self.workspace_symbol(target, None))
            .collect();
        let local = ra_ap_ide::Query::new(query.to_string());
        let local_targets = self.analysis()?.symbol_search(local, limit)?;
        for symbol in local_targets
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
        if out.is_empty()
            && let Some((qual_prefix, item_name)) = query.rsplit_once("::")
            && !item_name.is_empty()
        {
            let quals: Vec<String> = qual_prefix
                .split("::")
                .filter(|s| !s.is_empty())
                .map(|s| s.trim().to_ascii_lowercase())
                .collect();
            let mut item_query = ra_ap_ide::Query::new(item_name.to_string());
            item_query.exact();
            let mut candidates = self.analysis()?.symbol_search(item_query, limit.max(200))?;
            if candidates.is_empty() {
                candidates = self
                    .analysis()?
                    .symbol_search(ra_ap_ide::Query::new(item_name.to_string()), limit.max(200))?;
            }
            for target in candidates
                .into_iter()
                .filter(|target| self.is_in_view(target.file_id, overlay.as_ref()))
            {
                let module = self.module_path(&target);
                let Some(symbol) = self.workspace_symbol(target, module) else {
                    continue;
                };
                if symbol.container.as_deref().is_some_and(|c| {
                    let segs: Vec<String> = c
                        .split([':', ' ', '<', '>'])
                        .filter(|s| !s.is_empty() && *s != "impl" && *s != "for")
                        .map(|s| s.to_ascii_lowercase())
                        .collect();
                    segs.len() >= quals.len() && segs[segs.len() - quals.len()..] == quals[..]
                }) {
                    out.push(symbol);
                    if out.len() >= limit {
                        break;
                    }
                }
            }
            if !out.is_empty() {
                return Ok(out);
            }
        }
        let query_lower = query.to_ascii_lowercase();
        if !out.is_empty()
            && (out.iter().any(|s| s.name.eq_ignore_ascii_case(query))
                || query.ends_with('_')
                || (query.len() >= 3
                    && out
                        .iter()
                        .any(|s| s.name.to_ascii_lowercase().starts_with(&query_lower))))
        {
            return Ok(out);
        }
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
        let targets = self.analysis()?.symbol_search(query, limit)?;
        if targets.is_empty() {
            return Ok(out);
        }
        let crate_names: HashMap<FileId, String> = self
            .analysis()?
            .fetch_crates()?
            .into_iter()
            .filter_map(|info| Some((info.root_file_id, info.name?.replace('-', "_"))))
            .collect();
        Ok(targets
            .into_iter()
            .filter(|target| self.is_in_view(target.file_id, overlay))
            .filter_map(|target| {
                let module = self.module_path(&target).or_else(|| {
                    let analysis = self.analysis().ok()?;
                    analysis
                        .crates_for(target.file_id)
                        .ok()?
                        .into_iter()
                        .find_map(|k| crate_names.get(&analysis.crate_root(k).ok()?))
                        .cloned()
                });
                self.workspace_symbol(target, module)
            })
            .collect())
    }

    /// `crate::module::path` a hit is declared in, from its moniker (#328, #1109).
    fn module_path(&self, target: &NavigationTarget) -> Option<String> {
        let focus = target.focus_range.unwrap_or(target.full_range);
        let position = FilePosition {
            file_id: target.file_id,
            offset: focus.start(),
        };
        let monikers = self.analysis().ok()?.moniker(position).ok()??.info;
        monikers.into_iter().find_map(|result| {
            let MonikerResult::Moniker(moniker) = result else {
                return None;
            };
            let identifier = moniker.identifier;
            let mut path = identifier.crate_name;
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
        let text = self.analysis().ok()?.file_text(target.file_id).ok()?;
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
