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
    /// Exact printed actual value as a string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// Exact printed expected value as a string.
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

        let mut out = String::new();
        if let (Some(left), Some(right)) = (&self.left, &self.right) {
            if self.actual.is_some() && self.expected.is_some() {
                let act = self.actual.as_deref().unwrap_or(left);
                let exp = self.expected.as_deref().unwrap_or(right);
                if let Some(expr) = &self.expression {
                    out.push_str(&format!(
                        "assertion [{} ({expr})]: actual: {}, expected: {}\n",
                        self.format,
                        short_val(act),
                        short_val(exp)
                    ));
                } else {
                    out.push_str(&format!(
                        "assertion [{}]: actual: {}, expected: {}\n",
                        self.format,
                        short_val(act),
                        short_val(exp)
                    ));
                }
            } else if let Some(expr) = &self.expression {
                out.push_str(&format!(
                    "assertion [{} ({expr})]: left: {}, right: {}\n",
                    self.format,
                    short_val(left),
                    short_val(right)
                ));
            } else {
                out.push_str(&format!(
                    "assertion [{}]: left: {}, right: {}\n",
                    self.format,
                    short_val(left),
                    short_val(right)
                ));
            }
        } else if let Some(expr) = &self.expression {
            out.push_str(&format!("assertion [{}]: {expr}\n", self.format));
        } else {
            out.push_str(&format!("assertion [{}]\n", self.format));
        }
        out
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
    filter: Option<&str>,
    timeout_secs: u64,
) -> Result<DossierReport> {
    let report: VerifyReport = run_verify(
        remote,
        root,
        Some(root),
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
            Some(root),
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
            let assertion = parse_assertion_evidence(&failure.output)
                .or_else(|| parse_assertion_evidence(&report.tail));
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

/// Parses structured runtime assertion evidence from test failure output.
/// Supports Rust `assert_eq!` / `assert_ne!`, Node `strictEqual` / `deepStrictEqual`,
/// pytest literal comparisons, and Go testify comparisons.
/// Never evaluates expressions or guesses omitted values.
pub fn parse_assertion_evidence(output: &str) -> Option<AssertionEvidence> {
    if output.trim().is_empty() {
        return None;
    }
    let raw_lines: Vec<&str> = output.lines().collect();
    let stripped_lines: Vec<String> = raw_lines.iter().map(|l| strip_ansi(l)).collect();

    if let Some(ev) = parse_rust_assertion(&raw_lines, &stripped_lines) {
        return Some(ev);
    }
    if let Some(ev) = parse_node_assertion(&raw_lines, &stripped_lines) {
        return Some(ev);
    }
    if let Some(ev) = parse_testify_assertion(&raw_lines, &stripped_lines) {
        return Some(ev);
    }
    if let Some(ev) = parse_pytest_assertion(&raw_lines, &stripped_lines) {
        return Some(ev);
    }

    None
}

fn parse_rust_assertion(
    raw_lines: &[&str],
    stripped_lines: &[String],
) -> Option<AssertionEvidence> {
    let mut header_idx = None;
    let mut is_ne = false;
    let mut expression = None;

    for (i, line) in stripped_lines.iter().enumerate() {
        if line.starts_with("---- ") && line.ends_with(" stdout ----") {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("assertion `") || trimmed.starts_with("assertion failed:") {
            if trimmed.contains("left != right") || trimmed.contains("(left != right)") {
                is_ne = true;
                header_idx = Some(i);
                expression = Some("left != right".to_string());
                break;
            } else if trimmed.contains("left == right") || trimmed.contains("(left == right)") {
                is_ne = false;
                header_idx = Some(i);
                expression = Some("left == right".to_string());
                break;
            } else if let Some(between) = extract_between_backticks(trimmed) {
                if between.contains("!=") {
                    is_ne = true;
                    header_idx = Some(i);
                    expression = Some(between.to_string());
                    break;
                } else if between.contains("==") {
                    is_ne = false;
                    header_idx = Some(i);
                    expression = Some(between.to_string());
                    break;
                }
            }
        }
    }

    let header_idx = header_idx?;

    // Look for `left:`
    let mut left_line_idx = None;
    for i in (header_idx + 1)..stripped_lines.len() {
        let trimmed = stripped_lines[i].trim();
        if is_test_boundary(trimmed) {
            return None;
        }
        if stripped_lines[i].trim_start().starts_with("left:") {
            left_line_idx = Some(i);
            break;
        }
    }
    let left_line_idx = left_line_idx?;

    let mut left_lines = Vec::new();
    let first_left = stripped_lines[left_line_idx]
        .trim_start()
        .strip_prefix("left:")
        .unwrap_or("")
        .trim_start();
    left_lines.push(first_left.to_string());

    let mut right_line_idx = None;
    for i in (left_line_idx + 1)..stripped_lines.len() {
        let trimmed = stripped_lines[i].trim();
        if is_test_boundary(trimmed) {
            return None;
        }
        if stripped_lines[i].trim_start().starts_with("right:") {
            right_line_idx = Some(i);
            break;
        }
        left_lines.push(stripped_lines[i].clone());
    }
    let right_line_idx = right_line_idx?;

    let mut right_lines = Vec::new();
    let first_right = stripped_lines[right_line_idx]
        .trim_start()
        .strip_prefix("right:")
        .unwrap_or("")
        .trim_start();
    right_lines.push(first_right.to_string());

    let mut end_idx = stripped_lines.len();
    for i in (right_line_idx + 1)..stripped_lines.len() {
        let trimmed = stripped_lines[i].trim();
        if is_rust_end_boundary(trimmed) {
            end_idx = i;
            break;
        }
        if trimmed.is_empty()
            && i + 1 < stripped_lines.len()
            && is_rust_end_boundary(stripped_lines[i + 1].trim())
        {
            end_idx = i;
            break;
        }
        right_lines.push(stripped_lines[i].clone());
    }

    let left_str = clean_operand_lines(left_lines);
    let right_str = clean_operand_lines(right_lines);
    if left_str.is_empty() || right_str.is_empty() {
        return None;
    }

    let excerpt = raw_lines[header_idx..end_idx]
        .iter()
        .map(|s| s.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    let (actual, expected) = if is_ne {
        (None, None)
    } else {
        (Some(left_str.clone()), Some(right_str.clone()))
    };

    Some(AssertionEvidence {
        format: if is_ne {
            "assert_ne".to_string()
        } else {
            "assert_eq".to_string()
        },
        expression,
        actual,
        expected,
        left: Some(left_str.clone()),
        right: Some(right_str.clone()),
        operands: vec![left_str, right_str],
        excerpt,
    })
}

fn is_rust_end_boundary(trimmed: &str) -> bool {
    trimmed.starts_with("note:")
        || trimmed.starts_with("stack backtrace:")
        || trimmed.starts_with("---- ")
        || trimmed == "failures:"
        || trimmed.starts_with("failures:")
        || trimmed.starts_with("test result:")
        || trimmed.starts_with("thread '")
}

fn is_test_boundary(trimmed: &str) -> bool {
    trimmed.starts_with("---- ")
        || trimmed == "failures:"
        || trimmed.starts_with("failures:")
        || trimmed.starts_with("test result:")
}

fn clean_operand_lines(mut lines: Vec<String>) -> String {
    while let Some(last) = lines.last() {
        if last.trim().is_empty() {
            lines.pop();
        } else {
            break;
        }
    }
    let mut joined = lines.join("\n");
    let trimmed = joined.trim();
    if trimmed.starts_with('`') && (trimmed.ends_with('`') || trimmed.ends_with("`,")) {
        let unquoted = trimmed
            .trim_start_matches('`')
            .trim_end_matches(',')
            .trim_end_matches('`');
        return unquoted.to_string();
    }
    if !joined.contains('\n') && joined.ends_with(',') {
        joined.pop();
    }
    joined
}

fn extract_between_backticks(s: &str) -> Option<&str> {
    let start = s.find('`')? + 1;
    let end = s[start..].find('`')? + start;
    Some(&s[start..end])
}

fn parse_node_assertion(
    raw_lines: &[&str],
    stripped_lines: &[String],
) -> Option<AssertionEvidence> {
    let mut header_idx = None;
    for (i, line) in stripped_lines.iter().enumerate() {
        if line.contains("AssertionError") || line.contains("ERR_ASSERTION") {
            header_idx = Some(i);
            break;
        }
    }
    let header_idx = header_idx?;

    let mut op_idx = None;
    let mut op_name = None;
    for i in header_idx..stripped_lines.len() {
        let trimmed = stripped_lines[i].trim();
        if is_test_boundary(trimmed) {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("operator:") {
            let op = rest
                .trim()
                .trim_matches(|c| c == '\'' || c == '"' || c == ',');
            if op == "strictEqual" || op == "deepStrictEqual" {
                op_idx = Some(i);
                op_name = Some(op.to_string());
                break;
            }
        }
    }

    if let (Some(op_idx), Some(format)) = (op_idx, op_name) {
        let mut actual_idx = None;
        let mut expected_idx = None;
        for i in header_idx..op_idx {
            let trimmed = stripped_lines[i].trim();
            if trimmed.starts_with("actual:") && actual_idx.is_none() {
                actual_idx = Some(i);
            } else if trimmed.starts_with("expected:") && expected_idx.is_none() {
                expected_idx = Some(i);
            }
        }

        if let (Some(actual_idx), Some(expected_idx)) = (actual_idx, expected_idx) {
            if actual_idx < expected_idx && expected_idx < op_idx {
                let mut actual_lines = Vec::new();
                let first_act = stripped_lines[actual_idx]
                    .trim_start()
                    .strip_prefix("actual:")
                    .unwrap_or("")
                    .trim_start();
                actual_lines.push(first_act.to_string());
                for i in (actual_idx + 1)..expected_idx {
                    actual_lines.push(stripped_lines[i].clone());
                }

                let mut expected_lines = Vec::new();
                let first_exp = stripped_lines[expected_idx]
                    .trim_start()
                    .strip_prefix("expected:")
                    .unwrap_or("")
                    .trim_start();
                expected_lines.push(first_exp.to_string());
                for i in (expected_idx + 1)..op_idx {
                    expected_lines.push(stripped_lines[i].clone());
                }

                let actual_str = clean_node_lines(actual_lines);
                let expected_str = clean_node_lines(expected_lines);

                let mut expression = None;
                for i in (header_idx + 1)..actual_idx {
                    let trimmed = stripped_lines[i].trim();
                    if trimmed.starts_with("at ") {
                        break;
                    }
                    if trimmed.contains("!==")
                        || trimmed.contains("===")
                        || trimmed.contains("!=")
                        || trimmed.contains("==")
                    {
                        expression = Some(trimmed.to_string());
                        break;
                    }
                }

                let mut close_idx = op_idx;
                for i in (op_idx + 1)..stripped_lines.len().min(op_idx + 5) {
                    if stripped_lines[i].trim() == "}" || stripped_lines[i].trim().ends_with('}') {
                        close_idx = i;
                        break;
                    }
                }

                let excerpt = raw_lines[header_idx..=close_idx]
                    .iter()
                    .map(|s| s.trim_end())
                    .collect::<Vec<_>>()
                    .join("\n");

                return Some(AssertionEvidence {
                    format,
                    expression,
                    actual: Some(actual_str.clone()),
                    expected: Some(expected_str.clone()),
                    left: Some(actual_str.clone()),
                    right: Some(expected_str.clone()),
                    operands: vec![actual_str, expected_str],
                    excerpt,
                });
            }
        }
    }

    for i in header_idx..stripped_lines.len() {
        let trimmed = stripped_lines[i].trim();
        if trimmed.contains("Expected values to be strictly equal:") {
            for j in (i + 1)..stripped_lines.len().min(i + 5) {
                let expr_line = stripped_lines[j].trim();
                if expr_line.contains("!==") {
                    if let Some((left, right)) = expr_line.split_once("!==") {
                        let left = left.trim().to_string();
                        let right = right.trim().to_string();
                        let excerpt = raw_lines[header_idx..=j]
                            .iter()
                            .map(|s| s.trim_end())
                            .collect::<Vec<_>>()
                            .join("\n");
                        return Some(AssertionEvidence {
                            format: "strictEqual".to_string(),
                            expression: Some(expr_line.to_string()),
                            actual: Some(left.clone()),
                            expected: Some(right.clone()),
                            left: Some(left.clone()),
                            right: Some(right.clone()),
                            operands: vec![left, right],
                            excerpt,
                        });
                    }
                }
            }
        }
    }

    None
}

fn clean_node_lines(mut lines: Vec<String>) -> String {
    while let Some(last) = lines.last() {
        if last.trim().is_empty() {
            lines.pop();
        } else {
            break;
        }
    }
    let mut joined = lines.join("\n");
    let trimmed = joined.trim();
    if trimmed.ends_with(',') {
        joined = trimmed[..trimmed.len() - 1].trim().to_string();
    }
    joined
}

fn parse_testify_assertion(
    raw_lines: &[&str],
    stripped_lines: &[String],
) -> Option<AssertionEvidence> {
    for (i, line) in stripped_lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.contains("Not equal:") || trimmed.contains("Should not be equal:") {
            let is_ne = trimmed.contains("Should not be equal:");
            let mut expected = None;
            let mut actual = None;
            let mut end_idx = i;
            for j in (i + 1)..stripped_lines.len().min(i + 10) {
                let l = stripped_lines[j].trim();
                if l.starts_with("Test:") || is_test_boundary(l) {
                    break;
                }
                if let Some(exp) = l.strip_prefix("expected:") {
                    expected = Some(exp.trim().to_string());
                    end_idx = j;
                } else if let Some(act) = l.strip_prefix("actual  :") {
                    actual = Some(act.trim().to_string());
                    end_idx = j;
                } else if let Some(act) = l.strip_prefix("actual:") {
                    actual = Some(act.trim().to_string());
                    end_idx = j;
                }
            }
            if let (Some(expected), Some(actual)) = (expected, actual) {
                let excerpt = raw_lines[i..=end_idx]
                    .iter()
                    .map(|s| s.trim_end())
                    .collect::<Vec<_>>()
                    .join("\n");
                let (act_field, exp_field) = if is_ne {
                    (None, None)
                } else {
                    (Some(actual.clone()), Some(expected.clone()))
                };
                return Some(AssertionEvidence {
                    format: if is_ne {
                        "assert_ne".to_string()
                    } else {
                        "assert_eq".to_string()
                    },
                    expression: None,
                    actual: act_field,
                    expected: exp_field,
                    left: Some(actual.clone()),
                    right: Some(expected.clone()),
                    operands: vec![actual, expected],
                    excerpt,
                });
            }
        }
    }
    None
}

fn parse_pytest_assertion(
    raw_lines: &[&str],
    stripped_lines: &[String],
) -> Option<AssertionEvidence> {
    for (i, line) in stripped_lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("E   assert ")
            .or_else(|| trimmed.strip_prefix("E       assert "))
            .or_else(|| trimmed.strip_prefix("assert "))
        {
            if let Some((left, right)) = rest.split_once(" == ") {
                let left = left.trim();
                let right = right.trim();
                if !left.is_empty() && !right.is_empty() {
                    let excerpt = raw_lines[i].trim().to_string();
                    let expr = format!("{left} == {right}");
                    return Some(AssertionEvidence {
                        format: "assert_eq".to_string(),
                        expression: Some(expr),
                        actual: Some(left.to_string()),
                        expected: Some(right.to_string()),
                        left: Some(left.to_string()),
                        right: Some(right.to_string()),
                        operands: vec![left.to_string(), right.to_string()],
                        excerpt,
                    });
                }
            } else if let Some((left, right)) = rest.split_once(" != ") {
                let left = left.trim();
                let right = right.trim();
                if !left.is_empty() && !right.is_empty() {
                    let excerpt = raw_lines[i].trim().to_string();
                    let expr = format!("{left} != {right}");
                    return Some(AssertionEvidence {
                        format: "assert_ne".to_string(),
                        expression: Some(expr),
                        actual: None,
                        expected: None,
                        left: Some(left.to_string()),
                        right: Some(right.to_string()),
                        operands: vec![left.to_string(), right.to_string()],
                        excerpt,
                    });
                }
            }
        }
    }
    None
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
        assert_eq!(ev.actual.as_deref(), Some("4"));
        assert_eq!(ev.expected.as_deref(), Some("5"));
        assert_eq!(ev.operands, vec!["4", "5"]);
        assert!(ev.excerpt.contains("left: 4"));
        assert!(ev.excerpt.contains("right: 5"));
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
        assert_eq!(ev.actual.as_deref(), Some("4"));
        assert_eq!(ev.expected.as_deref(), Some("5"));
    }

    #[test]
    fn parses_rust_assert_ne() {
        let output = "thread 'main' panicked at src/lib.rs:14:9:\nassertion `left != right` failed\n  left: 4\n right: 4\n";
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
        let output = "thread 'test_msg' panicked at src/lib.rs:20:9:\nassertion `left == right` failed: expected matching user IDs\n  left: \"usr_1\"\n right: \"usr_2\"\n";
        let ev = parse_assertion_evidence(output).expect("parsed assertion");
        assert_eq!(ev.format, "assert_eq");
        assert_eq!(ev.left.as_deref(), Some("\"usr_1\""));
        assert_eq!(ev.right.as_deref(), Some("\"usr_2\""));
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

    #[test]
    fn parses_pytest_literal_comparison() {
        let output = "def test_f():\n>       assert 1 == 2\nE       assert 1 == 2\n\ntest_f.py:2: AssertionError\n";
        let ev = parse_assertion_evidence(output).expect("parsed pytest assertion");
        assert_eq!(ev.format, "assert_eq");
        assert_eq!(ev.expression.as_deref(), Some("1 == 2"));
        assert_eq!(ev.actual.as_deref(), Some("1"));
        assert_eq!(ev.expected.as_deref(), Some("2"));
    }

    #[test]
    fn parses_go_testify_comparison() {
        let output = "    foo_test.go:12:\n        \tError:      \tNot equal:\n        \t            \texpected: 1\n        \t            \tactual  : 2\n        \tTest:       \tTestFoo\n";
        let ev = parse_assertion_evidence(output).expect("parsed testify assertion");
        assert_eq!(ev.format, "assert_eq");
        assert_eq!(ev.expected.as_deref(), Some("1"));
        assert_eq!(ev.actual.as_deref(), Some("2"));
        assert_eq!(ev.operands, vec!["2", "1"]);
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
