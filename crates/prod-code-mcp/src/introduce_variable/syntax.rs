/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A checked, byte-addressed view of the analyzer's Rust CST. Unknown/missing syntax is an
//! uncertainty, never permission to fall back to text-based semantic certification.

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{borrow::Cow, net::SocketAddr, ops::Range, path::Path};

#[derive(Deserialize)]
struct RawNode {
    kind: String,
    start: [usize; 3],
    end: [usize; 3],
    #[serde(default)]
    children: Vec<RawNode>,
}

pub(super) struct Node {
    pub kind: String,
    pub span: Range<usize>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

pub(super) struct Syntax<'a> {
    pub text: &'a str,
    pub nodes: Vec<Node>,
}

impl<'a> Syntax<'a> {
    pub async fn load(remote: SocketAddr, root: &Path, file: &Path, text: &'a str) -> Result<Self> {
        let mut session = crate::session::LspSession::open(remote, root, Some(file)).await?;
        let uri = url::Url::from_file_path(file).map_err(|_| anyhow::anyhow!("invalid file"))?;
        let answer = session
            .query_with_text(
                file,
                text,
                "rust-analyzer/viewSyntaxTree",
                serde_json::json!({"textDocument": {"uri": uri.as_str()}}),
            )
            .await;
        session.close().await;
        let answer = answer
            .context("cannot establish executable occurrence identity: syntax unavailable")?;
        let raw: RawNode = serde_json::from_str(answer.as_str().context(
            "cannot establish executable occurrence identity: missing Rust syntax tree",
        )?)
        .context("cannot establish executable occurrence identity: unknown Rust syntax format")?;
        ensure!(
            raw.kind == "SOURCE_FILE" && raw.start[0] == 0 && raw.end[0] == text.len(),
            "Rust syntax tree does not cover the current source"
        );
        let mut syntax = Self {
            text,
            nodes: Vec::new(),
        };
        syntax.add(raw, None, 0)?;
        Ok(syntax)
    }

    fn add(&mut self, raw: RawNode, parent: Option<usize>, depth: usize) -> Result<usize> {
        ensure!(depth < 256, "Rust syntax nesting is unproven");
        let span = raw.start[0]..raw.end[0];
        ensure!(
            span.start <= span.end && self.text.get(span.clone()).is_some(),
            "invalid Rust syntax span"
        );
        let opaque = matches!(
            raw.kind.as_str(),
            "STRING" | "BYTE_STRING" | "C_STRING" | "CHAR" | "BYTE" | "COMMENT"
        );
        let id = self.nodes.len();
        self.nodes.push(Node {
            kind: raw.kind,
            span: span.clone(),
            parent,
            children: Vec::new(),
        });
        if !opaque {
            let mut at = span.start;
            for child in raw.children {
                ensure!(
                    child.start[0] == at && child.end[0] <= span.end,
                    "noncontiguous Rust syntax tree"
                );
                at = child.end[0];
                let child = self.add(child, Some(id), depth + 1)?;
                self.nodes[id].children.push(child);
            }
            ensure!(
                self.nodes[id].children.is_empty() || at == span.end,
                "incomplete Rust syntax tree"
            );
        }
        Ok(id)
    }

    pub fn code(&self, id: usize) -> &'a str {
        &self.text[self.nodes[id].span.clone()]
    }

    pub fn inside(&self, inner: usize, outer: usize) -> bool {
        let a = &self.nodes[inner].span;
        let b = &self.nodes[outer].span;
        b.start <= a.start && a.end <= b.end
    }

    pub fn ancestors(&self, id: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(Some(id), |i| self.nodes[*i].parent)
    }

    pub fn child(&self, id: usize, kind: &str) -> Option<usize> {
        self.nodes[id]
            .children
            .iter()
            .copied()
            .find(|i| self.nodes[*i].kind == kind)
    }

    pub fn expressions(&self, id: usize) -> Vec<usize> {
        self.nodes[id]
            .children
            .iter()
            .copied()
            .filter(|i| self.is_expr(*i))
            .collect()
    }

    pub fn is_expr(&self, id: usize) -> bool {
        (self.nodes[id].kind.ends_with("_EXPR") || self.nodes[id].kind == "LITERAL")
            && !self.in_pattern(id)
    }

    pub fn in_pattern(&self, id: usize) -> bool {
        // The CST shares expression-shaped nodes (notably LITERAL) with patterns.
        // Replacing a pattern literal with a local name would introduce a binding,
        // rather than read the local, and can silently change which arm matches.
        self.ancestors(id)
            .any(|i| self.nodes[i].kind.ends_with("_PAT"))
    }

    pub fn expression_at(&self, at: usize, len: usize) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .find_map(|(i, n)| (n.span == (at..at + len) && self.is_expr(i)).then_some(i))
    }

    pub fn function(&self, id: usize) -> Option<usize> {
        self.ancestors(id).find(|i| self.nodes[*i].kind == "FN")
    }

    pub fn binding(&self, function: usize, name: &str) -> Result<usize> {
        let bindings: Vec<_> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                (n.kind == "IDENT_PAT"
                    && self.inside(i, function)
                    && self
                        .child(i, "NAME")
                        .is_some_and(|n| ident(self.code(n)) == ident(name)))
                .then_some(i)
            })
            .collect();
        ensure!(
            bindings.len() == 1,
            "`{name}` has ambiguous binding identity ({} patterns, including parameters, let, for, closure and match bindings)",
            bindings.len()
        );
        Ok(bindings[0])
    }

    /// The operator tokens between the immediate expression operands (not nested operands).
    pub fn operator(&self, id: usize) -> String {
        self.nodes[id]
            .children
            .iter()
            .filter(|i| self.nodes[**i].children.is_empty())
            .filter(|i| !matches!(self.nodes[**i].kind.as_str(), "WHITESPACE" | "COMMENT"))
            .map(|i| self.code(*i))
            .collect()
    }
}

/// Rust identifiers have NFC identity, independent of raw spelling. Use the same
/// identity for freshness, local bindings, record names and fields.
pub(super) fn ident(s: &str) -> Cow<'_, str> {
    icu_normalizer::ComposingNormalizerBorrowed::new_nfc()
        .normalize(s.strip_prefix("r#").unwrap_or(s))
}
