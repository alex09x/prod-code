/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::RustEngineSnapshot;
use crate::types::WorkspaceSymbol;
use anyhow::Result;

impl RustEngineSnapshot {
    /// Explicit library scope, used only after qualified workspace resolution fails.
    /// Searching the bare member keeps `Type::method` meaningful to rust-analyzer's index.
    pub fn dependency_symbols(&self, query: &str, limit: usize) -> Result<Vec<WorkspaceSymbol>> {
        let parts: Vec<_> = query.split("::").filter(|part| !part.is_empty()).collect();
        let Some(name) = parts.last() else {
            return Ok(Vec::new());
        };
        let owners = &parts[..parts.len() - 1];
        let overlay = self.overlay_for_path(&self.workspace_root);
        let limit = limit.max(1);
        let mut exact = ra_ap_ide::Query::new((*name).to_string());
        exact.libs();
        exact.exact();
        let mut hits = self.library_symbols(exact, limit.max(200), overlay.as_ref())?;
        hits.retain(|hit| owner_matches(hit.container.as_deref(), owners));
        if hits.is_empty() {
            let mut fuzzy = ra_ap_ide::Query::new((*name).to_string());
            fuzzy.libs();
            hits = self.library_symbols(fuzzy, limit.max(200), overlay.as_ref())?;
            hits.retain(|hit| owner_matches(hit.container.as_deref(), owners));
        }
        if !hits.iter().any(|hit| hit.name.eq_ignore_ascii_case(name))
            && let Some(owner) = owners.last()
        {
            // A miss should suggest the dependency owner's methods, even for a typo the
            // fuzzy index cannot match. Inspect a bounded number of declaring files.
            let mut types = ra_ap_ide::Query::new((*owner).to_string());
            types.libs();
            types.exact();
            let types = self.library_symbols(types, 50, overlay.as_ref())?;
            for ty in types
                .into_iter()
                .filter(|ty| {
                    ty.name.eq_ignore_ascii_case(owner)
                        && owner_matches(ty.container.as_deref(), &owners[..owners.len() - 1])
                })
                .take(8)
            {
                for item in self.document_symbols(&ty.path)? {
                    if !matches!(item.kind.as_str(), "Function" | "Method") {
                        continue;
                    }
                    let position = self.file_position(&ty.path, item.line, item.col)?;
                    let container = self.moniker_container(position);
                    if !owner_matches(container.as_deref(), owners) {
                        continue;
                    }
                    let hit = WorkspaceSymbol {
                        path: ty.path.clone(),
                        name: item.name,
                        kind: item.kind,
                        line: item.line,
                        col: item.col,
                        end_line: item.end_line,
                        container,
                    };
                    if !hits.iter().any(|listed| {
                        listed.path == hit.path && listed.line == hit.line && listed.col == hit.col
                    }) {
                        hits.push(hit);
                    }
                }
            }
            hits.sort_by(|a, b| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));
        }
        hits.truncate(limit);
        Ok(hits)
    }
}

fn owner_matches(container: Option<&str>, owners: &[&str]) -> bool {
    if owners.is_empty() {
        return true;
    }
    let segments: Vec<_> = container.unwrap_or_default().split("::").collect();
    segments.len() >= owners.len()
        && segments[segments.len() - owners.len()..]
            .iter()
            .zip(owners)
            .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
}
