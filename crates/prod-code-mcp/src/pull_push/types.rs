/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;

/// Outcome of a pull up or push down refactoring.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HierarchyRefactorResult {
    pub operation: String,
    pub source_class: String,
    pub target_classes: Vec<String>,
    pub members: Vec<String>,
    pub files_modified: Vec<String>,
    #[serde(skip)]
    pub overlays: Vec<(PathBuf, String)>,
    pub diff: String,
    pub applied: bool,
    pub verified: bool,
    pub diagnostics: Vec<String>,
}

impl HierarchyRefactorResult {
    pub fn render(&self, max_diff_len: usize) -> String {
        let op_name = if self.operation == "pull_up" {
            "Pull Up"
        } else {
            "Push Down"
        };
        let mut out = format!(
            "{op_name} — source: `{}` -> targets: {:?}\n",
            self.source_class, self.target_classes
        );
        out.push_str(&format!(
            "- members ({}): {}\n",
            self.members.len(),
            self.members.join(", ")
        ));
        out.push_str(&format!(
            "- files modified ({}): {}\n",
            self.files_modified.len(),
            self.files_modified.join(", ")
        ));
        out.push_str(&format!("- applied: {}\n", self.applied));
        out.push_str(&format!("- verified: {}\n", self.verified));
        if !self.diagnostics.is_empty() {
            out.push_str(&format!("- diagnostics ({}):\n", self.diagnostics.len()));
            for d in &self.diagnostics {
                out.push_str(&format!("  • {d}\n"));
            }
        }
        if !self.diff.is_empty() {
            out.push_str("\nDiff:\n```diff\n");
            if self.diff.len() > max_diff_len {
                out.push_str(&self.diff[..max_diff_len]);
                out.push_str("\n... [truncated]\n");
            } else {
                out.push_str(&self.diff);
            }
            out.push_str("```\n");
        }
        out
    }
}

/// A parsed class, struct, or trait declaration.
#[derive(Debug, Clone)]
pub struct ClassDecl {
    pub name: String,
    pub language: String,
    pub file_path: PathBuf,
    pub super_names: Vec<String>,
    pub decl_start: usize,
    pub decl_end: usize,
    pub body_start: usize,
    pub body_end: usize,
    pub indent: String,
    pub members: Vec<MemberDecl>,
}

/// A member (method, field, property, constant, associated item) inside a class or trait.
#[derive(Debug, Clone)]
pub struct MemberDecl {
    pub name: String,
    pub kind: MemberKind,
    pub is_override: bool,
    pub start_offset: usize,
    pub end_offset: usize,
    pub full_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    Method,
    Field,
    Constant,
    AssociatedType,
}
