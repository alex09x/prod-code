/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureSite {
    pub file: String,
    pub line: u32,
    /// Lines around the site, numbered, target marked with `>`.
    pub snippet: String,
    /// Enclosing function, when the analyzer finds one.
    pub function: Option<String>,
    /// Direct callers of that function.
    pub callers: Vec<String>,
    /// `git diff HEAD` hunks of this file, when it changed.
    pub diff: Option<String>,
}

/// Structured runtime assertion evidence extracted from failure output (roadmap 8.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionEvidence {
    /// Supported assertion format, e.g. "assert_eq", "assert_ne", "strictEqual", "deepStrictEqual".
    pub format: String,
    /// The expression from the assertion if actually present in failure output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    /// Exact printed actual value, only where the assertion itself names that role (Node's
    /// `actual`); Rust's `assert_eq!` takes either order, so it leaves this unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// Exact printed expected value, under the same rule as `actual`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// Exact printed left operand as a string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    /// Exact printed right operand as a string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
    /// Exact printed operands in order as strings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operands: Vec<String>,
    /// Original excerpt of the failure output containing the assertion evidence.
    #[serde(alias = "evidence")]
    pub excerpt: String,
}

impl AssertionEvidence {
    /// Render a compact summary of the assertion evidence to avoid repeating large outputs.
    pub fn render_compact(&self) -> String {
        let short_val = |v: &str| -> String {
            let lines: Vec<&str> = v.lines().collect();
            if lines.len() <= 1 && v.len() <= 60 {
                v.to_string()
            } else if lines.len() <= 3 && v.len() <= 120 {
                lines.join(" ")
            } else {
                let first = lines.first().copied().unwrap_or("").trim();
                format!("{first} … ({} lines)", lines.len())
            }
        };

        let label = match &self.expression {
            Some(expr) => format!("{} ({expr})", self.format),
            None => self.format.clone(),
        };
        match (&self.actual, &self.expected, &self.left, &self.right) {
            (Some(actual), Some(expected), _, _) => format!(
                "assertion [{label}]: actual: {}, expected: {}\n",
                short_val(actual),
                short_val(expected)
            ),
            (_, _, Some(left), Some(right)) => format!(
                "assertion [{label}]: left: {}, right: {}\n",
                short_val(left),
                short_val(right)
            ),
            _ => match &self.expression {
                Some(expr) => format!("assertion [{}]: {expr}\n", self.format),
                None => format!("assertion [{}]\n", self.format),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureDossier {
    pub test: String,
    pub output: String,
    pub sites: Vec<FailureSite>,
    /// Changed functions whose callers reach this test, nearest first, with the hops and the
    /// diff of their file when no site above already shows it.
    pub suspects: Vec<Suspect>,
    /// Optional backward-compatible structured runtime assertion evidence.
    pub assertion: Option<AssertionEvidence>,
    /// Exact line of the failure/panic from the stack trace or failure site (Roadmap 8.2).
    pub panic_line: Option<u32>,
    /// Extracted assertion expression or condition under test (Roadmap 8.2).
    pub expression: Option<String>,
}

impl Serialize for FailureDossier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("FailureDossier", 11)?;
        state.serialize_field("test", &self.test)?;
        state.serialize_field("failing_test", &self.test)?;
        state.serialize_field("output", &self.output)?;
        state.serialize_field("sites", &self.sites)?;
        state.serialize_field("suspects", &self.suspects)?;
        state.serialize_field("suspect_recent_changes", &self.suspects)?;
        if let Some(ref a) = self.assertion {
            state.serialize_field("assertion", a)?;
            state.serialize_field("runtime_values", a)?;
        }
        if let Some(p) = self.panic_line {
            state.serialize_field("panic_line", &p)?;
        }
        if let Some(ref e) = self.expression {
            state.serialize_field("expression", e)?;
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for FailureDossier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawDossier {
            #[serde(default)]
            test: Option<String>,
            #[serde(default)]
            failing_test: Option<String>,
            #[serde(default)]
            output: Option<String>,
            #[serde(default)]
            sites: Vec<FailureSite>,
            #[serde(default)]
            suspects: Option<Vec<Suspect>>,
            #[serde(default)]
            suspect_recent_changes: Option<Vec<Suspect>>,
            #[serde(default)]
            assertion: Option<AssertionEvidence>,
            #[serde(default)]
            runtime_values: Option<AssertionEvidence>,
            #[serde(default)]
            structured_assertion: Option<AssertionEvidence>,
            #[serde(default)]
            panic_line: Option<u32>,
            #[serde(default)]
            expression: Option<String>,
        }

        let raw = RawDossier::deserialize(deserializer)?;
        let test = raw
            .failing_test
            .or(raw.test)
            .ok_or_else(|| serde::de::Error::missing_field("test or failing_test"))?;
        let output = raw.output.unwrap_or_default();
        let suspects = raw
            .suspect_recent_changes
            .or(raw.suspects)
            .unwrap_or_default();
        let assertion = raw
            .runtime_values
            .or(raw.assertion)
            .or(raw.structured_assertion);

        Ok(FailureDossier {
            test,
            output,
            sites: raw.sites,
            suspects,
            assertion,
            panic_line: raw.panic_line,
            expression: raw.expression,
        })
    }
}

impl FailureDossier {
    pub fn failing_test(&self) -> &str {
        &self.test
    }

    pub fn runtime_values(&self) -> Option<&AssertionEvidence> {
        self.assertion.as_ref()
    }

    pub fn suspect_recent_changes(&self) -> &[Suspect] {
        &self.suspects
    }
}

/// A changed function on the path to a failing test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suspect {
    pub function: String,
    pub file: String,
    pub line: u32,
    pub hops: usize,
    pub diff: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DossierReport {
    pub command: Vec<String>,
    /// Files changed in the working tree (the usual suspects).
    pub changed_files: Vec<String>,
    pub tests_passed: u64,
    pub tests_failed: u64,
    pub dossiers: Vec<FailureDossier>,
    /// Compiler diagnostics when the tests did not even build.
    pub build_errors: Vec<String>,
    /// The compiler's own machine-applicable fixes for those build errors, `file:line: what`
    /// (Rust), which `prod-code check --fix` applies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_fixes: Vec<String>,
    pub tail: String,
}

impl DossierReport {
    pub fn render(&self) -> String {
        let mut out = format!(
            "$ {}\n{} passed, {} failed\n",
            self.command.join(" "),
            self.tests_passed,
            self.tests_failed
        );
        if !self.changed_files.is_empty() {
            out.push_str(&format!(
                "changed in the working tree: {}\n",
                self.changed_files.join(", ")
            ));
        }
        if !self.build_errors.is_empty() {
            out.push_str("build errors:\n");
            for e in &self.build_errors {
                out.push_str(&format!("  {e}\n"));
            }
        }
        if !self.suggested_fixes.is_empty() {
            out.push_str(
                "suggested fixes (the compiler's own, machine-applicable; `prod-code check --fix` applies them):\n",
            );
            for f in &self.suggested_fixes {
                out.push_str(&format!("  {f}\n"));
            }
        }
        for d in &self.dossiers {
            out.push_str(&format!("\n=== {} ===\n", d.test));
            let msg: Vec<&str> = d.output.lines().take(12).collect();
            out.push_str(&msg.join("\n"));
            out.push('\n');
            if let Some(assertion) = &d.assertion {
                out.push_str(&assertion.render_compact());
            }
            for site in &d.sites {
                out.push_str(&format!("--- {}:{}", site.file, site.line));
                if let Some(f) = &site.function {
                    out.push_str(&format!("  in {f}"));
                }
                out.push('\n');
                out.push_str(&site.snippet);
                if !site.callers.is_empty() {
                    out.push_str(&format!("callers: {}\n", site.callers.join(", ")));
                }
                if let Some(diff) = &site.diff {
                    out.push_str("changed in the working tree:\n");
                    out.push_str(diff);
                    if !diff.ends_with('\n') {
                        out.push('\n');
                    }
                }
            }
            if !d.suspects.is_empty() {
                out.push_str("suspects (changed functions that reach this test, nearest first):\n");
                for s in &d.suspects {
                    out.push_str(&format!(
                        "  • {}  {}:{}  ({} call{} away)\n",
                        s.function,
                        s.file,
                        s.line,
                        s.hops,
                        if s.hops == 1 { "" } else { "s" }
                    ));
                }
                for s in &d.suspects {
                    if let Some(diff) = &s.diff {
                        out.push_str(&format!("changed in {}:\n{diff}", s.file));
                        if !diff.ends_with('\n') {
                            out.push('\n');
                        }
                    }
                }
            }
        }
        if self.dossiers.is_empty() && self.tests_failed == 0 && self.build_errors.is_empty() {
            out.push_str("no failures\n");
        } else if self.dossiers.is_empty() {
            out.push_str("--- output tail ---\n");
            out.push_str(&self.tail);
        }
        out
    }
}
