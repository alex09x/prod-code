/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::shadow::{HypothesisEdit, HypothesisSpec};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

/// The single operation ledger used by every compiler request. None means absence.
pub(super) struct ValidationProposal {
    pub spec: HypothesisSpec,
}

impl ValidationProposal {
    pub fn from_texts(root: &Path, texts: &[(PathBuf, String)]) -> Result<Self> {
        Self::new(
            root,
            texts.iter().map(|(p, t)| (p.clone(), Some(t.clone()))),
        )
    }

    fn new(
        root: &Path,
        operations: impl IntoIterator<Item = (PathBuf, Option<String>)>,
    ) -> Result<Self> {
        let mut edits: Vec<HypothesisEdit> = Vec::new();
        for (path, text) in operations {
            let relative_path = crate::shadow::relative_edit_path(root, &path)?;
            if text.is_some() {
                ensure!(
                    !root.join(&relative_path).is_symlink(),
                    "replacement of a leaf symlink is unsupported"
                );
            }
            ensure!(
                edits.iter().all(|e| {
                    let a = Path::new(&e.relative_path);
                    let b = Path::new(&relative_path);
                    !a.starts_with(b) && !b.starts_with(a)
                }),
                "duplicate/conflicting proposal operations: {relative_path}"
            );
            edits.push(HypothesisEdit {
                relative_path,
                text,
            });
        }
        Ok(Self {
            spec: HypothesisSpec {
                name: "proposed".into(),
                edits,
            },
        })
    }

    pub fn parse(root: &Path, args: &serde_json::Value) -> Result<Self> {
        if let Some(diff) = args.get("diff").and_then(|v| v.as_str()) {
            let paths = super::validation_diff::paths(diff)?;
            // Confinement and conflict checks happen before the patch parser reads files.
            Self::new(
                root,
                paths
                    .into_iter()
                    .map(|(p, deleted)| (p, (!deleted).then(String::new))),
            )?;
            let patched = crate::patch::apply(root, diff)?;
            return Self::new(
                root,
                patched
                    .texts
                    .into_iter()
                    .map(|(p, t)| (p, Some(t)))
                    .chain(patched.deleted.into_iter().map(|p| (p, None))),
            );
        }
        if let Some(edit) = args.get("workspace_edit") {
            // The preview cannot preserve ordered resource operations. Refuse the entire
            // proposal before applying text edits or invoking an analyzer.
            if let Some(ops) = edit.get("documentChanges").and_then(|v| v.as_array()) {
                ensure!(
                    ops.iter().all(|op| op.get("kind").is_none()),
                    "WorkspaceEdit resource operations are unsupported; complete proposal not compiler-verified; no validation was performed"
                );
            }
            let (texts, resources) = crate::refactor::planned_texts(root, edit)?;
            ensure!(
                !resources,
                "WorkspaceEdit resource operations not compiler-verified"
            );
            return Self::from_texts(root, &texts);
        }
        let texts = args
            .get("edits")
            .and_then(|v| v.as_array())
            .context("Missing 'edits' argument (or diff, or workspace_edit)")?
            .iter()
            .map(|edit| {
                let path = edit
                    .get("path")
                    .and_then(|v| v.as_str())
                    .context("edit without path")?;
                let text = edit
                    .get("new_text")
                    .and_then(|v| v.as_str())
                    .context("edit without new_text")?;
                Ok((super::resolve_file_path(root, path), text.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;
        Self::from_texts(root, &texts)
    }

    pub fn texts(&self, root: &Path) -> Vec<(PathBuf, String)> {
        self.spec
            .edits
            .iter()
            .filter_map(|e| {
                e.text
                    .as_ref()
                    .map(|t| (root.join(&e.relative_path), t.clone()))
            })
            .collect()
    }

    pub fn notes(&self) -> &'static str {
        if self.spec.edits.iter().any(|e| e.text.is_none()) {
            "analyzer: file deletions and their effects on remaining files are not checked; compile checks the complete supported proposal in a shadow\n"
        } else {
            ""
        }
    }
}
