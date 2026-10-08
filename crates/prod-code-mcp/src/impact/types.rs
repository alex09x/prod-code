/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::Serialize;

pub const CARGO_TEST_THRESHOLD: usize = 8;
pub const FAN_IN_FALLBACK_THRESHOLD: usize = 30;
pub const COLD_RETRIES: usize = 3;
pub const COLD_WAIT: std::time::Duration = std::time::Duration::from_millis(800);

/// A function/method the change touches or reaches.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Symbol {
    pub name: String,
    /// Checkout-relative path.
    pub file: String,
    /// 1-based line of the name.
    pub line: u32,
    /// 1-based column of the name.
    pub col: u32,
}

/// A call site calling a function whose signature changed.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct CallSite {
    /// Checkout-relative path of the calling file.
    pub file: String,
    /// 1-based line of the call.
    pub line: u32,
    /// 1-based column of the call.
    pub col: u32,
    /// Enclosing caller function name, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<String>,
    /// Whether this call site is in a sibling file (a different file than the declared function).
    pub is_sibling: bool,
}

/// A proactive warning that an updated function signature left unadjusted call sites
/// in sibling files before full compilation is attempted (Roadmap 8.1).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SignatureWarning {
    /// The function whose signature was adjusted.
    pub symbol: Symbol,
    /// The signature in the base revision.
    pub old_signature: String,
    /// The signature in the current working tree.
    pub new_signature: String,
    /// Call sites that were left unadjusted in the diff.
    pub unadjusted_call_sites: Vec<CallSite>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactReport {
    pub language: String,
    pub base: String,
    pub changed_files: Vec<String>,
    /// Functions whose bodies the diff touches.
    pub changed: Vec<Symbol>,
    /// Callers reached from the changed functions (transitively), tests excluded.
    pub callers: Vec<Symbol>,
    /// Test functions reached.
    pub tests: Vec<Symbol>,
    /// Command that runs only the affected tests, when the language supports selection.
    pub test_command: Option<Vec<String>>,
    /// Files whose changed lines lie outside any function (module-level code, manifests).
    pub unattributed_files: Vec<String>,
    /// How the analyzer's index was brought up to date first, when it had to be (Swift), and
    /// whether that worked: when it did not, no callers means unknown callers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexBuild>,
    /// Which changed function reaches which test, in how many calls: the suspects of a failure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reaches: Vec<Reach>,
    /// What the analysis could not establish: a test beyond those listed may reach the change,
    /// so only the whole suite can be trusted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incomplete: Vec<Gap>,
    /// Proactive warnings when an updated function signature left unadjusted call sites
    /// in sibling files before full compilation is attempted (Roadmap 8.1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signature_warnings: Vec<SignatureWarning>,
}

/// Something the analysis could not establish, so a test that reaches the change may be
/// missing from the selection.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Gap {
    /// A changed file is gone: what called into it can no longer be asked.
    Deleted { file: String },
    /// A changed source file whose change cannot be placed in its lines: a binary file, or a
    /// path or a hunk the diff could not be read for.
    Diff { file: String, error: String },
    /// The functions of a changed source file could not be listed.
    Symbols { file: String, error: String },
    /// The callers of a function could not be asked, or the answer could not be read.
    Callers { symbol: Symbol, error: String },
    /// The walk stopped at the depth limit while a function still had callers.
    Depth { symbol: Symbol, depth: usize },
    /// The callers of a function exceeded the fan-in limit (shared dispatcher / hub).
    FanIn {
        symbol: Symbol,
        callers: usize,
        limit: usize,
    },
    /// The impact analysis traversal reached the global time budget.
    Timeout { elapsed_ms: u64, budget_ms: u64 },
}

impl Gap {
    pub fn describe(&self) -> String {
        match self {
            Gap::Deleted { file } => {
                format!("{file} was deleted, so what called into it is unknown")
            }
            Gap::Diff { file, error } => {
                format!("the lines the change to {file} touches are unknown: {error}")
            }
            Gap::Symbols { file, error } => {
                format!("the functions of {file} could not be listed: {error}")
            }
            Gap::Callers { symbol, error } => format!(
                "the callers of {} ({}:{}) are unknown: {error}",
                symbol.name, symbol.file, symbol.line
            ),
            Gap::Depth { symbol, depth } => format!(
                "the walk stopped at depth {depth} while {} ({}:{}) still had callers",
                symbol.name, symbol.file, symbol.line
            ),
            Gap::FanIn {
                symbol,
                callers,
                limit,
            } => format!(
                "{} ({}:{}) has {callers} callers, exceeding the fan-in limit of {limit}; stopped to avoid dispatcher explosion",
                symbol.name, symbol.file, symbol.line
            ),
            Gap::Timeout {
                elapsed_ms,
                budget_ms,
            } => format!(
                "the impact analysis traversal reached its time budget ({elapsed_ms}ms >= {budget_ms}ms); returning accumulated callers and tests"
            ),
        }
    }
}

/// What `impact --ci` runs for a report, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiDecision {
    pub run: CiRun,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiRun {
    /// The language's whole test suite.
    WholeSuite,
    /// This command, which runs only the tests that reach the change.
    Selected(Vec<String>),
    /// Nothing: no function changed, or no test reaches one.
    Nothing,
}

/// A test the walk from a changed function reached, and in how many calls.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct Reach {
    pub test: Symbol,
    pub changed: Symbol,
    pub hops: usize,
}

/// The build that gives sourcekit-lsp its index: Swift 5 finds callers only through it (#166).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct IndexBuild {
    pub command: String,
    pub ok: bool,
    pub duration_ms: u64,
}
