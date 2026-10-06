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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The name rust-analyzer gives the function it extracts.
pub const PLACEHOLDER: &str = "fun_name";

/// More duplicates than this are reported, not tried: each one costs a type check.
pub const MAX_DUPLICATES: usize = 8;

/// One other place with the selection's text, and what became of it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Duplicate {
    /// The file it is in, relative to the checkout.
    pub file: String,
    pub line: u32,
    pub replaced: bool,
    pub reason: Option<String>,
    /// The literals it passes, when it differs from the selection only in literals.
    pub passes: Vec<String>,
}

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Extracted {
    pub name: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    /// What the selection now reads.
    pub call: String,
    /// Parameters the new function gained for literals the duplicates differ in, `name: type`.
    pub parameters: Vec<String>,
    pub duplicates: Vec<Duplicate>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Extracted {
    /// How many duplicates now call the new function.
    pub fn replaced(&self) -> usize {
        self.duplicates.iter().filter(|d| d.replaced).count()
    }

    pub fn render(&self) -> String {
        let mut diff = String::new();
        for (path, new_text) in &self.rewritten {
            let path = Path::new(path);
            let old_text = if self.applied {
                crate::refactor::text_before_apply(path)
            } else {
                std::fs::read_to_string(path).unwrap_or_default()
            };
            let shown = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            diff.push_str(
                &similar::TextDiff::from_lines(old_text.as_str(), new_text)
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{shown}"), &format!("b/{shown}"))
                    .to_string(),
            );
        }
        let kw = match Path::new(&self.file).extension().and_then(|e| e.to_str()) {
            Some("py") => "def",
            Some("go" | "swift") => "func",
            Some("ts" | "tsx" | "js" | "jsx") => "function",
            _ => "fn",
        };
        let mut out = format!(
            "`{kw} {}` extracted ({}); the selection now reads `{}`\n",
            self.name,
            self.file,
            self.call.trim()
        );
        if !self.parameters.is_empty() {
            out.push_str(&format!(
                "- new parameter(s) for the literals the copies differ in: {}\n",
                self.parameters.join(", ")
            ));
        }
        if self.duplicates.is_empty() {
            out.push_str("- no other place in the file has the selection's text\n");
        }
        for d in &self.duplicates {
            let at = if d.file == self.file {
                format!("line {}", d.line)
            } else {
                format!("{}:{}", d.file, d.line)
            };
            let passing = if d.passes.is_empty() {
                String::new()
            } else {
                format!(" passing {}", d.passes.join(", "))
            };
            match (&d.reason, d.replaced) {
                (_, true) => out.push_str(&format!(
                    "- {at}: the same code, now the same call{passing}\n"
                )),
                (Some(reason), false) => {
                    out.push_str(&format!("- {at}: left as it is: {reason}\n"))
                }
                (None, false) => out.push_str(&format!("- {at}: left as it is\n")),
            }
        }
        out.push('\n');
        out.push_str(&diff);
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

    /// Writes the result, unless the analyzer rejects it and `force` is not given.
    pub fn write(&mut self, force: bool) -> Result<()> {
        anyhow::ensure!(
            self.diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            self.diagnostics.len(),
            self.diagnostics.join("\n  ")
        );
        let files: BTreeMap<PathBuf, String> = self
            .rewritten
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.clone()))
            .collect();
        crate::refactor::apply_workspace_edit(
            &self.root,
            &crate::signature::whole_file_edit(&files),
        )?;
        self.applied = true;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rewrite {
    pub call: String,
    pub inserted_at: usize,
    pub function_len: usize,
}

impl Rewrite {
    /// Where `old[at..to]`, which the extraction did not touch, is in the new text.
    pub fn mapped(&self, start: usize, end: usize, at: usize, to: usize) -> Option<usize> {
        let grown = self.call.len() as isize - (end - start) as isize;
        if to <= start {
            Some(at)
        } else if end <= at && to <= self.inserted_at {
            Some((at as isize + grown) as usize)
        } else if self.inserted_at <= at {
            Some((at as isize + grown) as usize + self.function_len)
        } else {
            None
        }
    }
}

/// text of every literal at which it differs.
#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub from: usize,
    pub to: usize,
    pub differs: Vec<(usize, String)>,
}
