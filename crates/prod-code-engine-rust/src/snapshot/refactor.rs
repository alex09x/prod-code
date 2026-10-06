/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Refactoring operations: renaming, code actions/assists, and safe item deletion.

use anyhow::Result;
use ra_ap_ide::{
    AssistResolveStrategy, FileRange, FileStructureConfig, RenameConfig, SingleResolve, TextRange,
};
use ra_ap_ide_db::source_change::{FileSystemEdit, SourceChange};
use std::path::Path;

use super::RustEngineSnapshot;
use crate::types::{AssistInfo, FileMove, RefactorOutcome, RewrittenFile};
use crate::vfs::normalize_vfs_path;

impl RustEngineSnapshot {
    /// Renames the symbol at 1-based (line, col) to `new_name` across the workspace.
    /// `Ok(Err(reason))` means rust-analyzer refused (no symbol there, invalid name, conflict).
    pub fn rename(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        new_name: &str,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        let position = self.file_position(path, line, col)?;
        let config = RenameConfig {
            show_conflicts: true,
            prefer_no_std: false,
            prefer_prelude: true,
            prefer_absolute: false,
        };
        let change = match self.analysis.rename(position, new_name, &config)? {
            Ok(change) => change,
            Err(refused) => return Ok(Err(refused.to_string())),
        };
        Ok(Ok(self.outcome_from_change(&change)?))
    }

    /// Turns a rust-analyzer `SourceChange` into rewritten files, created files and moves.
    pub(crate) fn outcome_from_change(&self, change: &SourceChange) -> Result<RefactorOutcome> {
        let mut outcome = RefactorOutcome::default();
        for (edited_id, (edit, _snippet)) in change.source_file_edits.iter() {
            let Some(edited_path) = self.path_for_file_id(*edited_id) else {
                continue;
            };
            let old = self.analysis.file_text(*edited_id)?;
            let mut new_text = old.to_string();
            let edits = edit.iter().count();
            edit.apply(&mut new_text);
            outcome.files.push(RewrittenFile {
                path: edited_path,
                new_text,
                edits,
                old_line_count: old.lines().count() as u32,
            });
        }
        for fs_edit in &change.file_system_edits {
            match fs_edit {
                FileSystemEdit::MoveFile { src, dst } => {
                    if let (Some(from), Some(to)) =
                        (self.path_for_file_id(*src), self.anchored_path(dst))
                    {
                        outcome.moves.push(FileMove { from, to });
                    }
                }
                FileSystemEdit::MoveDir { src, dst, .. } => {
                    if let (Some(from), Some(to)) =
                        (self.anchored_path(src), self.anchored_path(dst))
                    {
                        outcome.moves.push(FileMove { from, to });
                    }
                }
                FileSystemEdit::CreateFile {
                    dst,
                    initial_contents,
                } => {
                    if let Some(path) = self.anchored_path(dst) {
                        outcome.created.push(RewrittenFile {
                            path,
                            new_text: initial_contents.clone(),
                            edits: 1,
                            old_line_count: 0,
                        });
                    }
                }
            }
        }
        outcome.files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(outcome)
    }

    pub(crate) fn file_range(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        end: Option<(u32, u32)>,
    ) -> Result<FileRange> {
        let start = self.file_position(path, line, col)?;
        let end = match end {
            Some((line, col)) => self.file_position(path, line, col)?.offset,
            None => start.offset,
        };
        let file_id = start.file_id;
        let start = start.offset;
        let (start, end) = if end < start {
            (end, start)
        } else {
            (start, end)
        };
        Ok(FileRange {
            file_id,
            range: TextRange::new(start, end),
        })
    }

    /// Code actions available at a 1-based position or selection (`end` inclusive-exclusive).
    pub fn list_assists(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        end: Option<(u32, u32)>,
    ) -> Result<Vec<AssistInfo>> {
        let frange = self.file_range(path, line, col, end)?;
        let (assist_config, diagnostics_config) = Self::assist_configs();
        let assists = self.analysis.assists_with_fixes(
            &assist_config,
            &diagnostics_config,
            AssistResolveStrategy::None,
            frange,
        )?;
        Ok(assists
            .into_iter()
            .map(|a| AssistInfo {
                id: a.id.0.to_string(),
                kind: format!("{:?}", a.id.1),
                subtype: a.id.2,
                label: a.label.to_string(),
                group: a.group.map(|g| g.0),
            })
            .collect())
    }

    /// Computes the edit of one assist (by `id` and optional `subtype`) at the position.
    /// `Ok(Err(reason))` when no such assist is offered there.
    pub fn apply_assist(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        end: Option<(u32, u32)>,
        id: &str,
        subtype: Option<usize>,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        let frange = self.file_range(path, line, col, end)?;
        let (assist_config, diagnostics_config) = Self::assist_configs();
        let offered = self.analysis.assists_with_fixes(
            &assist_config,
            &diagnostics_config,
            AssistResolveStrategy::None,
            frange,
        )?;
        let Some(target) = offered
            .iter()
            .find(|a| a.id.0 == id && (subtype.is_none() || a.id.2 == subtype))
        else {
            let available: Vec<String> = offered.iter().map(|a| a.id.0.to_string()).collect();
            let available = if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            };
            // Inside a macro call's input rust-analyzer sees tokens more than code, and most
            // refactorings are not offered there; "not offered here" alone leaves the reader to
            // guess why (#99).
            if let Some(mac) = self.macro_call_around(frange) {
                return Ok(Err(format!(
                    "assist `{id}` is not offered here: the selection is inside `{mac}!`, and \
                     rust-analyzer does not offer `{id}` inside a macro call's input. Move the \
                     code out of the macro (into a function the macro calls) first. Available \
                     here: {available}"
                )));
            }
            return Ok(Err(format!(
                "assist `{id}` is not offered here; available: {available}"
            )));
        };
        let resolve = AssistResolveStrategy::Single(SingleResolve {
            assist_id: target.id.0.to_string(),
            assist_kind: target.id.1,
            assist_subtype: target.id.2,
        });
        let resolved = self.analysis.assists_with_fixes(
            &assist_config,
            &diagnostics_config,
            resolve,
            frange,
        )?;
        let Some(change) = resolved
            .into_iter()
            .find(|a| a.id.0 == id && (subtype.is_none() || a.id.2 == subtype))
            .and_then(|a| a.source_change)
        else {
            return Ok(Err(format!("assist `{id}` produced no edit")));
        };
        Ok(Ok(self.outcome_from_change(&change)?))
    }

    /// The path of the innermost macro call whose input contains `frange`, if any.
    fn macro_call_around(&self, frange: FileRange) -> Option<String> {
        use ra_ap_syntax::AstNode;
        let file = self.analysis.parse(frange.file_id).ok()?;
        let token = file
            .syntax()
            .token_at_offset(frange.range.start())
            .right_biased()?;
        token
            .parent_ancestors()
            .filter_map(ra_ap_syntax::ast::MacroCall::cast)
            .find(|call| {
                call.token_tree()
                    .is_some_and(|tree| tree.syntax().text_range().contains_range(frange.range))
            })
            .and_then(|call| call.path())
            .map(|path| path.syntax().text().to_string())
    }

    /// Deletes the item (function, type, const, field, module …) whose name is at the 1-based
    /// position, but only when nothing else in the workspace references it.
    /// `Ok(Err(dossier))` lists the usages that block the deletion.
    pub fn safe_delete(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        let position = self.file_position(path, line, col)?;
        let usages = self.find_all_refs(path, line, col)?;
        if !usages.is_empty() {
            let mut listed: Vec<String> = usages
                .iter()
                .take(20)
                .map(|u| format!("{}:{}:{}", u.path.display(), u.line, u.col))
                .collect();
            if usages.len() > listed.len() {
                listed.push(format!("… {} more", usages.len() - listed.len()));
            }
            return Ok(Err(format!(
                "{} usage(s) reference this item; delete refused:\n{}",
                usages.len(),
                listed.join("\n")
            )));
        }
        let file_id = position.file_id;
        let text = self.analysis.file_text(file_id)?;
        let offset = position.offset;
        let config = FileStructureConfig {
            exclude_locals: false,
        };
        let nodes = self.analysis.file_structure(&config, file_id)?;
        // Only the item whose name is at the position. Falling back to the smallest item that
        // merely contains it deleted a line of a function's body for a position on a parameter
        // (#138): a position that names no item is refused instead.
        let node = nodes
            .iter()
            .filter(|n| n.navigation_range.contains_inclusive(offset))
            .min_by_key(|n| n.node_range.len());
        let Some(node) = node else {
            return Ok(Err(
                "no deletable item is named at this position; give the position of an item's \
                 name (a parameter is removed with `code_change_signature`)"
                    .to_string(),
            ));
        };
        let start = usize::from(node.node_range.start());
        let mut end = usize::from(node.node_range.end());
        // Take the line terminator with the item, and one blank line if that leaves two.
        if text[end..].starts_with('\n') {
            end += 1;
        }
        let mut new_text = String::with_capacity(text.len());
        new_text.push_str(&text[..start]);
        new_text.push_str(&text[end..]);
        let new_text = new_text.replace("\n\n\n", "\n\n");
        let path_in_view = self
            .path_for_file_id(file_id)
            .unwrap_or_else(|| normalize_vfs_path(path, &self.workspace_root));
        Ok(Ok(RefactorOutcome {
            files: vec![RewrittenFile {
                path: path_in_view,
                new_text,
                edits: 1,
                old_line_count: text.lines().count() as u32,
            }],
            created: Vec::new(),
            moves: Vec::new(),
        }))
    }
}
