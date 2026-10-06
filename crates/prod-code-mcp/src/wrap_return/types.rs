/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::utils::display;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Which wrapper the return type gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wrapper {
    Option,
    Result,
    Promise,
    Pointer,
    Custom(String),
}

impl serde::Serialize for Wrapper {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Option => serializer.serialize_str("option"),
            Self::Result => serializer.serialize_str("result"),
            Self::Promise => serializer.serialize_str("promise"),
            Self::Pointer => serializer.serialize_str("pointer"),
            Self::Custom(name) => serializer.serialize_str(name),
        }
    }
}

impl Wrapper {
    pub fn parse(text: &str) -> Result<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            anyhow::bail!(
                "wrapper cannot be empty: use `option`, `result`, `promise`, `pointer`, or a custom envelope type name"
            );
        }
        match trimmed.to_ascii_lowercase().as_str() {
            "option" | "optional" | "nullable" => Ok(Self::Option),
            "result" | "expected" | "error" => Ok(Self::Result),
            "promise" | "future" | "async" => Ok(Self::Promise),
            "pointer" | "ptr" => Ok(Self::Pointer),
            _ => Ok(Self::Custom(trimmed.to_string())),
        }
    }

    pub fn assist_id(&self) -> &'static str {
        match self {
            Self::Option => "wrap_return_type_in_option",
            Self::Result => "wrap_return_type_in_result",
            _ => "wrap_return_type_in_option",
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Option => "Option",
            Self::Result => "Result",
            Self::Promise => "Promise",
            Self::Pointer => "Pointer",
            Self::Custom(name) => name.as_str(),
        }
    }
}

/// What the wrapping did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WrappedReturn {
    pub function: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub was: String,
    pub now: String,
    /// Call sites that got `?` (or `await`, etc.).
    pub propagated: usize,
    /// Call sites whose caller cannot propagate, with the reason.
    pub blocked: Vec<String>,
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl WrappedReturn {
    pub fn render(&self, diff_budget: usize) -> String {
        let prop_note = if self.now.starts_with("Promise") {
            format!("{} call site(s) propagate with `await`", self.propagated)
        } else if self.now.starts_with("Option") || self.now.starts_with("Result") {
            format!("{} call site(s) propagate with `?`", self.propagated)
        } else {
            format!("{} call site(s) propagate", self.propagated)
        };
        let mut out = format!(
            "`{}` ({})\n\n- returned: `{}`\n- now returns: `{}`\n- {}\n\n",
            self.function, self.file, self.was, self.now, prop_note
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
            let target_desc = if self.now.starts_with("Option") {
                "an `Option`"
            } else if self.now.starts_with("Result") {
                "a `Result`"
            } else if self.now.starts_with("Promise") {
                "a `Promise`"
            } else {
                &self.now
            };
            out.push_str(&format!(
                "\n{} call site(s) cannot propagate: the calling function does not return {target_desc}. \
                 Each needs a decision — unwrap, match, or wrap that caller too:\n",
                self.blocked.len()
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) this could not read as a call):\n",
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

/// Function declaration info for non-Rust languages.
#[derive(Debug, Clone)]
pub struct PolyglotFuncDecl {
    pub name: String,
    pub decl_start: usize,
    pub name_start: usize,
    pub close_paren: usize,
    pub body_open: usize,
    pub body_close: usize,
    pub was: String,
    pub ret_span: Option<(usize, usize)>,
    pub is_async: bool,
    pub is_arrow: bool,
    pub has_return_type: bool,
}
