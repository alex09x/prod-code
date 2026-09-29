//! Failure dossier (roadmap 8.2): run the tests (or one filter), and for every failure
//! collect where it happened, the code there, the enclosing function's callers and what
//! changed in that file, so an agent gets the whole picture in one call.

use crate::session::LspSession;
use crate::verify::{VerifyKind, VerifyReport, run_verify};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureDossier {
    pub test: String,
    pub output: String,
    pub sites: Vec<FailureSite>,
    /// Changed functions whose callers reach this test, nearest first, with the hops and the
    /// diff of their file when no site above already shows it.
    #[serde(default)]
    pub suspects: Vec<Suspect>,
    /// Optional backward-compatible structured runtime assertion evidence.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "runtime_values",
        alias = "structured_assertion"
    )]
    pub assertion: Option<AssertionEvidence>,
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

/// `file:line` locations mentioned in a failure's output that lie inside the checkout, in
/// order of appearance, deduplicated: Rust panics and `-->` notes, Go `file.go:12:`, Python
/// `File "x.py", line 12`, JS/TS `(x.ts:12:5)`, Swift/C `x.swift:12: error`.
pub fn locations_in(root: &Path, text: &str) -> Vec<(String, u32)> {
    locations_in_with_hint(root, text, "")
}

/// [`locations_in`] with a hint (the failing test's name, e.g. `pkg/sub.TestX`) used to
/// resolve bare file names such as Go's `signal_test.go:7` to the right directory.
pub fn locations_in_with_hint(root: &Path, text: &str, hint: &str) -> Vec<(String, u32)> {
    let root_str = std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned();
    // Every source file of the checkout, for resolving bare file names.
    let all_files: Vec<String> = crate::sync::scan_workspace_files(root, None)
        .map(|files| files.into_iter().map(|f| f.relative_path).collect())
        .unwrap_or_default();
    let hint_segments: Vec<&str> = hint
        .split(['/', '.', ':'])
        .filter(|s| !s.is_empty())
        .collect();
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut push = |file: &str, line: u32| {
        let file = file.trim_matches(|c| c == '"' || c == '(' || c == ')' || c == '\'');
        let mut rel = file
            .strip_prefix(&format!("{root_str}/"))
            .unwrap_or(file)
            .to_string();
        if rel.starts_with('/')
            || rel.starts_with("..")
            || rel.contains("/.cargo/")
            || rel.contains("/rustlib/")
        {
            return;
        }
        if !root.join(&rel).is_file() {
            // A bare name: the unique file with that basename, or the one whose directory
            // matches the test's package.
            if rel.contains('/') {
                return;
            }
            let candidates: Vec<&String> = all_files
                .iter()
                .filter(|p| p.rsplit('/').next() == Some(rel.as_str()))
                .collect();
            let chosen = match candidates.as_slice() {
                [] => return,
                [one] => (*one).clone(),
                many => many
                    .iter()
                    .find(|p| {
                        hint_segments
                            .iter()
                            .any(|seg| p.split('/').any(|part| part == *seg))
                    })
                    .map(|p| (*p).clone())
                    .unwrap_or_else(|| (*many[0]).clone()),
            };
            rel = chosen;
        }
        if seen.insert((rel.clone(), line)) {
            out.push((rel, line));
        }
    };
    for raw in text.lines() {
        // Python: File "path", line N
        if let Some(rest) = raw.trim().strip_prefix("File \"")
            && let Some((file, rest)) = rest.split_once("\", line ")
            && let Some(n) = rest
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|n| n.parse().ok())
        {
            push(file, n);
            continue;
        }
        // Everything else: tokens shaped path:line[:col]
        for token in raw.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
            let mut parts = token.splitn(3, ':');
            let (Some(file), Some(line)) = (parts.next(), parts.next()) else {
                continue;
            };
            if !file.contains('.') || file.starts_with("http") {
                continue;
            }
            let Ok(n) = line
                .trim_end_matches(|c: char| !c.is_ascii_digit())
                .parse::<u32>()
            else {
                continue;
            };
            if n == 0 {
                continue;
            }
            push(file, n);
        }
    }
    out
}

fn git_diff_of(root: &Path, file: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "-U3", "--no-color", "HEAD", "--", file])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let hunks: Vec<&str> = text.lines().skip_while(|l| !l.starts_with("@@")).collect();
    if hunks.is_empty() {
        None
    } else {
        let mut s = hunks.join("\n");
        if s.len() > 4000 {
            s.truncate(4000);
            s.push_str("\n…");
        }
        Some(s)
    }
}

/// The error-level fixes among `fixes`, one line each: `file:line: message`.
pub fn suggested(fixes: &[crate::fixit::Fix]) -> Vec<String> {
    let mut out: Vec<String> = fixes
        .iter()
        .filter(|f| f.level == "error")
        .filter_map(|f| {
            let first = f.edits.first()?;
            Some(format!(
                "{}:{}: {}",
                first.file,
                first.line,
                f.message.lines().next().unwrap_or("")
            ))
        })
        .collect();
    out.dedup();
    out
}

/// Runs the tests (all, or `filter`) and builds a dossier for every failure.
pub async fn diagnose(
    remote: SocketAddr,
    root: &Path,
    hint: Option<&Path>,
    filter: Option<&str>,
    timeout_secs: u64,
) -> Result<DossierReport> {
    let project_hint = hint.or(Some(root));
    let report: VerifyReport = run_verify(
        remote,
        root,
        project_hint,
        VerifyKind::Test,
        filter,
        timeout_secs,
    )
    .await?;
    let build_errors: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.level == "error" && !d.message.starts_with("test failed"))
        .map(|d| {
            format!(
                "{}{}",
                d.message.lines().next().unwrap_or(""),
                d.file
                    .as_deref()
                    .map(|f| format!(" ({f}:{})", d.line.unwrap_or(0)))
                    .unwrap_or_default()
            )
        })
        .collect();
    // Tests that do not build fail for a reason the compiler may already know how to fix: its
    // machine-applicable suggestions for the errors are the fixes to suggest.
    let suggested_fixes = if build_errors.is_empty() || report.language != "rust" {
        Vec::new()
    } else {
        run_verify(
            remote,
            root,
            project_hint,
            VerifyKind::Check,
            None,
            timeout_secs,
        )
        .await
        .map(|check| suggested(&check.fixes))
        .unwrap_or_default()
    };
    let mut dossiers = Vec::new();
    if !report.failures.is_empty() {
        let mut session = LspSession::open(remote, root, None).await.ok();
        for failure in report.failures.iter().take(10) {
            let mut sites = Vec::new();
            let mut locations = locations_in_with_hint(root, &failure.output, &failure.name);
            // pytest node ids (`tests/test_x.py::test_y`) name the file but no line: point at
            // the test function itself.
            if locations.is_empty()
                && let Some((file, rest)) = failure.name.split_once("::")
                && root.join(file).is_file()
            {
                let func = rest.rsplit("::").next().unwrap_or(rest).to_string();
                let line = session_symbol_line(session.as_mut(), &root.join(file), &func)
                    .await
                    .unwrap_or(1);
                locations.push((file.to_string(), line));
            }
            for (file, line) in locations.into_iter().take(3) {
                let abs = root.join(&file);
                let text = std::fs::read_to_string(&abs).unwrap_or_default();
                let snippet = crate::remote_fs::snippet(&text, line, 6);
                let mut function = None;
                let mut callers = Vec::new();
                if let Some(session) = session.as_mut()
                    && let Ok(uri) = session.uri_for(&abs)
                    && let Ok(symbols) = session
                        .query(
                            &abs,
                            "textDocument/documentSymbol",
                            serde_json::json!({ "textDocument": { "uri": uri.clone() } }),
                        )
                        .await
                {
                    let mut functions = Vec::new();
                    collect_functions(
                        symbols.as_array().map(|a| a.as_slice()).unwrap_or(&[]),
                        &mut functions,
                    );
                    if let Some((name, _, _, sl, sc)) = functions
                        .iter()
                        .filter(|(_, start, end, _, _)| *start <= line && line <= *end)
                        .min_by_key(|(_, start, end, _, _)| end - start)
                        .cloned()
                    {
                        function = Some(name.clone());
                        let position = serde_json::json!({ "line": sl - 1, "character": sc - 1 });
                        if let Ok(items) = session
                            .query(&abs, "textDocument/prepareCallHierarchy", serde_json::json!({ "textDocument": { "uri": uri.clone() }, "position": position }))
                            .await
                            && let Some(item) = items.as_array().and_then(|a| a.first()).cloned()
                            && let Ok(incoming) = session
                                .query(&abs, "callHierarchy/incomingCalls", serde_json::json!({ "item": item }))
                                .await
                        {
                            for edge in incoming.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                                if let Some(n) = edge.get("from").and_then(|f| f.get("name")).and_then(|n| n.as_str()) {
                                    callers.push(n.to_string());
                                }
                            }
                        }
                    }
                }
                sites.push(FailureSite {
                    diff: git_diff_of(root, &file),
                    file,
                    line,
                    snippet,
                    function,
                    callers,
                });
            }
            // Only this failure's own output: the run's tail holds other tests' assertions.
            let assertion = parse_assertion_evidence(&failure.output);
            dossiers.push(FailureDossier {
                test: failure.name.clone(),
                output: failure.output.clone(),
                sites,
                suspects: Vec::new(),
                assertion,
            });
        }
        if let Some(session) = session {
            session.close().await;
        }
        // Which changed function reaches which failing test, through the callers graph.
        if let Ok(impact) = crate::impact::analyze(remote, root, None, 4).await {
            for d in &mut dossiers {
                let shown: BTreeSet<String> = d.sites.iter().map(|s| s.file.clone()).collect();
                let mut diffed = BTreeSet::new();
                d.suspects = crate::impact::suspects_for(&impact.reaches, &d.test)
                    .into_iter()
                    .map(|(sym, hops)| Suspect {
                        diff: (!shown.contains(&sym.file) && diffed.insert(sym.file.clone()))
                            .then(|| git_diff_of(root, &sym.file))
                            .flatten(),
                        function: sym.name,
                        file: sym.file,
                        line: sym.line,
                        hops,
                    })
                    .collect();
            }
        }
    }
    let changed_files: Vec<String> = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--name-only", "HEAD"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    Ok(DossierReport {
        command: report.command.clone(),
        changed_files,
        tests_passed: report.tests_passed,
        tests_failed: report.tests_failed,
        dossiers,
        build_errors,
        suggested_fixes,
        tail: report.tail.clone(),
    })
}

/// The 1-based line of function `name` in `file`, through the session's document symbols.
async fn session_symbol_line(
    session: Option<&mut LspSession>,
    file: &Path,
    name: &str,
) -> Option<u32> {
    let session = session?;
    let uri = session.uri_for(file).ok()?;
    let symbols = session
        .query(
            file,
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await
        .ok()?;
    let mut functions = Vec::new();
    collect_functions(
        symbols.as_array().map(|a| a.as_slice()).unwrap_or(&[]),
        &mut functions,
    );
    functions
        .iter()
        .find(|(n, _, _, _, _)| n == name || n.starts_with(&format!("{name}(")))
        .map(|(_, _, _, sl, _)| *sl)
}

fn collect_functions(symbols: &[serde_json::Value], out: &mut Vec<(String, u32, u32, u32, u32)>) {
    for sym in symbols {
        let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
        let name = sym
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        let range = sym
            .get("range")
            .or_else(|| sym.get("location").and_then(|l| l.get("range")));
        let sel = sym.get("selectionRange").or(range);
        if matches!(kind, 6 | 9 | 12)
            && let (Some(range), Some(sel)) = (range, sel)
            && let (Some(start), Some(end), Some(ss)) =
                (range.get("start"), range.get("end"), sel.get("start"))
        {
            let l = |v: &serde_json::Value, k: &str| {
                v.get(k).and_then(|x| x.as_u64()).unwrap_or(0) as u32 + 1
            };
            out.push((
                name,
                l(start, "line"),
                l(end, "line"),
                l(ss, "line"),
                l(ss, "character"),
            ));
        }
        if let Some(children) = sym.get("children").and_then(|c| c.as_array()) {
            collect_functions(children, out);
        }
    }
}

/// Strips ANSI CSI and OSC escape sequences from `s`.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if (0x40..=0x7E).contains(&(next as u32)) {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next == '\x07' {
                        break;
                    }
                    if next == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parses structured assertion evidence from one test's failure output: Rust `assert_eq!` /
/// `assert_ne!` panics, and Node `assert.strictEqual` / `assert.deepStrictEqual` failures as
/// Node prints the error's own fields, as jest reprints them, or as Node's short `a !== b`
/// message. A value is taken only where the printed layout shows where it starts and ends; an
/// incomplete block or a value the printer elided yields `None`. Nothing is evaluated.
pub fn parse_assertion_evidence(output: &str) -> Option<AssertionEvidence> {
    if output.trim().is_empty() {
        return None;
    }
    let raw: Vec<&str> = output.lines().collect();
    let stripped: Vec<String> = raw.iter().map(|l| strip_ansi(l)).collect();
    // Each format answers `None` when it is absent and `Some(None)` when it is present but
    // cannot be bound completely: then no looser format may take part of the same block.
    parse_rust_assertion(&raw, &stripped)
        .or_else(|| parse_node_error_fields(&raw, &stripped))
        .or_else(|| parse_jest_node_assert(&raw, &stripped))
        .or_else(|| parse_node_short_message(&raw, &stripped))
        .flatten()
}

/// The raw lines `from..=to`, escape sequences included.
fn raw_excerpt(raw: &[&str], from: usize, to: usize) -> String {
    raw[from..=to].join("\n")
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// `assert_eq!` / `assert_ne!` since Rust 1.73:
/// "assertion `left == right` failed[: message]\n  left: {:?}\n right: {:?}". The operands keep
/// the macro's names: either one may be the expected value, so no actual/expected is claimed.
fn parse_rust_assertion(raw: &[&str], stripped: &[String]) -> Option<Option<AssertionEvidence>> {
    let (header, is_ne) = stripped.iter().enumerate().find_map(|(i, line)| {
        let (is_ne, rest) =
            if let Some(rest) = line.strip_prefix("assertion `left == right` failed") {
                (false, rest)
            } else {
                (true, line.strip_prefix("assertion `left != right` failed")?)
            };
        (rest.trim_end().is_empty() || rest.starts_with(": ")).then_some((i, is_ne))
    })?;
    Some(rust_operands(raw, stripped, header, is_ne))
}

/// The operands after a Rust assertion header. A `Debug` value may hold blank lines, so only
/// the panic hook's own trailer (the backtrace note, a backtrace, the next panic, which the
/// hook starts with a blank line) or the end of the test's libtest block ends the message.
/// libtest closes a block with one blank line, two before its `failures:` list: a block
/// without one was cut short, and a third belongs to the value, whose own trailing line breaks
/// are then unknown.
fn rust_operands(
    raw: &[&str],
    stripped: &[String],
    header: usize,
    is_ne: bool,
) -> Option<AssertionEvidence> {
    let trailer = |line: &str| {
        line.starts_with("note: run with ")
            || line.starts_with("stack backtrace:")
            || line.starts_with("thread '")
    };
    let next_block = |line: &str| {
        line == "failures:" || (line.starts_with("---- ") && line.ends_with(" stdout ----"))
    };
    let stop =
        (header + 1..stripped.len()).find(|&i| trailer(&stripped[i]) || next_block(&stripped[i]));
    let end = match stop {
        Some(at) if stripped[at].starts_with("thread '") && stripped[at - 1].is_empty() => at - 1,
        Some(at) if trailer(&stripped[at]) => at,
        stop => {
            let stop = stop.unwrap_or(stripped.len());
            let content = (header + 1..stop).rfind(|&i| !stripped[i].trim().is_empty())? + 1;
            if !(1..=2).contains(&(stop - content)) {
                return None;
            }
            content
        }
    };
    // A custom message may hold "  left: " lines of its own: the operands start at the last
    // one. Two " right: " lines after it leave the split between the operands unknown.
    let (left_at, right_at) = (header + 1..end)
        .rev()
        .filter(|&j| stripped[j].starts_with("  left: "))
        .find_map(|j| {
            let mut rights = (j + 1..end).filter(|&k| stripped[k].starts_with(" right: "));
            let first = rights.next()?;
            Some(rights.next().is_none().then_some((j, first)))
        })??;
    let operand = |first: &str, rest: &[String]| {
        std::iter::once(first)
            .chain(rest.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let left = operand(
        &stripped[left_at]["  left: ".len()..],
        &stripped[left_at + 1..right_at],
    );
    let right = operand(
        &stripped[right_at][" right: ".len()..],
        &stripped[right_at + 1..end],
    );
    // A block cut after one of a value's blank lines still leaves its brackets open.
    if [&left, &right]
        .iter()
        .any(|v| v.trim().is_empty() || !closes(v, true))
    {
        return None;
    }
    Some(AssertionEvidence {
        format: if is_ne { "assert_ne" } else { "assert_eq" }.to_string(),
        expression: Some(
            if is_ne {
                "left != right"
            } else {
                "left == right"
            }
            .to_string(),
        ),
        actual: None,
        expected: None,
        left: Some(left.clone()),
        right: Some(right.clone()),
        operands: vec![left, right],
        excerpt: raw_excerpt(raw, header, end - 1),
    })
}

/// Node's `assert` names its operands: the first argument is `actual`, the second `expected`.
fn node_evidence(
    format: &str,
    expression: Option<String>,
    actual: String,
    expected: String,
    excerpt: String,
) -> AssertionEvidence {
    AssertionEvidence {
        format: format.to_string(),
        expression,
        actual: Some(actual.clone()),
        expected: Some(expected.clone()),
        left: Some(actual.clone()),
        right: Some(expected.clone()),
        operands: vec![actual, expected],
        excerpt,
    }
}

/// Whether Node's `util.inspect` shortened a value: objects past its depth, and the tails of
/// long arrays and strings.
fn inspect_elided(value: &str) -> bool {
    value.contains("[Object]")
        || value.contains("[Array]")
        || (value.contains("... ")
            && (value.contains(" more item") || value.contains(" more character")))
}

/// The error's own properties as Node prints an uncaught or test-runner `AssertionError`: the
/// last stack frame opens `{`, top-level fields sit two columns in from the error header, and
/// `}` at the header's column closes the block. Only those top-level fields are bound, so an
/// `expected:` or `operator:` key nested in a value, or printed in the diff above, is never
/// taken for the error's own. Without the closing brace the block is incomplete, and the short
/// message above it is not taken instead.
fn parse_node_error_fields(raw: &[&str], stripped: &[String]) -> Option<Option<AssertionEvidence>> {
    let header = stripped.iter().position(|l| {
        l.trim_start()
            .starts_with("AssertionError [ERR_ASSERTION]: ")
    })?;
    let base = indent_of(&stripped[header]);
    let open = (header + 1..stripped.len()).find(|&i| {
        let line = stripped[i].trim_end();
        indent_of(line) == base + 4 && line.trim_start().starts_with("at ") && line.ends_with(" {")
    })?;
    Some(node_error_fields(raw, stripped, header, base, open))
}

fn node_error_fields(
    raw: &[&str],
    stripped: &[String],
    header: usize,
    base: usize,
    open: usize,
) -> Option<AssertionEvidence> {
    let mut fields: Vec<(String, Vec<String>)> = Vec::new();
    let mut close = None;
    for (i, line) in stripped.iter().enumerate().skip(open + 1) {
        let line = line.trim_end();
        let indent = indent_of(line);
        let text = line.trim_start_matches(' ');
        if text.is_empty() {
            return None;
        }
        if indent == base && text == "}" {
            close = Some(i);
            break;
        }
        if indent < base + 2 {
            return None;
        }
        if indent == base + 2 && !text.starts_with(['}', ']', ')']) {
            let (key, value) = text.split_once(": ")?;
            fields.push((key.to_string(), vec![value.to_string()]));
        } else {
            fields.last_mut()?.1.push(line.to_string());
        }
    }
    let close = close?;
    let value_of = |name: &str| -> Option<String> {
        let mut found = fields
            .iter()
            .enumerate()
            .filter(|(_, (key, _))| key == name);
        let (at, (_, lines)) = found.next()?;
        if found.next().is_some() {
            return None;
        }
        let joined = lines.join("\n");
        // Every field but the last ends with a comma.
        let value = if at + 1 < fields.len() {
            joined.strip_suffix(',')?
        } else if joined.ends_with(',') {
            return None;
        } else {
            joined.as_str()
        };
        (!value.trim().is_empty() && !inspect_elided(value)).then(|| value.to_string())
    };
    let format = match value_of("operator")?.as_str() {
        "'strictEqual'" => "strictEqual",
        "'deepStrictEqual'" => "deepStrictEqual",
        _ => return None,
    };
    let actual = value_of("actual")?;
    let expected = value_of("expected")?;
    let expression = node_short_pair(stripped, header).map(|(_, line, _, _)| line);
    Some(node_evidence(
        format,
        expression,
        actual,
        expected,
        raw_excerpt(raw, header, close),
    ))
}

/// Node's message for two short unequal primitives: "Expected values to be strictly equal:",
/// then `actual !== expected` on the next non-blank line. Exactly one ` !== ` makes the split
/// unambiguous; a quoted value that holds that text gives two and is refused. Output that ends
/// at the pair may have cut it: a line after it shows it was printed whole.
fn node_short_pair(stripped: &[String], header: usize) -> Option<(usize, String, String, String)> {
    if !stripped[header]
        .trim_end()
        .ends_with("Expected values to be strictly equal:")
    {
        return None;
    }
    let at = (header + 1..stripped.len()).find(|&i| !stripped[i].trim().is_empty())?;
    if at + 1 == stripped.len() {
        return None;
    }
    let line = stripped[at].trim();
    if line.matches(" !== ").count() != 1 || inspect_elided(line) {
        return None;
    }
    let (actual, expected) = line.split_once(" !== ")?;
    if actual.is_empty() || expected.is_empty() {
        return None;
    }
    Some((
        at,
        line.to_string(),
        actual.to_string(),
        expected.to_string(),
    ))
}

/// The short message alone, as vitest prints a Node assertion error.
fn parse_node_short_message(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let header = stripped.iter().position(|l| {
        let l = l.trim();
        l.starts_with("AssertionError") && l.ends_with("Expected values to be strictly equal:")
    })?;
    Some(
        node_short_pair(stripped, header).map(|(at, line, actual, expected)| {
            node_evidence(
                "strictEqual",
                Some(line),
                actual,
                expected,
                raw_excerpt(raw, header, at),
            )
        }),
    )
}

/// jest's reprint of a Node assertion error: "assert.strictEqual(received, expected)", then
/// "Expected value to strictly be equal to:" and "Received:" at the hint's column, each value
/// two columns further in (further lines of a multi-line string at the hint's column), and a
/// blank line after the received value. Output cut before that blank line is refused.
fn parse_jest_node_assert(raw: &[&str], stripped: &[String]) -> Option<Option<AssertionEvidence>> {
    let (hint, format, label) =
        stripped
            .iter()
            .enumerate()
            .find_map(|(i, line)| match line.trim() {
                "assert.strictEqual(received, expected)" => {
                    Some((i, "strictEqual", "Expected value to strictly be equal to:"))
                }
                "assert.deepStrictEqual(received, expected)" => Some((
                    i,
                    "deepStrictEqual",
                    "Expected value to deeply and strictly equal to:",
                )),
                _ => None,
            })?;
    let base = indent_of(&stripped[hint]);
    let at_base =
        |i: usize, text: &str| indent_of(&stripped[i]) == base && stripped[i].trim() == text;
    let values = || {
        let expected_label =
            (hint + 1..stripped.len()).find(|&i| !stripped[i].trim().is_empty())?;
        if !at_base(expected_label, label) {
            return None;
        }
        let received_label =
            (expected_label + 1..stripped.len()).find(|&i| at_base(i, "Received:"))?;
        let end = (received_label + 1..stripped.len()).find(|&i| stripped[i].trim().is_empty())?;
        let expected = jest_value(&stripped[expected_label + 1..received_label], base)?;
        let actual = jest_value(&stripped[received_label + 1..end], base)?;
        Some(node_evidence(
            format,
            None,
            actual,
            expected,
            raw_excerpt(raw, hint, end - 1),
        ))
    };
    Some(values())
}

/// One value as jest prints it: the first line two columns in from `base`, any further lines
/// (a multi-line string) at `base`. Refused unless its strings and brackets all close and
/// nothing was elided (`…` past jest's `maxWidth`, `[Object]` / `[Array]` past `maxDepth`).
fn jest_value(lines: &[String], base: usize) -> Option<String> {
    let (first, rest) = lines.split_first()?;
    let first = first.trim_end().strip_prefix(&" ".repeat(base + 2))?;
    if first.is_empty() || first.starts_with(' ') {
        return None;
    }
    let mut value = first.to_string();
    for line in rest {
        let line = line.trim_end();
        value.push('\n');
        if !line.is_empty() {
            value.push_str(line.strip_prefix(&" ".repeat(base))?);
        }
    }
    (!value.contains('…')
        && !value.contains("[Object]")
        && !value.contains("[Array]")
        && closes(&value, false))
    .then_some(value)
}

/// Whether every double-quoted string and every bracket in a printed value closes. With
/// `rust_chars`, Rust `Debug` char literals (`'{'`, `'"'`, `'\''`) are skipped as well; any other
/// apostrophe is text.
fn closes(value: &str, rust_chars: bool) -> bool {
    let mut open = Vec::new();
    let mut in_string = false;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if in_string {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '\'' if rust_chars => {
                let mut ahead = chars.clone();
                let literal = match ahead.next() {
                    Some('\\') => ahead.next().is_some() && ahead.any(|c| c == '\''),
                    Some(_) => ahead.next() == Some('\''),
                    None => false,
                };
                if literal {
                    chars = ahead;
                }
            }
            '"' => in_string = true,
            '(' | '[' | '{' => open.push(c),
            ')' | ']' | '}' => {
                let want = match c {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                if open.pop() != Some(want) {
                    return false;
                }
            }
            _ => {}
        }
    }
    !in_string && open.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_locations_inside_the_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("t.py"), "x\n").unwrap();
        let abs = std::fs::canonicalize(root).unwrap();
        let text = format!(
            "thread 'x' panicked at {}/src/lib.rs:7:9:\n  --> src/lib.rs:3:5\n  File \"{}/t.py\", line 2, in f\n at /usr/lib/x.py:1\n",
            abs.display(),
            abs.display()
        );
        let locs = locations_in(root, &text);
        assert_eq!(
            locs,
            vec![
                ("src/lib.rs".to_string(), 7),
                ("src/lib.rs".to_string(), 3),
                ("t.py".to_string(), 2)
            ]
        );
    }

    #[test]
    fn only_the_fixes_for_errors_are_suggested() {
        let fix = |level: &str, message: &str| crate::fixit::Fix {
            level: level.to_string(),
            code: None,
            message: message.to_string(),
            edits: vec![crate::fixit::Edit {
                file: "src/lib.rs".into(),
                start: 0,
                end: 1,
                line: 4,
                line_text: None,
                replacement: String::new(),
            }],
        };
        assert_eq!(
            suggested(&[
                fix("error", "mismatched types\nconsider borrowing"),
                fix("warning", "unused variable"),
                fix("error", "mismatched types\nconsider borrowing"),
            ]),
            vec!["src/lib.rs:4: mismatched types".to_string()]
        );
    }

    #[test]
    fn parses_rust_assert_eq_simple() {
        let output = "thread 'tests::it_fails' panicked at src/lib.rs:13:9:\nassertion `left == right` failed\n  left: 4\n right: 5\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n";
        let ev = parse_assertion_evidence(output).expect("parsed assertion");
        assert_eq!(ev.format, "assert_eq");
        assert_eq!(ev.expression.as_deref(), Some("left == right"));
        assert_eq!(ev.left.as_deref(), Some("4"));
        assert_eq!(ev.right.as_deref(), Some("5"));
        // `assert_eq!` accepts either order: no operand is claimed to be the expected one.
        assert_eq!(ev.actual, None);
        assert_eq!(ev.expected, None);
        assert_eq!(ev.operands, vec!["4", "5"]);
        assert_eq!(
            ev.excerpt,
            "assertion `left == right` failed\n  left: 4\n right: 5"
        );
        assert_eq!(
            ev.render_compact(),
            "assertion [assert_eq (left == right)]: left: 4, right: 5\n"
        );
    }

    #[test]
    fn parses_rust_assert_eq_multiline_debug() {
        let output = "thread 'tests::it_fails' panicked at src/lib.rs:13:9:\nassertion `left == right` failed\n  left: Foo {\n    a: 1,\n    b: 2,\n}\n right: Foo {\n    a: 1,\n    b: 3,\n}\nnote: run with `RUST_BACKTRACE=1`\n";
        let ev = parse_assertion_evidence(output).expect("parsed assertion");
        assert_eq!(ev.format, "assert_eq");
        assert_eq!(ev.left.as_deref(), Some("Foo {\n    a: 1,\n    b: 2,\n}"));
        assert_eq!(ev.right.as_deref(), Some("Foo {\n    a: 1,\n    b: 3,\n}"));
        assert_eq!(ev.operands.len(), 2);
    }

    #[test]
    fn parses_rust_assert_eq_colored() {
        let output = "\x1b[1m\x1b[31mthread 'tests::it_fails' panicked at \x1b[0msrc/lib.rs:13:9:\n\x1b[1m\x1b[31massertion `left == right` failed\x1b[0m\n\x1b[1m\x1b[31m  left: \x1b[0m\x1b[32m4\x1b[0m\n\x1b[1m\x1b[31m right: \x1b[0m\x1b[32m5\x1b[0m\nnote: run with RUST_BACKTRACE=1\n";
        let ev = parse_assertion_evidence(output).expect("parsed assertion");
        assert_eq!(ev.format, "assert_eq");
        assert_eq!(ev.left.as_deref(), Some("4"));
        assert_eq!(ev.right.as_deref(), Some("5"));
        assert_eq!(ev.actual, None);
        assert_eq!(ev.expected, None);
        // The excerpt keeps the escape sequences exactly as the runner printed them.
        assert!(
            ev.excerpt
                .starts_with("\x1b[1m\x1b[31massertion `left == right` failed\x1b[0m\n")
        );
        assert!(
            ev.excerpt
                .ends_with("\x1b[1m\x1b[31m right: \x1b[0m\x1b[32m5\x1b[0m")
        );
    }

    #[test]
    fn parses_rust_assert_ne() {
        let output = "thread 'main' panicked at src/lib.rs:14:9:\nassertion `left != right` failed\n  left: 4\n right: 4\n\n";
        let ev = parse_assertion_evidence(output).expect("parsed assertion");
        assert_eq!(ev.format, "assert_ne");
        assert_eq!(ev.expression.as_deref(), Some("left != right"));
        assert_eq!(ev.left.as_deref(), Some("4"));
        assert_eq!(ev.right.as_deref(), Some("4"));
        assert_eq!(ev.actual, None);
        assert_eq!(ev.expected, None);
        assert_eq!(ev.operands, vec!["4", "4"]);
    }

    #[test]
    fn parses_rust_assert_eq_with_custom_message() {
        let output = "thread 'test_msg' panicked at src/lib.rs:20:9:\nassertion `left == right` failed: expected matching user IDs\n  left: \"usr_1\"\n right: \"usr_2\"\n\n";
        let ev = parse_assertion_evidence(output).expect("parsed assertion");
        assert_eq!(ev.format, "assert_eq");
        assert_eq!(ev.left.as_deref(), Some("\"usr_1\""));
        assert_eq!(ev.right.as_deref(), Some("\"usr_2\""));
    }

    #[test]
    fn a_left_line_inside_a_custom_message_is_not_an_operand() {
        let output = "thread 't' panicked at src/lib.rs:3:5:\nassertion `left == right` failed: first line\n  left: from the message\n  left: 7\n right: 8\n\n";
        let ev = parse_assertion_evidence(output).expect("parsed assertion");
        assert_eq!(ev.left.as_deref(), Some("7"));
        assert_eq!(ev.right.as_deref(), Some("8"));
    }

    #[test]
    fn a_rust_block_without_both_operands_gives_nothing() {
        let output = "thread 't' panicked at src/lib.rs:3:5:\nassertion `left == right` failed\n  left: 7\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\n";
        assert_eq!(parse_assertion_evidence(output), None);
    }

    #[test]
    fn rust_operands_keep_their_blank_lines_up_to_the_panic_hooks_trailer() {
        let head = "\nthread 't' panicked at src/lib.rs:3:5:\nassertion `left == right` failed\n  left: A {\n\n    x\n}\n right: B {\n\n    y\n}\n";
        for trailer in [
            "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\n",
            "stack backtrace:\n   0: rust_begin_unwind\n\n",
            // The hook starts every panic message with a blank line of its own.
            "\nthread 'other' panicked at src/lib.rs:9:1:\nboom\n\n",
            // The end of a libtest block: its separator, then the next block or the list.
            "\n",
            "\n\n",
        ] {
            let ev = parse_assertion_evidence(&format!("{head}{trailer}"))
                .unwrap_or_else(|| panic!("no evidence before {trailer:?}"));
            assert_eq!(ev.left.as_deref(), Some("A {\n\n    x\n}"), "{trailer:?}");
            assert_eq!(ev.right.as_deref(), Some("B {\n\n    y\n}"), "{trailer:?}");
            assert!(
                ev.excerpt.ends_with(" right: B {\n\n    y\n}"),
                "{trailer:?}"
            );
        }
        // Before the hook's trailer a value's own trailing line break is printed as is.
        let own = head.replace("    y\n}\n", "    y\n}\n\n")
            + "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n";
        let ev = parse_assertion_evidence(&own).expect("evidence");
        assert_eq!(ev.right.as_deref(), Some("B {\n\n    y\n}\n"));
    }

    #[test]
    fn a_rust_block_cut_short_or_split_ambiguously_gives_nothing() {
        let block = "\nthread 't' panicked at src/lib.rs:3:5:\nassertion `left == right` failed\n  left: A {\n\n    x\n}\n right: B {\n\n    y\n}\n\n";
        // Cut at every line: without libtest's separator, or with a value's brackets still
        // open after one of its blank lines, nothing is claimed.
        let lines: Vec<&str> = block.split_inclusive('\n').collect();
        for cut in 1..lines.len() {
            let prefix = lines[..cut].concat();
            assert_eq!(parse_assertion_evidence(&prefix), None, "{prefix:?}");
        }
        // More blank lines than libtest's separators belong to the value, which then has an
        // unknown number of trailing line breaks.
        assert_eq!(parse_assertion_evidence(&format!("{block}\n\n")), None);
        // A second `right:` after the last `left:` could start either operand.
        let two_rights = block.replace("    x\n", "    x\n right: inside\n");
        assert_eq!(parse_assertion_evidence(&two_rights), None);
        // The Rust header claims the block: a Node message in it is not taken instead.
        let with_node = block.replace(
            "}\n\n",
            "\nAssertionError: Expected values to be strictly equal:\n\n1 !== 2\n\n",
        );
        assert_eq!(parse_assertion_evidence(&with_node), None);
    }

    #[test]
    fn rust_char_operands_do_not_count_as_brackets() {
        let output = "assertion `left == right` failed\n  left: '{'\n right: ['\"', '\\'', '\\u{7b}', ')']\n\n";
        let ev = parse_assertion_evidence(output).expect("evidence");
        assert_eq!(ev.left.as_deref(), Some("'{'"));
        assert_eq!(ev.right.as_deref(), Some("['\"', '\\'', '\\u{7b}', ')']"));
    }

    #[test]
    fn parses_node_strict_equal() {
        let output = "node:assert:95\n  throw new AssertionError(obj);\n  ^\n\nAssertionError [ERR_ASSERTION]: Expected values to be strictly equal:\n\n1 !== 2\n\n    at [eval]:1:42 {\n  generatedMessage: true,\n  code: 'ERR_ASSERTION',\n  actual: 1,\n  expected: 2,\n  operator: 'strictEqual'\n}\n\nNode.js v24.3.0\n";
        let ev = parse_assertion_evidence(output).expect("parsed node assertion");
        assert_eq!(ev.format, "strictEqual");
        assert_eq!(ev.expression.as_deref(), Some("1 !== 2"));
        assert_eq!(ev.actual.as_deref(), Some("1"));
        assert_eq!(ev.expected.as_deref(), Some("2"));
        assert_eq!(ev.left.as_deref(), Some("1"));
        assert_eq!(ev.right.as_deref(), Some("2"));
        assert_eq!(ev.operands, vec!["1", "2"]);
        assert!(ev.excerpt.contains("AssertionError"));
        assert!(ev.excerpt.contains("operator: 'strictEqual'"));
    }

    #[test]
    fn parses_node_deep_strict_equal_multiline() {
        let output = "AssertionError [ERR_ASSERTION]: Expected values to be strictly deep-equal:\n+ actual - expected\n\n  {\n+   a: 1\n-   a: 2\n  }\n\n    at [eval]:1:42 {\n  generatedMessage: true,\n  code: 'ERR_ASSERTION',\n  actual: {\n    a: 'foo',\n    b: [ 1, 2 ]\n  },\n  expected: {\n    a: 'bar',\n    b: [ 1, 3 ]\n  },\n  operator: 'deepStrictEqual'\n}\n";
        let ev = parse_assertion_evidence(output).expect("parsed node deep assertion");
        assert_eq!(ev.format, "deepStrictEqual");
        assert_eq!(ev.expression, None);
        assert_eq!(
            ev.actual.as_deref(),
            Some("{\n    a: 'foo',\n    b: [ 1, 2 ]\n  }")
        );
        assert_eq!(
            ev.expected.as_deref(),
            Some("{\n    a: 'bar',\n    b: [ 1, 3 ]\n  }")
        );
        assert_eq!(ev.operands.len(), 2);
    }

    #[test]
    fn parses_node_with_ansi_colors() {
        let output = "\x1b[31mAssertionError [ERR_ASSERTION]: Expected values to be strictly equal:\x1b[0m\n\n1 !== 2\n\n    at test.js:1:1 {\n  actual: \x1b[32m1\x1b[0m,\n  expected: \x1b[31m2\x1b[0m,\n  operator: 'strictEqual'\n}\n";
        let ev = parse_assertion_evidence(output).expect("parsed colored node assertion");
        assert_eq!(ev.format, "strictEqual");
        assert_eq!(ev.actual.as_deref(), Some("1"));
        assert_eq!(ev.expected.as_deref(), Some("2"));
    }

    const NESTED_FIELDS: &str = "AssertionError [ERR_ASSERTION]: Expected values to be strictly deep-equal:\n+ actual - expected\n\n  {\n+   expected: 'inner-a',\n-   expected: 'inner-b',\n    operator: 'strictEqual'\n  }\n\n    at run (file.js:1:1) {\n  generatedMessage: true,\n  code: 'ERR_ASSERTION',\n  actual: {\n    expected: 'inner-a',\n    operator: 'strictEqual'\n  },\n  expected: {\n    expected: 'inner-b',\n    operator: 'strictEqual'\n  },\n  operator: 'deepStrictEqual'\n}\n";

    #[test]
    fn node_fields_nested_in_a_value_or_the_diff_are_not_the_errors_own() {
        let ev = parse_assertion_evidence(NESTED_FIELDS).expect("parsed node fields");
        assert_eq!(ev.format, "deepStrictEqual");
        assert_eq!(ev.expression, None);
        assert_eq!(
            ev.actual.as_deref(),
            Some("{\n    expected: 'inner-a',\n    operator: 'strictEqual'\n  }")
        );
        assert_eq!(
            ev.expected.as_deref(),
            Some("{\n    expected: 'inner-b',\n    operator: 'strictEqual'\n  }")
        );
        assert!(ev.excerpt.ends_with("operator: 'deepStrictEqual'\n}"));
    }

    #[test]
    fn incomplete_or_elided_node_fields_give_nothing() {
        // Cut before the closing brace.
        let cut = NESTED_FIELDS.trim_end().strip_suffix("\n}").unwrap();
        assert_eq!(parse_assertion_evidence(cut), None);
        // Cut inside the expected value.
        let cut = &NESTED_FIELDS[..NESTED_FIELDS.find("    expected: 'inner-b'").unwrap()];
        assert_eq!(parse_assertion_evidence(cut), None);
        // A top-level field missing.
        let no_operator = NESTED_FIELDS.replace("  },\n  operator: 'deepStrictEqual'\n", "  }\n");
        assert_eq!(parse_assertion_evidence(&no_operator), None);
        // util.inspect elided a nested object or the tail of an array.
        let depth =
            NESTED_FIELDS.replace("operator: 'strictEqual'\n  },", "deeper: [Object]\n  },");
        assert_eq!(parse_assertion_evidence(&depth), None);
        let items =
            NESTED_FIELDS.replace("expected: 'inner-a',", "list: [ 1, ... 99 more items ],");
        assert_eq!(parse_assertion_evidence(&items), None);
    }

    #[test]
    fn a_node_short_message_splits_only_on_a_single_operator() {
        let output = "AssertionError: Expected values to be strictly equal:\n\n'a' !== 'b'\n\n";
        let ev = parse_assertion_evidence(output).expect("parsed short message");
        assert_eq!(ev.format, "strictEqual");
        assert_eq!(ev.actual.as_deref(), Some("'a'"));
        assert_eq!(ev.expected.as_deref(), Some("'b'"));
        assert_eq!(ev.expression.as_deref(), Some("'a' !== 'b'"));
        let ambiguous =
            "AssertionError: Expected values to be strictly equal:\n\n'x !== y' !== 'z'\n\n";
        assert_eq!(parse_assertion_evidence(ambiguous), None);
        // Output that stops at the pair may have cut its second value.
        let cut = "AssertionError: Expected values to be strictly equal:\n\n'a' !== 'b";
        assert_eq!(parse_assertion_evidence(cut), None);
    }

    #[test]
    fn a_cut_error_fields_block_is_not_read_from_its_short_message() {
        let fields = "AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:\n\n1 !== 2\n\n    at [eval]:1:42 {\n  generatedMessage: true,\n  code: 'ERR_ASSERTION',\n  actual: 1,\n  expected: 2,\n  operator: 'strictEqual'\n}\n";
        assert!(parse_assertion_evidence(fields).is_some());
        let cut = fields.strip_suffix("}\n").unwrap();
        assert_eq!(parse_assertion_evidence(cut), None);
        let cut = &fields[..fields.find("  expected: 2").unwrap()];
        assert_eq!(parse_assertion_evidence(cut), None);
    }

    const JEST_STRICT: &str = "    assert.strictEqual(received, expected)\n\n    Expected value to strictly be equal to:\n      \"line one\n    line 2\"\n    Received:\n      \"line one\n    line two\"\n\n    Difference:\n";

    #[test]
    fn jest_values_are_bound_by_their_labels_and_columns() {
        let ev = parse_assertion_evidence(JEST_STRICT).expect("parsed jest reprint");
        assert_eq!(ev.format, "strictEqual");
        assert_eq!(ev.expected.as_deref(), Some("\"line one\nline 2\""));
        assert_eq!(ev.actual.as_deref(), Some("\"line one\nline two\""));
        assert_eq!(ev.left, ev.actual);
        assert_eq!(ev.right, ev.expected);
        assert!(
            ev.excerpt
                .starts_with("    assert.strictEqual(received, expected)")
        );
        assert!(ev.excerpt.ends_with("    line two\""));
    }

    #[test]
    fn truncated_or_elided_jest_values_give_nothing() {
        // Output cut inside the received value: no blank line ends it.
        let cut = &JEST_STRICT[..JEST_STRICT.find("\n\n    Difference").unwrap()];
        assert_eq!(parse_assertion_evidence(cut), None);
        // A string that is still open when the next label comes.
        let open = JEST_STRICT.replace("    line 2\"\n", "    Received:\n    line 2\"\n");
        assert_eq!(parse_assertion_evidence(&open), None);
        // jest's maxWidth and maxDepth elisions.
        let wide = "    assert.deepStrictEqual(received, expected)\n\n    Expected value to deeply and strictly equal to:\n      [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, …]\n    Received:\n      [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, …]\n\n";
        assert_eq!(parse_assertion_evidence(wide), None);
        let deep = wide.replace("[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, …]", "{\"a\": [Object]}");
        assert_eq!(parse_assertion_evidence(&deep), None);
        // A label that does not match the hint's operator.
        let mismatched =
            JEST_STRICT.replace("strictly be equal to:", "deeply and strictly equal to:");
        assert_eq!(parse_assertion_evidence(&mismatched), None);
    }

    #[test]
    fn pytest_and_testify_output_is_not_parsed() {
        let pytest = "def test_f():\n>       assert result == expected\nE       assert result == expected\n\ntest_f.py:2: AssertionError\n";
        assert_eq!(parse_assertion_evidence(pytest), None);
        let testify = "    foo_test.go:12:\n        \tError:      \tNot equal:\n        \t            \texpected: 1\n        \t            \tactual  : 2\n        \tTest:       \tTestFoo\n";
        assert_eq!(parse_assertion_evidence(testify), None);
    }

    #[test]
    fn rejects_unrelated_logs_and_explicit_panics() {
        assert_eq!(
            parse_assertion_evidence("thread 'x' panicked at 'explicit panic'"),
            None
        );
        assert_eq!(
            parse_assertion_evidence("assertion failed: flag_is_true"),
            None
        );
        assert_eq!(
            parse_assertion_evidence("[INFO] checking assert condition: ok"),
            None
        );
        assert_eq!(
            parse_assertion_evidence("assertion `left == right` failed"),
            None
        );
        assert_eq!(
            parse_assertion_evidence("TypeError: undefined is not a function"),
            None
        );
    }

    #[test]
    fn does_not_collect_across_test_boundaries() {
        let output = "---- tests::test_one stdout ----\nthread 'tests::test_one' panicked at src/lib.rs:10:5:\nassertion `left == right` failed\n  left: 1\n right: 2\n\n---- tests::test_two stdout ----\nthread 'tests::test_two' panicked at src/lib.rs:20:5:\nassertion `left == right` failed\n  left: 10\n right: 20\n";
        let ev = parse_assertion_evidence(output).expect("parsed assertion");
        assert_eq!(ev.left.as_deref(), Some("1"));
        assert_eq!(ev.right.as_deref(), Some("2"));
        assert!(!ev.excerpt.contains("test_two"));
    }

    #[test]
    fn render_compact_produces_clean_evidence() {
        let ev = AssertionEvidence {
            format: "assert_eq".to_string(),
            expression: Some("left == right".to_string()),
            actual: Some("4".to_string()),
            expected: Some("5".to_string()),
            left: Some("4".to_string()),
            right: Some("5".to_string()),
            operands: vec!["4".to_string(), "5".to_string()],
            excerpt: "assertion `left == right` failed\n  left: 4\n right: 5".to_string(),
        };
        let compact = ev.render_compact();
        assert_eq!(
            compact,
            "assertion [assert_eq (left == right)]: actual: 4, expected: 5\n"
        );
    }
}
