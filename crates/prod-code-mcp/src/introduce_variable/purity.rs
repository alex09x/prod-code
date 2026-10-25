/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Small, affirmative purity proofs. An inferred result type alone says nothing about an
//! operator's operands, autoderef, or arbitrary trait implementations.

use super::syntax::{Syntax, ident};
use crate::signature::effects::{SCALARS, hover_is_builtin, hover_markdown};
use anyhow::{Context, Result, bail, ensure};
use std::{collections::BTreeMap, net::SocketAddr, path::Path};

#[derive(Clone)]
enum Value {
    Scalar,
    Record(usize),
    Reference(Box<Value>),
}

pub(super) struct Proof<'a, 's> {
    pub syntax: &'s Syntax<'a>,
    pub remote: SocketAddr,
    pub root: &'s Path,
    pub file: &'s Path,
    pub function: usize,
    hovers: BTreeMap<usize, Option<String>>,
    values: BTreeMap<usize, Value>,
}

impl<'a, 's> Proof<'a, 's> {
    pub fn new(
        syntax: &'s Syntax<'a>,
        remote: SocketAddr,
        root: &'s Path,
        file: &'s Path,
        function: usize,
    ) -> Self {
        Self {
            syntax,
            remote,
            root,
            file,
            function,
            hovers: BTreeMap::new(),
            values: BTreeMap::new(),
        }
    }

    pub async fn scalar(&mut self, node: usize) -> Result<()> {
        ensure!(
            matches!(self.value(node, 0).await?, Value::Scalar),
            "cannot prove operator purity: `{}` is not a proven primitive value",
            self.syntax.code(node)
        );
        Ok(())
    }

    async fn definition_is(&self, at: usize, declaration: usize) -> Result<()> {
        let s = self.syntax;
        let (line, col) = crate::signature::position_at(s.text, at)?;
        let uri =
            url::Url::from_file_path(self.file).map_err(|_| anyhow::anyhow!("invalid file"))?;
        let answer = crate::tools::execute_lsp_query(self.remote, self.root, self.file,
            "textDocument/definition", serde_json::json!({
                "textDocument": {"uri": uri.as_str()}, "position": {"line": line - 1, "character": col - 1}
            })).await?;
        let entries: Vec<_> = match &answer {
            serde_json::Value::Array(a) => a.iter().collect(),
            serde_json::Value::Object(_) => vec![&answer],
            _ => Vec::new(),
        };
        ensure!(
            entries.len() == 1,
            "cannot prove binding identity: definition is unavailable or ambiguous"
        );
        let loc = entries[0];
        let target_uri = loc
            .get("targetUri")
            .or_else(|| loc.get("uri"))
            .and_then(|u| u.as_str());
        let range = loc.get("targetSelectionRange").or_else(|| loc.get("range"));
        let target = target_uri.and_then(crate::signature::local_file);
        let position = range.and_then(|r| {
            Some((
                u32::try_from(r.pointer("/start/line")?.as_u64()?)
                    .ok()?
                    .checked_add(1)?,
                u32::try_from(r.pointer("/start/character")?.as_u64()?)
                    .ok()?
                    .checked_add(1)?,
            ))
        });
        ensure!(
            target.as_deref() == Some(self.file)
                && position
                    == crate::signature::line_col_at(s.text, s.nodes[declaration].span.start),
            "cannot prove binding identity: analyzer definition differs from the unique local declaration"
        );
        Ok(())
    }

    async fn value(&mut self, node: usize, depth: usize) -> Result<Value> {
        ensure!(
            depth < 64,
            "cannot prove operator purity: recursive value/type"
        );
        if let Some(value) = self.values.get(&node) {
            return Ok(value.clone());
        }
        let s = self.syntax;
        let children = s.expressions(node);
        let value = match s.nodes[node].kind.as_str() {
            "PAREN_EXPR" if children.len() == 1 => {
                Box::pin(self.value(children[0], depth + 1)).await?
            }
            "IF_EXPR" if children.len() == 3 => {
                for child in children {
                    ensure!(
                        matches!(Box::pin(self.value(child, depth + 1)).await?, Value::Scalar),
                        "cannot prove primitive conditional value"
                    );
                }
                Value::Scalar
            }
            "BLOCK_EXPR" => {
                let list = s.child(node, "STMT_LIST").context("unknown block value")?;
                let tail = s.expressions(list);
                ensure!(
                    tail.len() == 1
                        && !s.nodes[list]
                            .children
                            .iter()
                            .any(|i| matches!(s.nodes[*i].kind.as_str(), "LET_STMT" | "EXPR_STMT")),
                    "cannot prove a compound block value"
                );
                Box::pin(self.value(tail[0], depth + 1)).await?
            }
            "LITERAL" => {
                ensure!(
                    s.nodes[node].children.iter().any(|i| matches!(
                        s.nodes[*i].kind.as_str(),
                        "INT_NUMBER" | "FLOAT_NUMBER" | "TRUE_KW" | "FALSE_KW"
                    )),
                    "cannot prove purity of this literal kind"
                );
                Value::Scalar
            }
            "PATH_EXPR" => {
                let name = s.code(node);
                ensure!(
                    crate::signature::effects::is_path(name) && !name.contains("::"),
                    "cannot prove purity of a nonlocal path"
                );
                let binding = s.binding(self.function, name)?;
                let declaration = s.child(binding, "NAME").context("binding has no name")?;
                self.definition_is(s.nodes[node].span.start, declaration)
                    .await?;
                let parent = s.nodes[binding]
                    .parent
                    .context("binding has no declaration")?;
                ensure!(
                    matches!(s.nodes[parent].kind.as_str(), "PARAM" | "LET_STMT"),
                    "cannot prove the value of a destructuring or deferred binding"
                );
                if let Some(ty) = s.nodes[parent]
                    .children
                    .iter()
                    .copied()
                    .find(|i| s.nodes[*i].kind.ends_with("_TYPE"))
                {
                    Box::pin(self.ty(ty, depth + 1)).await?
                } else {
                    let rhs = s.expressions(parent);
                    ensure!(rhs.len() == 1, "cannot prove inferred binding type");
                    Box::pin(self.value(rhs[0], depth + 1)).await?
                }
            }
            "FIELD_EXPR" if children.len() == 1 => {
                let mut base = Box::pin(self.value(children[0], depth + 1)).await?;
                while let Value::Reference(inner) = base {
                    base = *inner;
                }
                let Value::Record(record) = base else {
                    bail!("cannot prove field access has no user-defined Deref");
                };
                ensure!(
                    !s.nodes
                        .iter()
                        .enumerate()
                        .any(|(i, n)| n.kind == "ATTR" && s.inside(i, record)),
                    "cannot prove fields preserved by record/field attribute expansion"
                );
                let name = s.child(node, "NAME_REF").context("unknown field syntax")?;
                let fields: Vec<_> = s
                    .nodes
                    .iter()
                    .enumerate()
                    .filter_map(|(i, n)| {
                        (n.kind == "RECORD_FIELD"
                            && s.inside(i, record)
                            && s.child(i, "NAME")
                                .is_some_and(|f| ident(s.code(f)) == ident(s.code(name))))
                        .then_some(i)
                    })
                    .collect();
                ensure!(fields.len() == 1, "cannot prove local field identity");
                let ty = s.nodes[fields[0]]
                    .children
                    .iter()
                    .copied()
                    .find(|i| s.nodes[*i].kind.ends_with("_TYPE"))
                    .context("cannot prove field type")?;
                Box::pin(self.ty(ty, depth + 1)).await?
            }
            "BIN_EXPR" if children.len() == 2 => {
                ensure!(
                    [
                        "+", "-", "*", "/", "%", "<<", ">>", "&", "|", "^", "==", "!=", "<", "<=",
                        ">", ">=", "&&", "||"
                    ]
                    .contains(&s.operator(node).as_str()),
                    "cannot prove purity of this operator"
                );
                for child in children {
                    ensure!(
                        matches!(Box::pin(self.value(child, depth + 1)).await?, Value::Scalar),
                        "cannot prove operator purity: operand `{}` may dispatch user traits",
                        s.code(child)
                    );
                }
                Value::Scalar
            }
            "PREFIX_EXPR"
                if children.len() == 1 && matches!(s.operator(node).as_str(), "-" | "!") =>
            {
                ensure!(
                    matches!(
                        Box::pin(self.value(children[0], depth + 1)).await?,
                        Value::Scalar
                    ),
                    "cannot prove unary operator purity"
                );
                Value::Scalar
            }
            _ => bail!(
                "cannot prove operator purity for {} (calls, indexing, dereference and unknown syntax can run user code)",
                s.nodes[node].kind
            ),
        };
        self.values.insert(node, value.clone());
        Ok(value)
    }

    async fn ty(&mut self, node: usize, depth: usize) -> Result<Value> {
        ensure!(depth < 64, "cannot prove recursive field type");
        let s = self.syntax;
        if s.nodes[node].kind == "REF_TYPE" {
            let ty = s.nodes[node]
                .children
                .iter()
                .copied()
                .find(|i| s.nodes[*i].kind.ends_with("_TYPE"))
                .context("unknown reference type")?;
            return Ok(Value::Reference(Box::new(
                Box::pin(self.ty(ty, depth + 1)).await?,
            )));
        }
        ensure!(
            s.nodes[node].kind == "PATH_TYPE"
                && crate::signature::effects::is_path(s.code(node))
                && !s.code(node).contains("::"),
            "cannot prove operator purity for this type syntax"
        );
        let name = s.code(node);
        if SCALARS.contains(&name) {
            let hover = hover_markdown(
                self.remote,
                self.root,
                self.file,
                s.text,
                s.nodes[node].span.start,
                &mut self.hovers,
            )
            .await;
            ensure!(
                hover.as_deref().is_some_and(|h| hover_is_builtin(h, name)),
                "cannot prove `{name}` denotes the built-in primitive type"
            );
            return Ok(Value::Scalar);
        }
        let records: Vec<_> = s
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                (n.kind == "STRUCT"
                    && s.child(i, "NAME")
                        .is_some_and(|n| ident(s.code(n)) == ident(name)))
                .then_some(i)
            })
            .collect();
        ensure!(
            records.len() == 1,
            "cannot prove operator purity: `{name}` is not a unique local record or primitive"
        );
        let record = records[0];
        ensure!(
            s.child(record, "GENERIC_PARAM_LIST").is_none(),
            "cannot prove generic record field types"
        );
        self.definition_is(
            s.nodes[node].span.start,
            s.child(record, "NAME").context("record has no name")?,
        )
        .await?;
        Ok(Value::Record(record))
    }
}
