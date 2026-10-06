/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Refactoring target mode: static factory method or fluent builder pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplaceMode {
    Factory,
    Builder,
}

impl std::fmt::Display for ReplaceMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Factory => write!(f, "factory"),
            Self::Builder => write!(f, "builder"),
        }
    }
}

/// A parsed field of a struct, class, or interface.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FieldDecl {
    pub name: String,
    pub ty: String,
    pub vis: String,
}

/// A parsed struct or class declaration.
#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub language: String,
    pub fields: Vec<FieldDecl>,
    pub generics: Option<String>,
    pub is_pub: bool,
    pub decl_start: usize,
    pub decl_end: usize,
    pub line: u32,
    pub col: u32,
}

/// Discovered raw instantiation to rewrite.
#[derive(Debug, Clone)]
pub struct InstantiationSite {
    pub start: usize,
    pub end: usize,
    pub field_values: BTreeMap<String, String>,
    pub field_order: Vec<String>,
    pub prefix: String,
    pub has_rest_pattern: bool,
}

/// Result of replacing constructors with factory methods or builders.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReplaceConstructorResult {
    pub type_name: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub mode: ReplaceMode,
    pub target_name: String,
    pub declared_fields: Vec<String>,
    pub instantiations_rewritten: usize,
    pub blocked: Vec<String>,
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
    pub language: String,
}

impl ReplaceConstructorResult {
    /// Render a human-readable report of the refactoring outcome.
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({}) — replaced constructor with {}\n\n",
            self.type_name, self.file, self.mode
        );
        out.push_str(&format!(
            "- target: `{}` ({})\n",
            self.target_name, self.mode
        ));
        out.push_str(&format!(
            "- declared fields ({}): {}\n",
            self.declared_fields.len(),
            if self.declared_fields.is_empty() {
                "none".to_string()
            } else {
                self.declared_fields.join(", ")
            }
        ));
        out.push_str(&format!(
            "- {} instantiation(s) rewritten across {} file(s)\n\n",
            self.instantiations_rewritten,
            self.rewritten.len()
        ));

        let mut body = String::new();
        let mut changed_lines = 0usize;
        for (path, new_text) in &self.rewritten {
            let old_text = crate::refactor::text_before_apply(Path::new(path));
            let rel = display(&self.root, Path::new(path));
            let diff = similar::TextDiff::from_lines(&old_text, new_text);
            changed_lines += diff
                .iter_all_changes()
                .filter(|c| c.tag() != similar::ChangeTag::Equal)
                .count();
            body.push_str(
                &diff
                    .unified_diff()
                    .context_radius(2)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        out.push_str(&format!(
            "{} changed line(s) in {} file(s)\n\n",
            changed_lines,
            self.rewritten.len()
        ));
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }

        if !self.blocked.is_empty() {
            out.push_str(&format!(
                "\n{} instantiation(s) could not be safely rewritten:\n",
                self.blocked.len()
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s)):\n",
                self.unmatched.len()
            ));
            for u in &self.unmatched {
                out.push_str(&format!("  {u}\n"));
            }
        }

        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }

        if self.applied {
            out.push_str(&format!(
                "\n[applied to {} file(s)]\n",
                self.rewritten.len()
            ));
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make these edits\n");
        }
        out
    }
}

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
