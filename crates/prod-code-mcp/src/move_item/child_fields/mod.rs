/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Minimum parent-visibility preservation for private struct fields moved to a child module (#1816).
//!
//! In Rust, a field without an explicit visibility modifier in module `M` has visibility
//! `pub(in M)` (accessible in `M` and its descendant submodules). Moving the struct into direct
//! child module `M::child` narrows any unannotated field to `M::child`. Widening only the private
//! fields actually accessed in `M`'s non-destination subtree to `pub(super)` restores their exact
//! original `pub(in M)` visibility without exposing unused fields or widening to `pub(crate)` /
//! `pub`. When minimal field usage cannot be proven before writing (such as opaque macros or
//! unresolved receiver types), planning refuses before modifying any file.

mod access;
mod macros;
mod scan;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ra_ap_syntax::{AstNode, Edition, ast, ast::HasName, ast::HasVisibility};

use super::budget::MoveBudget;
use super::item::{display, offset_of};
use super::plan::ItemPlan;
use super::rust_syntax;

#[derive(Debug, Clone)]
pub(super) struct PrivateRecordField {
    pub canonical: String,
    pub def_line_col: (u32, u32),
}

#[derive(Debug, Clone)]
pub(super) struct PrivateTupleField {
    pub index: usize,
    pub def_line_col: (u32, u32),
    pub ty_line_col: (u32, u32),
}

#[derive(Debug, Clone)]
pub(super) enum PrivateFields {
    Record {
        unique_names: BTreeSet<String>,
        fields: Vec<PrivateRecordField>,
    },
    Tuple {
        fields: Vec<PrivateTupleField>,
    },
}

#[derive(Debug, Default)]
pub(super) struct NeededFields {
    pub record_names: BTreeSet<String>,
    pub tuple_indices: BTreeSet<usize>,
}

impl NeededFields {
    pub(super) fn mark_all(&mut self, shape: &PrivateFields) {
        match shape {
            PrivateFields::Record { unique_names, .. } => {
                self.record_names.extend(unique_names.iter().cloned());
            }
            PrivateFields::Tuple { fields } => {
                self.tuple_indices.extend(fields.iter().map(|f| f.index));
            }
        }
    }

    pub(super) fn is_complete(&self, shape: &PrivateFields) -> bool {
        match shape {
            PrivateFields::Record { unique_names, .. } => {
                self.record_names.len() == unique_names.len()
            }
            PrivateFields::Tuple { fields } => self.tuple_indices.len() == fields.len(),
        }
    }
}

pub(super) fn canonical_ident(raw: &str) -> String {
    raw.strip_prefix("r#").unwrap_or(raw).to_string()
}

/// Widens only the private fields of a moved `struct` that are accessed in the parent module's
/// remaining privacy subtree to `pub(super)`. Non-struct items and structs without private fields
/// pass through unchanged.
#[allow(clippy::too_many_arguments)]
pub(super) async fn widen_accessed_fields(
    budget: &MoveBudget,
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    source_text: &str,
    (start, decl_start, decl_end): (u32, u32, u32),
    struct_name: &str,
    target: &Path,
    plan: &ItemPlan,
    struct_refs: &[(PathBuf, u32, u32)],
    rebased_item: &str,
    edition: Edition,
) -> Result<String> {
    let Some(shape) = inspect_private_fields(source_text, decl_start, struct_name, edition)? else {
        return Ok(rebased_item.to_string());
    };
    ensure_direct_child_owners(root, file, target, struct_name, plan)?;
    let scope_files = collect_parent_privacy_scopes(struct_name, plan)?;

    let cut_start = offset_of(source_text, start, 1).unwrap_or(0);
    let cut_end = offset_of(source_text, decl_end + 1, 1).unwrap_or(source_text.len());
    let mut needed = NeededFields::default();

    for (scope_path, scope_ranges) in &scope_files {
        let owned;
        let text = if scope_path == file {
            source_text
        } else {
            owned = rust_syntax::read(scope_path)?;
            owned.as_str()
        };
        let file_edition = plan
            .graph
            .nodes
            .iter()
            .find(|n| n.file == *scope_path)
            .map_or(edition, |n| rust_syntax::edition(&n.root));
        let excluded = (scope_path == file).then_some(cut_start..cut_end);
        scan::scan_scope_file(
            budget,
            remote,
            root,
            file,
            scope_path,
            text,
            scope_ranges,
            excluded,
            struct_name,
            &shape,
            struct_refs,
            file_edition,
            &mut needed,
        )
        .await?;
    }

    apply_pub_super(rebased_item, edition, &needed)
}

fn inspect_private_fields(
    source_text: &str,
    decl_start: u32,
    struct_name: &str,
    edition: Edition,
) -> Result<Option<PrivateFields>> {
    let Some(node) = rust_syntax::item_at(source_text, decl_start, edition) else {
        return Ok(None);
    };
    let Some(item) = ast::Struct::cast(node) else {
        return Ok(None);
    };
    let Some(field_list) = item.field_list() else {
        return Ok(None);
    };
    match field_list {
        ast::FieldList::RecordFieldList(list) => {
            let mut fields = Vec::new();
            let mut unique_names = BTreeSet::new();
            for field in list.fields() {
                let has_default = field
                    .syntax()
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .any(|t| t.kind() == ra_ap_syntax::SyntaxKind::EQ);
                anyhow::ensure!(
                    !has_default,
                    "moving struct `{struct_name}` with default field values into a child module \
                     is unsupported; nothing was written"
                );
                if field.visibility().is_some() {
                    continue;
                }
                let name = field.name().context("Rust struct field name is missing")?;
                let canonical = canonical_ident(name.text());
                let def_line_col = crate::signature::position_at(
                    source_text,
                    rust_syntax::range(name.syntax()).start,
                )?;
                unique_names.insert(canonical.clone());
                fields.push(PrivateRecordField {
                    canonical,
                    def_line_col,
                });
            }
            if fields.is_empty() {
                Ok(None)
            } else {
                Ok(Some(PrivateFields::Record {
                    unique_names,
                    fields,
                }))
            }
        }
        ast::FieldList::TupleFieldList(list) => {
            let mut fields = Vec::new();
            for (index, field) in list.fields().enumerate() {
                if field.visibility().is_some() {
                    continue;
                }
                let ty = field.ty().context("Rust tuple field type is missing")?;
                let def_line_col = crate::signature::position_at(
                    source_text,
                    rust_syntax::range(field.syntax()).start,
                )?;
                let ty_line_col = crate::signature::position_at(
                    source_text,
                    rust_syntax::range(ty.syntax()).start,
                )?;
                fields.push(PrivateTupleField {
                    index,
                    def_line_col,
                    ty_line_col,
                });
            }
            if fields.is_empty() {
                Ok(None)
            } else {
                Ok(Some(PrivateFields::Tuple { fields }))
            }
        }
    }
}

fn ensure_direct_child_owners(
    root: &Path,
    file: &Path,
    target: &Path,
    struct_name: &str,
    plan: &ItemPlan,
) -> Result<()> {
    let aligned = plan.from.len() == plan.to.len()
        && plan.from.iter().all(|from_owner| {
            plan.to.iter().any(|to_owner| {
                to_owner.identity == from_owner.identity
                    && to_owner.module.krate == from_owner.module.krate
                    && to_owner.module.segments.len() == from_owner.module.segments.len() + 1
                    && to_owner
                        .module
                        .segments
                        .starts_with(&from_owner.module.segments)
            })
        });
    anyhow::ensure!(
        aligned,
        "cannot preserve exact parent field visibility for `{struct_name}`: `{}` is not a direct \
         child module of `{}` in every Cargo target; nothing was written",
        display(root, target),
        display(root, file)
    );
    Ok(())
}

fn collect_parent_privacy_scopes(
    struct_name: &str,
    plan: &ItemPlan,
) -> Result<BTreeMap<PathBuf, Vec<Range<usize>>>> {
    let mut scopes: BTreeMap<PathBuf, Vec<Range<usize>>> = BTreeMap::new();
    for node in &plan.graph.nodes {
        let in_parent_subtree = plan.from.iter().any(|from_owner| {
            node.identity == from_owner.identity
                && node.module.krate == from_owner.module.krate
                && node
                    .module
                    .segments
                    .starts_with(&from_owner.module.segments)
        });
        if !in_parent_subtree {
            continue;
        }
        let in_target_subtree = plan.to.iter().any(|to_owner| {
            node.identity == to_owner.identity
                && node.module.krate == to_owner.module.krate
                && node.module.segments.starts_with(&to_owner.module.segments)
        });
        if in_target_subtree {
            continue;
        }
        if node
            .declaration
            .as_ref()
            .is_some_and(|(_, decl)| decl.unresolvable_path)
            || !node.file.is_file()
        {
            anyhow::bail!(
                "cannot prove minimum field visibility for `{struct_name}` because submodule `{}` \
                 has an unresolvable or missing module file; nothing was written",
                node.module.absolute()
            );
        }
        let entries = scopes.entry(node.file.clone()).or_default();
        if !entries
            .iter()
            .any(|r| r.start <= node.scope.start && r.end >= node.scope.end)
        {
            entries.retain(|r| !(node.scope.start <= r.start && node.scope.end >= r.end));
            entries.push(node.scope.clone());
        }
    }
    Ok(scopes)
}

pub(super) fn apply_pub_super(
    rebased_item: &str,
    edition: Edition,
    needed: &NeededFields,
) -> Result<String> {
    if needed.record_names.is_empty() && needed.tuple_indices.is_empty() {
        return Ok(rebased_item.to_string());
    }
    let tree = rust_syntax::parse(rebased_item, edition);
    let item = tree
        .syntax()
        .children()
        .find_map(ast::Struct::cast)
        .context("rebased item is no longer a Rust struct")?;
    let mut insertions = Vec::new();
    match item.field_list() {
        Some(ast::FieldList::RecordFieldList(list)) => {
            for field in list.fields() {
                if field.visibility().is_some() {
                    continue;
                }
                let name = field.name().context("rebased field name is missing")?;
                if needed.record_names.contains(&canonical_ident(name.text())) {
                    insertions.push(rust_syntax::range(name.syntax()).start);
                }
            }
        }
        Some(ast::FieldList::TupleFieldList(list)) => {
            for (index, field) in list.fields().enumerate() {
                if field.visibility().is_some() {
                    continue;
                }
                if needed.tuple_indices.contains(&index) {
                    let ty = field.ty().context("rebased tuple field type is missing")?;
                    insertions.push(rust_syntax::range(ty.syntax()).start);
                }
            }
        }
        None => {}
    }
    insertions.sort_unstable();
    let mut out = rebased_item.to_string();
    for offset in insertions.into_iter().rev() {
        out.insert_str(offset, "pub(super) ");
    }
    Ok(out)
}
