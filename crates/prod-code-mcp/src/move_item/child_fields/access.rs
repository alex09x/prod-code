/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Result;
use ra_ap_syntax::{AstNode, SourceFile, SyntaxKind, SyntaxNode, SyntaxToken, ast, ast::HasAttrs};

use super::macros::inside_token_tree;
use super::{NeededFields, PrivateFields, canonical_ident};
use crate::move_item::budget::MoveBudget;
use crate::move_item::item::display;

pub(super) enum PendingAccess {
    Record {
        canonical: String,
        raw: String,
        pos: (u32, u32),
    },
    Tuple {
        index: usize,
        pos: (u32, u32),
        under_cfg: bool,
    },
}

pub(super) fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .ok()
            .zip(b.canonicalize().ok())
            .is_some_and(|(ca, cb)| ca == cb)
}

pub(super) fn collect_dot_field_candidates(
    text: &str,
    tree: &SourceFile,
    in_scope: &impl Fn(usize) -> bool,
    is_self_of_s: &impl Fn(&SyntaxNode) -> bool,
    shape: &PrivateFields,
    needed: &mut NeededFields,
) -> Result<Vec<PendingAccess>> {
    let tokens: Vec<SyntaxToken> = tree
        .syntax()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect();
    let mut pending = Vec::new();

    for (idx, tok) in tokens.iter().enumerate() {
        if needed.is_complete(shape) {
            break;
        }
        if tok.kind() != SyntaxKind::DOT || !in_scope(usize::from(tok.text_range().start())) {
            continue;
        }
        let Some(next) = tokens.get(idx + 1) else {
            continue;
        };
        let after = tokens.get(idx + 2).map(|t| t.text());
        match shape {
            PrivateFields::Record { fields, .. } if next.kind() == SyntaxKind::IDENT => {
                if matches!(after, Some("(" | "::")) {
                    continue;
                }
                let ident = canonical_ident(next.text());
                if needed.record_names.contains(&ident)
                    || !fields.iter().any(|f| f.canonical == ident)
                {
                    continue;
                }
                if is_direct_self_field_expr(next, is_self_of_s) {
                    needed.record_names.insert(ident);
                    continue;
                }
                let pos =
                    crate::signature::position_at(text, usize::from(next.text_range().start()))?;
                pending.push(PendingAccess::Record {
                    canonical: ident,
                    raw: next.text().to_string(),
                    pos,
                });
            }
            PrivateFields::Tuple { fields }
                if matches!(
                    next.kind(),
                    SyntaxKind::INT_NUMBER | SyntaxKind::FLOAT_NUMBER
                ) =>
            {
                for (tuple_idx, byte_at) in parse_tuple_indices(next) {
                    if needed.tuple_indices.contains(&tuple_idx) {
                        continue;
                    }
                    if !fields.iter().any(|f| f.index == tuple_idx) {
                        continue;
                    }
                    if next.kind() == SyntaxKind::INT_NUMBER
                        && is_direct_self_field_expr(next, is_self_of_s)
                    {
                        needed.tuple_indices.insert(tuple_idx);
                        continue;
                    }
                    let pos = crate::signature::position_at(text, byte_at)?;
                    pending.push(PendingAccess::Tuple {
                        index: tuple_idx,
                        pos,
                        under_cfg: has_cfg_ancestor(next),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(pending)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn resolve_pending_accesses(
    budget: &MoveBudget,
    remote: SocketAddr,
    root: &Path,
    source_file: &Path,
    scope_path: &Path,
    struct_name: &str,
    shape: &PrivateFields,
    pending: Vec<PendingAccess>,
    needed: &mut NeededFields,
) -> Result<()> {
    for item in pending {
        if needed.is_complete(shape) {
            break;
        }
        match (item, shape) {
            (
                PendingAccess::Record {
                    canonical,
                    raw,
                    pos,
                },
                PrivateFields::Record { fields, .. },
            ) => {
                if needed.record_names.contains(&canonical) {
                    continue;
                }
                let defs = resolve_definitions(budget, remote, root, scope_path, pos).await?;
                if defs.iter().any(|(def_path, l, c)| {
                    same_file(def_path, source_file)
                        && fields
                            .iter()
                            .any(|f| f.canonical == canonical && f.def_line_col == (*l, *c))
                }) {
                    needed.record_names.insert(canonical);
                } else if defs.is_empty() {
                    anyhow::bail!(
                        "cannot prove whether {}:{}:{} accesses private field `{struct_name}::{raw}` \
                         because its receiver type could not be resolved; nothing was written",
                        display(root, scope_path),
                        pos.0,
                        pos.1
                    );
                }
            }
            (
                PendingAccess::Tuple {
                    index,
                    pos,
                    under_cfg,
                },
                PrivateFields::Tuple { fields },
            ) => {
                if needed.tuple_indices.contains(&index) {
                    continue;
                }
                let Some(priv_field) = fields.iter().find(|f| f.index == index) else {
                    continue;
                };
                let defs = resolve_definitions(budget, remote, root, scope_path, pos).await?;
                if defs.iter().any(|(def_path, l, c)| {
                    same_file(def_path, source_file)
                        && ((*l, *c) == priv_field.def_line_col
                            || (*l, *c) == priv_field.ty_line_col)
                }) {
                    needed.tuple_indices.insert(index);
                } else if defs.is_empty() && under_cfg {
                    anyhow::bail!(
                        "cannot prove whether {}:{}:{} accesses private tuple field \
                         `{struct_name}.{index}` under conditional compilation; nothing was written",
                        display(root, scope_path),
                        pos.0,
                        pos.1
                    );
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_direct_self_field_expr(
    tok: &SyntaxToken,
    is_self_of_s: &impl Fn(&SyntaxNode) -> bool,
) -> bool {
    let Some(parent) = tok.parent() else {
        return false;
    };
    if inside_token_tree(&parent) {
        return false;
    }
    let Some(field_expr) = parent.ancestors().take(2).find_map(ast::FieldExpr::cast) else {
        return false;
    };
    if !is_self_of_s(field_expr.syntax()) {
        return false;
    }
    let mut expr = field_expr.expr();
    while let Some(current) = expr {
        match current {
            ast::Expr::ParenExpr(p) => expr = p.expr(),
            ast::Expr::RefExpr(r) => expr = r.expr(),
            ast::Expr::PrefixExpr(p) if p.op_kind() == Some(ast::UnaryOp::Deref) => {
                expr = p.expr();
            }
            ast::Expr::PathExpr(p) => {
                return p
                    .path()
                    .and_then(|path| path.as_single_name_ref())
                    .is_some_and(|nr| nr.text() == "self");
            }
            _ => return false,
        }
    }
    false
}

fn parse_tuple_indices(tok: &SyntaxToken) -> Vec<(usize, usize)> {
    let start = usize::from(tok.text_range().start());
    let raw = tok.text();
    if let Some((left, right)) = raw.split_once('.') {
        let mut out = Vec::new();
        if let Ok(a) = left.parse::<usize>() {
            out.push((a, start));
        }
        if let Ok(b) = right.parse::<usize>() {
            out.push((b, start + left.len() + 1));
        }
        out
    } else {
        raw.parse::<usize>()
            .ok()
            .into_iter()
            .map(|i| (i, start))
            .collect()
    }
}

fn has_cfg_ancestor(tok: &SyntaxToken) -> bool {
    let Some(parent) = tok.parent() else {
        return false;
    };
    parent.ancestors().any(|anc| {
        ast::AnyHasAttrs::cast(anc).is_some_and(|item| {
            item.attrs().any(|attr| {
                attr.simple_name()
                    .is_some_and(|n| n == "cfg" || n == "cfg_attr")
            })
        })
    })
}

async fn resolve_definitions(
    budget: &MoveBudget,
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    (line, col): (u32, u32),
) -> Result<Vec<(PathBuf, u32, u32)>> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
        .to_string();
    let res = budget
        .run(
            "resolving struct field access",
            crate::tools::execute_lsp_query(
                remote,
                root,
                file,
                "textDocument/definition",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": {
                        "line": line.saturating_sub(1),
                        "character": col.saturating_sub(1),
                    },
                }),
            ),
        )
        .await?;
    if res.is_null() {
        return Ok(Vec::new());
    }
    let entries = if let Some(arr) = res.as_array() {
        arr.as_slice()
    } else {
        std::slice::from_ref(&res)
    };
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(target_uri) = entry
            .get("targetUri")
            .or_else(|| entry.get("uri"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        let Some(start) = entry
            .pointer("/targetSelectionRange/start")
            .or_else(|| entry.pointer("/targetRange/start"))
            .or_else(|| entry.pointer("/range/start"))
        else {
            continue;
        };
        let (Some(l), Some(c)) = (
            start
                .get("line")
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok()),
            start
                .get("character")
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok()),
        ) else {
            continue;
        };
        if let (Some(path), Some(line), Some(col)) = (
            crate::signature::local_file(target_uri),
            l.checked_add(1),
            c.checked_add(1),
        ) {
            out.push((path, line, col));
        }
    }
    Ok(out)
}
