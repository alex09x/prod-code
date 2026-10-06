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

use crate::extract_field::helpers::display;

/// What the extraction did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtractedField {
    /// The type the field was added to.
    pub owner: String,
    /// The method the expression came from.
    pub method: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub name: String,
    pub ty: String,
    /// What each construction site now initialises the field with.
    pub init: String,
    /// How many places in the method now read the field.
    pub replaced: usize,
    /// Construction sites given the initialiser.
    pub constructors: usize,
    pub blocked: Vec<String>,
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl ExtractedField {
    /// The report: where the value lives now, and whether the result compiles.
    pub fn render(&self, diff_budget: usize) -> String {
        let recv_read = if self.file.ends_with(".ts")
            || self.file.ends_with(".tsx")
            || self.file.ends_with(".js")
            || self.file.ends_with(".jsx")
        {
            format!("this.{}", self.name)
        } else if self.file.ends_with(".cpp")
            || self.file.ends_with(".cc")
            || self.file.ends_with(".cxx")
            || self.file.ends_with(".h")
            || self.file.ends_with(".hpp")
        {
            format!("this->{}", self.name)
        } else if self.file.ends_with(".go") {
            format!("r.{}", self.name)
        } else {
            format!("self.{}", self.name)
        };
        let field_decl = if self.ty.is_empty() {
            self.name.clone()
        } else {
            format!("{}: {}", self.name, self.ty)
        };
        let mut out = format!(
            "`{}.{}` ({})\n\n- new field: `{}`\n- `{}` now reads `{}` in {} place(s)\n- \
             {} construction site(s) initialise it with `{}`\n\n",
            self.owner,
            self.name,
            self.file,
            field_decl,
            self.method,
            recv_read,
            self.replaced,
            self.constructors,
            self.init
        );
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
                "\n{} use(s) of `{}` stop compiling with one more field and cannot be rewritten:\n",
                self.blocked.len(),
                self.owner
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) this could not read):\n",
                self.unmatched.len()
            ));
            for r in &self.unmatched {
                out.push_str(&format!("  {r}\n"));
            }
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
            out.push_str(
                "\nthe expression is now written at every construction site: if it names a \
                 local or a parameter of the method, it cannot be spelled there. Pass `init` \
                 with what a new value should start as.\n",
            );
        }
        if !self.ty.is_empty()
            && self.file.ends_with(".rs")
            && !crate::encapsulate_field::returns_by_value(&self.ty)
        {
            out.push_str(&format!(
                "\n`{}` is not one of the primitive `Copy` types: if it is not `Copy` at all, a \
                 place where the method used the expression by value now moves out of `self`, \
                 and the analyzer does not check that. Ask for `verify: \"compile\"` to be \
                 sure.\n",
                self.ty
            ));
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

/// What the braces opening at `open` are, read from what follows them: a pattern is followed
/// by `=>`, a single `=`, `|` or a `:` type ascription; anything else builds a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Braces {
    Literal,
    /// A pattern; `rest` when it ends in `..` and so matches a struct with more fields.
    Pattern {
        rest: bool,
    },
}
