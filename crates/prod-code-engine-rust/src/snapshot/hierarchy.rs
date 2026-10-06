/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Call hierarchy queries (incoming calls, outgoing calls).

use anyhow::Result;
use ra_ap_ide::{CallHierarchyConfig, RaFixtureConfig};
use ra_ap_load_cargo::worktree::Overlay;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::RustEngineSnapshot;
use crate::types::{CallEdge, HierarchyItem};
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
        let targets = match self.analysis.call_hierarchy(pos, &config)? {
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
            .analysis
            .incoming_calls(&config, pos)?
            .unwrap_or_default();
        let mut edges = self.call_edges(calls, overlay.as_ref());
        // Callers that disappear when tests are excluded are the tests.
        let without_tests = CallHierarchyConfig {
            exclude_tests: true,
            ra_fixture: RaFixtureConfig::default(),
        };
        let non_test: HashSet<(PathBuf, u32, u32)> = self
            .call_edges(
                self.analysis
                    .incoming_calls(&without_tests, pos)?
                    .unwrap_or_default(),
                overlay.as_ref(),
            )
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
            .analysis
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
                        let text = self.analysis.file_text(range.file_id).ok()?;
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
}
