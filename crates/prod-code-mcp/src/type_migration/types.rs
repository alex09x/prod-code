/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// One place the new type does not fit, with enough context to judge it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Site {
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// The analyzer's message, first line.
    pub message: String,
    pub code: Option<String>,
    /// The line of source, trimmed.
    pub source: String,
    /// What would fix this site, when the shape of the error says so plainly.
    pub suggestion: Option<String>,
    /// Where the analyzer's range for it ends, 1-based line and column.
    #[serde(skip)]
    pub end: Option<(u32, u32)>,
}

/// A conversion written at a site: the value that was there, and the value that is there now.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Conversion {
    pub file: String,
    pub line: u32,
    pub was: String,
    pub now: String,
}

/// What the migration would do, and what it would leave to be done.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Migration {
    pub symbol: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub was: String,
    pub now: String,
    /// The declaration's file, rewritten.
    pub rewritten: Vec<(String, String)>,
    /// Every site the new type does not fit, in file order.
    pub sites: Vec<Site>,
    /// How many diagnostics the change caused on an attribute rather than on code: an error
    /// inside what a derive generates is reported at the derive, and none of them is a place
    /// anyone can edit. Ones the file already had are not counted at all (#79).
    pub in_attributes: usize,
    /// The sites `convert` turned into conversions the analyzer accepts.
    pub converted: Vec<Conversion>,
    /// Why `convert` wrote nothing although it had candidates, when it did not.
    pub conversion_note: Option<String>,
    pub applied: bool,
    #[serde(default)]
    pub transitive_count: usize,
    #[serde(default)]
    pub transitively_migrated: Vec<String>,
}

impl Migration {
    /// The report: the declaration, then the work the change creates.
    pub fn render(&self, budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: `{}`\n- now: `{}`\n",
            self.symbol, self.file, self.was, self.now
        );
        if !self.converted.is_empty() {
            let label = if self.converted.iter().all(|c| c.now.ends_with(".into()")) {
                "converted with `.into()`"
            } else {
                "converted with language-idiomatic conversion"
            };
            out.push_str(&format!(
                "\n{} site(s) {label}, each accepted by the analyzer:\n",
                self.converted.len()
            ));
            for c in &self.converted {
                out.push_str(&format!(
                    "  {}:{}  `{}` → `{}`\n",
                    c.file, c.line, c.was, c.now
                ));
            }
        }
        if self.transitive_count > 0 {
            out.push_str(&format!(
                "\n{} declaration(s) transitively migrated along data-flow graph:\n",
                self.transitive_count
            ));
            for decl in &self.transitively_migrated {
                out.push_str(&format!("  {decl}\n"));
            }
        }
        if let Some(note) = &self.conversion_note {
            out.push_str(&format!("\n{note}\n"));
        }
        if self.sites.is_empty() {
            out.push_str("\nnothing else has to change: the analyzer accepts the new type\n");
        } else {
            let files = self
                .sites
                .iter()
                .map(|s| s.file.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            out.push_str(&format!(
                "\n{} site(s) in {files} file(s) do not fit the new type:\n",
                self.sites.len()
            ));
            let mut current = String::new();
            for (written, site) in self.sites.iter().enumerate() {
                if site.file != current {
                    current = site.file.clone();
                    out.push_str(&format!("\n{current}\n"));
                }
                if written >= budget {
                    out.push_str(&format!(
                        "  … {} more site(s)\n",
                        self.sites.len() - written
                    ));
                    break;
                }
                out.push_str(&format!(
                    "  {}:{}  {}{}\n      {}\n",
                    site.line,
                    site.col,
                    site.message,
                    site.code
                        .as_deref()
                        .map(|c| format!(" [{c}]"))
                        .unwrap_or_default(),
                    site.source
                ));
                if let Some(fix) = &site.suggestion {
                    out.push_str(&format!("      try: {fix}\n"));
                }
            }
            out.push_str(
                "\nthese are not failures, they are the migration: each site needs a decision \
                 about how the old value becomes the new one. Nothing is suggested that this \
                 cannot see plainly, and a conversion is written only when `convert` is set and \
                 the analyzer accepts it.\n",
            );
        }
        if self.in_attributes > 0 {
            out.push_str(&format!(
                "\n{} further diagnostic(s) landed on a `#[derive(…)]` line rather than on code. \
                 An error inside what a derive generates is reported at the derive, and there \
                 is nothing at those positions to edit; they are left out of the list above.\n",
                self.in_attributes
            ));
        }
        if self.applied && !self.converted.is_empty() {
            let rest = if self.sites.is_empty() {
                ""
            } else {
                "; the sites above were not"
            };
            out.push_str(&format!(
                "\n[the declaration and {} conversion(s) were written{rest}]\n",
                self.converted.len()
            ));
        } else if self.applied {
            out.push_str("\n[the declaration was written; the sites above were not]\n");
        } else if !self.converted.is_empty() {
            out.push_str(
                "\nnothing was written; pass `apply: true` to write the declaration and the \
                 conversions, and `force: true` while sites remain\n",
            );
        } else {
            out.push_str(
                "\nnothing was written; pass `apply: true` to write the declaration alone, and \
                 `force: true` while sites remain\n",
            );
        }
        out
    }
}

pub(crate) enum Converted {
    Accepted {
        texts: BTreeMap<PathBuf, String>,
        conversions: Vec<Conversion>,
        reports: Vec<crate::diagnostics::DiagnosticsReport>,
        tried: BTreeSet<(String, u32, u32)>,
    },
    Nothing {
        tried: BTreeSet<(String, u32, u32)>,
    },
    Dropped {
        note: String,
        tried: BTreeSet<(String, u32, u32)>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct EnclosingFn {
    pub(crate) name: String,
    pub(crate) return_type: String,
    pub(crate) ret_start: usize,
    pub(crate) ret_end: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ParamInfo {
    pub(crate) name: String,
    pub(crate) ty: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}
