/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use anyhow::{Context, Result, ensure};
use ra_ap_syntax::{AstNode, Edition, SourceFile, SyntaxNode, TextRange, TextSize, ast};

/// Original-file expression ancestry and lexical declaration identity. Analyzer
/// validation of the owning type and constructor references remains separate.
pub(super) struct ValueScope {
    root: SyntaxNode,
    body: TextSize,
}

impl ValueScope {
    pub(super) fn parse(file: &Path, text: &str, body: usize) -> Result<Self> {
        // Standalone Rust files use rustc's default edition. A Cargo consumer must
        // use its declared/inherited edition; an invalid manifest is not a default.
        let edition = if file
            .ancestors()
            .skip(1)
            .any(|p| p.join("Cargo.toml").is_file())
        {
            crate::parameter_object::rust_edition(file).map_err(anyhow::Error::msg)?
        } else {
            2015
        };
        let edition = match edition {
            2015 => Edition::Edition2015,
            2018 => Edition::Edition2018,
            2021 => Edition::Edition2021,
            2024 => Edition::Edition2024,
            _ => anyhow::bail!("unsupported Rust edition {edition}"),
        };
        let parsed = SourceFile::parse(text, edition);
        let errors = parsed.errors();
        ensure!(
            errors.is_empty(),
            "cannot establish Rust value scope: {errors:?}"
        );
        Ok(Self {
            root: parsed.syntax_node(),
            body: TextSize::try_from(body).context("method offset exceeds Rust syntax limits")?,
        })
    }

    pub(super) fn contains(&self, from: usize, to: usize) -> bool {
        let Some(expr) = self.expression(from, to) else {
            return false;
        };
        let range = expr.text_range();
        for node in expr.ancestors() {
            if let Some(block) = ast::BlockExpr::cast(node.clone()) {
                if block.const_token().is_some() {
                    return false;
                }
                if node.text_range().start() == self.body {
                    return true;
                }
            }
            if ast::Item::can_cast(node.kind())
                || ast::Pat::can_cast(node.kind())
                || ast::Type::can_cast(node.kind())
                || ast::ConstArg::can_cast(node.kind())
                || ast::TokenTree::can_cast(node.kind())
            {
                return false;
            }
            // Only the array's own separator marks a repeat-count operand.
            if let Some(array) = ast::ArrayExpr::cast(node)
                && let Some(separator) = array.semicolon_token()
                && separator.text_range().end() <= range.start()
            {
                return false;
            }
        }
        false
    }

    pub(super) fn same_bindings(&self, from: usize, to: usize, other: usize, end: usize) -> bool {
        let Some(selected) = self.expression(from, to) else {
            return false;
        };
        let Some(candidate) = self.expression(other, end) else {
            return false;
        };
        super::captures::compatible(&selected, &candidate)
            && super::bindings::identities(&selected).is_some_and(|selected| {
                super::bindings::identities(&candidate).is_some_and(|other| other == selected)
            })
    }

    fn expression(&self, from: usize, to: usize) -> Option<SyntaxNode> {
        let (Ok(from), Ok(to)) = (TextSize::try_from(from), TextSize::try_from(to)) else {
            return None;
        };
        let range = TextRange::new(from, to);
        // Parse the original complete file, never an expression wrapper or a
        // whitespace-masked program. Only an exact expression node can be edited.
        let element = self.root.covering_element(range);
        element
            .ancestors()
            .find(|n| n.text_range() == range && ast::Expr::can_cast(n.kind()))
    }
}
