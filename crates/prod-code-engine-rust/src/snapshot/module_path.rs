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
use anyhow::Result;
use ra_ap_ide::{FilePosition, FileStructureConfig, MonikerResult};
use std::path::Path;

impl RustEngineSnapshot {
    /// Semantic containing module, including the crate name. Filenames cannot establish it:
    /// `#[path]` declarations and inline modules need not follow the directory layout.
    pub fn module_path_for_file(&self, path: &Path) -> Result<Option<String>> {
        let file_id = self.require_file_id(path)?;
        let nodes = self.analysis()?.file_structure(
            &FileStructureConfig {
                exclude_locals: true,
            },
            file_id,
        )?;
        for node in nodes.iter().filter(|node| node.parent.is_none()) {
            if let Some(module) = self.moniker_container(FilePosition {
                file_id,
                offset: node.navigation_range.start(),
            }) {
                return Ok(Some(module));
            }
        }
        // An empty child module still has a parent declaration. Its moniker includes the
        // child name, whereas monikers of declarations in this file end in the item name.
        let parents = self.analysis()?.parent_module(FilePosition {
            file_id,
            offset: 0.into(),
        })?;
        for parent in parents {
            let position = FilePosition {
                file_id: parent.file_id,
                offset: parent.focus_range.unwrap_or(parent.full_range).start(),
            };
            if let Some(module) = self.moniker_container(position) {
                return Ok(Some(format!("{module}::{}", parent.name)));
            }
        }
        Ok(None)
    }

    pub(super) fn moniker_container(&self, position: FilePosition) -> Option<String> {
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
}
