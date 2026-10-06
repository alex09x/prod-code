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

use super::enclosing::unreported_note;

/// What the extraction did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtractedParameter {
    /// The function the parameter was added to.
    pub symbol: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub name: String,
    /// Empty when the parameter is written without one (JavaScript, or Python with no type).
    pub ty: String,
    /// The new parameter as the declaration now spells it: `name: T`, `name T`, `T name`,
    /// `_ name: T` or `name`.
    #[serde(skip)]
    pub parameter: String,
    /// The expression that left the body, as it was written.
    pub expression: String,
    /// How many places in the body now read the parameter.
    pub replaced: usize,
    pub call_sites: usize,
    pub rewritten: Vec<(String, String)>,
    pub unmatched: Vec<String>,
    /// Files that call the function by name where the analyzer reported no reference: checked
    /// with the rewritten files, not rewritten (#294).
    pub unreported: Vec<String>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl ExtractedParameter {
    /// The report: what moved out of the body, and whether the result compiles.
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- new parameter: `{}`\n- from the body: `{}`\n- {} place(s) in the \
             body now read it, {} call site(s) pass it\n\n",
            self.symbol, self.file, self.parameter, self.expression, self.replaced, self.call_sites
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
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot given the argument ({} reference(s) that are not a call with this \
                 arity — a function pointer, a macro, or a call already changed):\n",
                self.unmatched.len()
            ));
            for r in &self.unmatched {
                out.push_str(&format!("  {r}\n"));
            }
        }
        out.push_str(&unreported_note(&self.unreported, &self.file));
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
            // The expression travelled to places where its names may not exist. That is the
            // usual cause and it is not worth making the reader work it out.
            out.push_str(
                "\nthe expression is now written at every call site: if it names a local, a \
                 parameter or anything private to the function it came from, it cannot be \
                 spelled there. Extract something the callers can see.\n",
            );
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

/// A function or method as `textDocument/documentSymbol` reports it, 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enclosing {
    pub name: String,
    pub start: u32,
    pub end: u32,
    /// The column the range ends at on `end`.
    pub end_col: u32,
    /// Where the name is written (`selectionRange`), when the analyzer says.
    pub name_at: Option<(u32, u32)>,
}

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
