/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeSet;

use super::output::{MAX_IMPACT_OUTPUT_ITEMS, append_omitted};
use super::types::{CiDecision, CiRun, ImpactReport};

impl ImpactReport {
    /// Why the selection cannot be trusted and the whole suite should run instead: an index
    /// that could not be built, a gap in the analysis, or lines changed outside any function.
    /// `None` when it can.
    pub fn full_suite_reason(&self) -> Option<String> {
        if self.index.as_ref().is_some_and(|b| !b.ok) {
            return Some("the analyzer's index could not be built, so callers are unknown".into());
        }
        if let Some(first) = self.incomplete.first() {
            let more = self.incomplete.len() - 1;
            return Some(if more == 0 {
                format!("the analysis is incomplete: {}", first.describe())
            } else {
                format!(
                    "the analysis is incomplete: {} (and {more} more)",
                    first.describe()
                )
            });
        }
        if !self.unattributed_files.is_empty() {
            return Some(format!(
                "lines changed outside any function in {}",
                self.unattributed_files.join(", ")
            ));
        }
        // Only Rust tests become `cargo test` filters; another language's tests in a Rust-root
        // repository run under their own command (#1471).
        let rust_filter_count = self
            .tests
            .iter()
            .filter(|test| test.file.ends_with(".rs"))
            .map(|test| test.name.rsplit("::").next().unwrap_or(&test.name))
            .collect::<BTreeSet<_>>()
            .len();
        if self.language == "rust" && rust_filter_count > 25 {
            return Some(format!(
                "{rust_filter_count} distinct Rust test filters exceed the selective threshold; the whole suite is faster"
            ));
        }
        None
    }

    /// What `impact --ci` runs: the selection only when it can be trusted, the whole suite when
    /// it cannot or the language cannot select tests, nothing when no test can be affected
    /// (#201, #434).
    pub fn ci_decision(&self) -> CiDecision {
        let reason = self.full_suite_reason();
        if let Some(plan) = &self.test_plan {
            if let Some(reason) = reason {
                if !plan.unsupported_whole.is_empty() {
                    return CiDecision {
                        run: CiRun::Unsupported,
                        why: format!(
                            "{reason}; no whole-suite command for {}",
                            plan.unsupported_whole.join(", ")
                        ),
                    };
                }
                if plan.whole_is_root_only && plan.whole.is_some() {
                    return CiDecision {
                        run: CiRun::WholeSuite,
                        why: format!("the whole suite, because {reason}"),
                    };
                }
                return match &plan.whole {
                    Some(command) => CiDecision {
                        run: CiRun::PlannedSuite(command.clone()),
                        why: format!("whole suites for known affected projects, because {reason}"),
                    },
                    None => CiDecision {
                        run: CiRun::Unsupported,
                        why: format!("{reason}; no affected project suite could be planned"),
                    },
                };
            }
            if !plan.unsupported_selected.is_empty() {
                return CiDecision {
                    run: CiRun::Unsupported,
                    why: format!(
                        "no test command for {}",
                        plan.unsupported_selected.join(", ")
                    ),
                };
            }
            if let Some(command) = &plan.selected {
                let why = if plan.selection_fallbacks.is_empty() {
                    format!("{} test(s) that reach the change", self.tests.len())
                } else {
                    format!(
                        "{} affected test(s); whole suites for {}",
                        self.tests.len(),
                        plan.selection_fallbacks.join(", ")
                    )
                };
                return CiDecision {
                    run: CiRun::Selected(command.clone()),
                    why,
                };
            }
        }
        let (run, why) = match (reason, &self.test_command) {
            (Some(reason), _) => (
                CiRun::WholeSuite,
                format!("the whole suite, because {reason}"),
            ),
            (None, Some(selected)) => (
                CiRun::Selected(selected.clone()),
                format!("{} test(s) that reach the change", self.tests.len()),
            ),
            (None, None) if self.changed.is_empty() => {
                (CiRun::Nothing, "no function changed".to_string())
            }
            (None, None) if self.tests.is_empty() => (
                CiRun::Nothing,
                "no test reaches the changed functions".to_string(),
            ),
            (None, None) => (
                CiRun::WholeSuite,
                "the whole suite, because this language's tests cannot be selected".to_string(),
            ),
        };
        CiDecision { run, why }
    }

    /// The Markdown a CI job shows for this analysis and the command it ran (`why` says why
    /// that command), for `$GITHUB_STEP_SUMMARY`.
    pub fn ci_summary(&self, command: Option<&[String]>, why: &str) -> String {
        let mut out = format!(
            "### prod-code impact of `{}`\n\n{} changed file(s), {} changed function(s), {} test(s) reached.\n\n",
            self.base,
            self.changed_files.len(),
            self.changed.len(),
            self.tests.len()
        );
        if !self.changed.is_empty() {
            out.push_str("| changed function | file |\n|---|---|\n");
            for s in &self.changed {
                out.push_str(&format!("| `{}` | `{}:{}` |\n", s.name, s.file, s.line));
            }
            out.push('\n');
        }
        if !self.tests.is_empty() {
            out.push_str("Tests that reach them:\n\n");
            for s in &self.tests {
                out.push_str(&format!("- `{}` (`{}:{}`)\n", s.name, s.file, s.line));
            }
            out.push('\n');
        }
        if !self.incomplete.is_empty() {
            out.push_str("The analysis is incomplete, so the selection cannot be trusted:\n\n");
            for gap in &self.incomplete {
                out.push_str(&format!("- {}\n", gap.describe()));
            }
            out.push('\n');
        }
        if !self.signature_warnings.is_empty() {
            out.push_str(
                "⚠️ **Signature Warnings**: updated signatures left unadjusted call sites:\n\n",
            );
            for warn in &self.signature_warnings {
                out.push_str(&format!(
                    "- `{}` (`{}:{}`):\n  - Old: `{}`\n  - New: `{}`\n  - Unadjusted call sites ({}):\n",
                    warn.symbol.name,
                    warn.symbol.file,
                    warn.symbol.line,
                    warn.old_signature,
                    warn.new_signature,
                    warn.unadjusted_call_sites.len()
                ));
                for site in &warn.unadjusted_call_sites {
                    let tag = if site.is_sibling { "[sibling] " } else { "" };
                    let caller_str = match &site.caller {
                        Some(c) => format!(" in `{c}`"),
                        None => String::new(),
                    };
                    out.push_str(&format!(
                        "    - {}`{}:{}:{}`{}\n",
                        tag, site.file, site.line, site.col, caller_str
                    ));
                }
            }
            out.push('\n');
        }
        match command {
            Some(c) => out.push_str(&format!("Ran `{}`: {why}.\n", c.join(" "))),
            None => out.push_str(&format!("Ran nothing: {why}.\n")),
        }
        out
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let unindexed = self.index.as_ref().is_some_and(|b| !b.ok);
        let reached = if unindexed {
            "callers and tests unknown".to_string()
        } else if !self.incomplete.is_empty() {
            format!(
                "{} caller(s), {} test(s), incomplete",
                self.callers.len(),
                self.tests.len()
            )
        } else {
            format!(
                "{} caller(s), {} test(s)",
                self.callers.len(),
                self.tests.len()
            )
        };
        out.push_str(&format!(
            "impact of {} ({} changed file(s), {} changed function(s), {reached})\n",
            self.base,
            self.changed_files.len(),
            self.changed.len(),
        ));
        if let Some(b) = &self.index {
            out.push_str(&if b.ok {
                format!(
                    "index: `{}` ({:.1}s) before the call hierarchy\n",
                    b.command,
                    b.duration_ms as f64 / 1000.0
                )
            } else {
                format!(
                    "index: `{}` failed, so the analyzer has no index; callers and tests below are unknown, not none\n",
                    b.command
                )
            });
        }
        if !self.changed.is_empty() {
            out.push_str("changed functions:\n");
            for s in self.changed.iter().take(MAX_IMPACT_OUTPUT_ITEMS) {
                out.push_str(&format!(
                    "  • {}  {}:{}:{}\n",
                    s.name, s.file, s.line, s.col
                ));
            }
            append_omitted(&mut out, self.changed.len(), "changed functions", "  ");
        }
        if !self.signature_warnings.is_empty() {
            out.push_str("signature warnings (unadjusted call sites before full compilation):\n");
            for warn in self.signature_warnings.iter().take(MAX_IMPACT_OUTPUT_ITEMS) {
                out.push_str(&format!(
                    "  ⚠️  `{}` signature changed in {}:{}:{}\n",
                    warn.symbol.name, warn.symbol.file, warn.symbol.line, warn.symbol.col
                ));
                out.push_str(&format!("      old: {}\n", warn.old_signature));
                out.push_str(&format!("      new: {}\n", warn.new_signature));
                let sibling_count = warn
                    .unadjusted_call_sites
                    .iter()
                    .filter(|c| c.is_sibling)
                    .count();
                out.push_str(&format!(
                    "      unadjusted sibling call sites ({}):\n",
                    sibling_count
                ));
                for site in warn
                    .unadjusted_call_sites
                    .iter()
                    .take(MAX_IMPACT_OUTPUT_ITEMS)
                {
                    let caller_str = match &site.caller {
                        Some(c) => format!(" in `{c}`"),
                        None => String::new(),
                    };
                    let tag = if site.is_sibling { "[sibling] " } else { "" };
                    out.push_str(&format!(
                        "        • {}{}:{}:{}{}\n",
                        tag, site.file, site.line, site.col, caller_str
                    ));
                }
                append_omitted(
                    &mut out,
                    warn.unadjusted_call_sites.len(),
                    "call sites",
                    "        ",
                );
            }
            append_omitted(
                &mut out,
                self.signature_warnings.len(),
                "signature warnings",
                "  ",
            );
        }
        if !self.callers.is_empty() {
            out.push_str("reached callers:\n");
            for s in self.callers.iter().take(MAX_IMPACT_OUTPUT_ITEMS) {
                out.push_str(&format!(
                    "  • {}  {}:{}:{}\n",
                    s.name, s.file, s.line, s.col
                ));
            }
            append_omitted(&mut out, self.callers.len(), "callers", "  ");
        }
        if !self.tests.is_empty() {
            out.push_str("affected tests:\n");
            for s in self.tests.iter().take(MAX_IMPACT_OUTPUT_ITEMS) {
                out.push_str(&format!(
                    "  • {}  {}:{}:{}\n",
                    s.name, s.file, s.line, s.col
                ));
            }
            append_omitted(&mut out, self.tests.len(), "tests", "  ");
        } else if unindexed {
            out.push_str("affected tests: unknown (no index); run the full suite\n");
        } else if !self.incomplete.is_empty() {
            out.push_str(
                "affected tests: unknown (the analysis is incomplete); run the full suite\n",
            );
        } else if !self.changed.is_empty() {
            out.push_str("affected tests: none reach the changed functions\n");
        }
        if !self.incomplete.is_empty() {
            out.push_str(
                "incomplete analysis (tests beyond those listed may be affected; run the full suite):\n",
            );
            for gap in self.incomplete.iter().take(MAX_IMPACT_OUTPUT_ITEMS) {
                out.push_str(&format!("  • {}\n", gap.describe()));
            }
            append_omitted(
                &mut out,
                self.incomplete.len(),
                "incomplete-analysis notes",
                "  ",
            );
        }
        if !self.unattributed_files.is_empty() {
            out.push_str(&format!(
                "changes outside functions (run the full suite for these): {}\n",
                self.unattributed_files
                    .iter()
                    .take(MAX_IMPACT_OUTPUT_ITEMS)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            append_omitted(&mut out, self.unattributed_files.len(), "files", "  ");
        }
        // An untrusted selection is not offered as the command: the tests it found are listed
        // above, and what `--run` runs is the whole suite (#1471).
        let decision = self.ci_decision();
        match decision.run {
            CiRun::WholeSuite => {
                if let Some(reason) = self.full_suite_reason() {
                    out.push_str(&format!("run: the whole suite ({reason})\n"));
                } else {
                    out.push_str(&format!("run: {}\n", decision.why));
                }
            }
            CiRun::PlannedSuite(cmd) | CiRun::Selected(cmd) => {
                out.push_str(&format!("run: {} ({})\n", shell_words(&cmd), decision.why));
            }
            CiRun::Unsupported => out.push_str(&format!(
                "run: cannot plan complete coverage ({})\n",
                decision.why
            )),
            CiRun::Nothing => {}
        }
        out
    }
}

pub(crate) fn shell_words(cmd: &[String]) -> String {
    cmd.iter()
        .map(|w| {
            if w.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./=:,".contains(c))
            {
                w.clone()
            } else {
                format!("'{}'", w.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
