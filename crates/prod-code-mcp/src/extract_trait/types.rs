/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Extracted {
    pub trait_name: String,
    pub type_name: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub methods: Vec<String>,
    pub kept: Vec<String>,
    pub imports: Vec<(String, String)>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Extracted {
    pub fn render(&self) -> String {
        let mut out = format!(
            "`trait {}` for `{}` ({})\n\n- into the trait: {}\n",
            self.trait_name,
            self.type_name,
            self.file,
            self.methods.join(", ")
        );
        if !self.kept.is_empty() {
            out.push_str(&format!("- still inherent: {}\n", self.kept.join(", ")));
        }
        if !self.imports.is_empty() {
            out.push_str("imports:\n");
            for (file, line) in &self.imports {
                out.push_str(&format!("  {file}: added `{line}`\n"));
            }
        }
        for (path, new_text) in &self.rewritten {
            let full = Path::new(path);
            let old_text = if self.applied {
                crate::refactor::text_before_apply(full)
            } else {
                std::fs::read_to_string(full).unwrap_or_default()
            };
            let rel = full
                .strip_prefix(&self.root)
                .unwrap_or(full)
                .display()
                .to_string();
            out.push('\n');
            out.push_str(
                &similar::TextDiff::from_lines(old_text.as_str(), new_text.as_str())
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        out.push_str(if self.applied {
            "\n[applied]\n"
        } else {
            "\nnothing was written; pass `apply: true` to make this edit\n"
        });
        out
    }
}

pub fn is_ident(c: char) -> bool {
    unicode_ident::is_xid_continue(c) || c == '_'
}

pub fn valid_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    name != "_"
        && (unicode_ident::is_xid_start(first) || first == '_')
        && chars.all(is_ident)
        && !matches!(
            name,
            "Self"
                | "abstract"
                | "as"
                | "async"
                | "await"
                | "become"
                | "box"
                | "break"
                | "const"
                | "continue"
                | "crate"
                | "do"
                | "dyn"
                | "else"
                | "enum"
                | "extern"
                | "false"
                | "final"
                | "fn"
                | "for"
                | "gen"
                | "if"
                | "impl"
                | "in"
                | "let"
                | "loop"
                | "macro"
                | "match"
                | "mod"
                | "move"
                | "mut"
                | "override"
                | "priv"
                | "pub"
                | "ref"
                | "return"
                | "self"
                | "static"
                | "struct"
                | "super"
                | "trait"
                | "true"
                | "try"
                | "type"
                | "typeof"
                | "union"
                | "unsafe"
                | "unsized"
                | "use"
                | "virtual"
                | "where"
                | "while"
                | "yield"
        )
}

/// One item of an `impl` block: its text runs from `start` (its comments, docs and attributes
/// included) to `end`, exclusive.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub start: usize,
    pub end: usize,
    /// The function's name, when the item is one.
    pub name: Option<String>,
}

/// An inherent `impl Type { … }` block.
#[derive(Debug, Clone, PartialEq)]
pub struct ImplBlock {
    /// Where the `impl` keyword starts. Text before it is never replaced.
    pub start: usize,
    pub open: usize,
    pub close: usize,
    /// The declaration between `impl` and the self type, including `<` and `>`.
    pub generics: String,
    /// The declared lifetime/type/const parameter names, ready for a type argument list.
    pub generic_args: Vec<String>,
    pub self_ty: String,
    /// The original header after `impl`, without surrounding whitespace.
    pub header: String,
    /// The original `where` clause, when present.
    pub where_clause: String,
    /// Whether the original header put the `where` clause on another line.
    pub where_on_newline: bool,
    pub items: Vec<Item>,
}
