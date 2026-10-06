/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::path::Path;

use crate::signature::types::SignatureChange;
use crate::signature::util::display;

impl SignatureChange {
    /// Refuses to write a change that is not complete: a reference the rewrite did not reach
    /// or a change no reference explains, whatever `force` says, since `force` overrides the
    /// analyzer's verdict on a complete change and a reorder can compile and still run
    /// differently; and, unless `force`, a call that would `.await` outside an `async fn`. The
    /// direct write and the compile gate both stop here before anything is written (#446).
    pub fn ensure_writable(&self, force: bool) -> Result<()> {
        if !self.unmatched.is_empty() || !self.unexpected.is_empty() {
            let listed: Vec<String> = self
                .unmatched
                .iter()
                .map(|u| format!("not rewritten: {u}"))
                .chain(
                    self.unexpected
                        .iter()
                        .map(|u| format!("changed without being a reference: {u}")),
                )
                .collect();
            anyhow::bail!(
                "the change to `{}` is not complete: {} reference(s) were not rewritten and {} \
                 change(s) are not a reference; nothing was written, and `force` does not \
                 override this:\n  {}",
                self.symbol,
                self.unmatched.len(),
                self.unexpected.len(),
                listed.join("\n  ")
            );
        }
        anyhow::ensure!(
            self.not_async.is_empty() || force,
            "{} call(s) would `.await` from a function that is not `async`; nothing was \
             written. Make those callers `async` first, or pass `force: true`:\n  {}",
            self.not_async.len(),
            self.not_async.join("\n  ")
        );
        Ok(())
    }

    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: ({})\n- now: ({})\n",
            self.symbol, self.file, self.old_signature, self.new_signature
        );
        if let Some((was, now)) = &self.returns {
            out.push_str(&format!("- returns: `{was}` → `{now}`\n"));
        }
        if let Some((was, now)) = &self.visibility {
            out.push_str(&format!("- visibility: `{was}` → `{now}`\n"));
        }
        if let Some((was, now)) = self.asyncness {
            let word = |a: bool| if a { "async" } else { "not async" };
            out.push_str(&format!(
                "- {} → {}: every call {} `.await`\n",
                word(was),
                word(now),
                if now { "gains" } else { "loses" }
            ));
        }
        for site in &self.not_async {
            out.push_str(&format!(
                "- {site}: awaits from a function that is not `async`\n"
            ));
        }
        if !self.rule.is_empty() {
            out.push_str(&format!("- call sites: `{}`\n", self.rule));
        }
        out.push('\n');
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
                "\nnot rewritten ({} reference(s) the rule did not match — a call through a \
                 function pointer, a macro, or a spelling structural search cannot see):\n",
                self.unmatched.len()
            ));
            for r in &self.unmatched {
                out.push_str(&format!("  {r}\n"));
            }
        }
        if !self.unexpected.is_empty() {
            out.push_str(&format!(
                "\nchanged without being a known reference ({}), check these by hand:\n",
                self.unexpected.len()
            ));
            for r in &self.unexpected {
                out.push_str(&format!("  {r}\n"));
            }
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe proposal passes validation: 0 errors\n");
        } else {
            out.push_str("\nthe proposal fails validation:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        if self.applied {
            out.push_str(&format!(
                "\n[applied to {} file(s)]\n",
                self.rewritten.len()
            ));
        } else if !self.unmatched.is_empty() || !self.unexpected.is_empty() {
            out.push_str(
                "\nnothing was written, and `apply` writes nothing while a reference is not \
                 rewritten or a change is not a reference, `force` or not\n",
            );
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make these edits\n");
        }
        out
    }
}
