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

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Generified {
    pub function: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub was: String,
    pub now: String,
    /// Files that call the function, checked against the new signature.
    pub callers_checked: usize,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Generified {
    pub fn render(&self) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: `{}`\n- now: `{}`\n- {} file(s) that call it checked against the \
             new signature; their calls do not change, the type argument is inferred\n",
            self.function, self.file, self.was, self.now, self.callers_checked
        );
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str(
                "\nthe analyzer rejects the result — the body uses something the bound does not \
                 promise, or a caller passes a type that does not satisfy it or can no longer be \
                 inferred:\n",
            );
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        if self.applied {
            out.push_str("\n[applied]\n");
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make this edit\n");
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct PolyglotFuncDecl {
    pub name: String,
    pub decl_start: usize,
    pub _name_start: usize,
    pub name_end: usize,
    pub open_paren: usize,
    pub close_paren: usize,
    pub has_generics: bool,
    pub generics_span: Option<(usize, usize)>,
}
