//! Typed verification commands (Phase 6.4): `check`, `lint` and `test` run through remote exec
//! with machine-readable output where the toolchain offers it, parsed into structured
//! diagnostics and test failures for terminals and agents.

use crate::exec::{TailBuffer, run_remote};
use crate::sync::expected_engine;
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyKind {
    Check,
    Lint,
    Test,
}

impl VerifyKind {
    pub fn label(&self) -> &'static str {
        match self {
            VerifyKind::Check => "check",
            VerifyKind::Lint => "lint",
            VerifyKind::Test => "test",
        }
    }
}

/// One compiler or linter finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub level: String,
    pub code: Option<String>,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u64>,
    pub column: Option<u64>,
}

impl Diagnostic {
    pub fn render(&self) -> String {
        let at = match (&self.file, self.line, self.column) {
            (Some(f), Some(l), Some(c)) => format!("{f}:{l}:{c}"),
            (Some(f), Some(l), None) => format!("{f}:{l}"),
            (Some(f), _, _) => f.clone(),
            _ => String::new(),
        };
        let code = self
            .code
            .as_ref()
            .map(|c| format!("[{c}] "))
            .unwrap_or_default();
        if at.is_empty() {
            format!("{}: {code}{}", self.level, self.message)
        } else {
            format!("{}: {code}{} ({at})", self.level, self.message)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestFailure {
    pub name: String,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    pub kind: VerifyKind,
    pub language: String,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub diagnostics: Vec<Diagnostic>,
    pub tests_passed: u64,
    pub tests_failed: u64,
    pub failures: Vec<TestFailure>,
    /// Tail of the raw combined output, for anything the parsers did not understand.
    pub tail: String,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }

    pub fn errors(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.level == "error")
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.level == "warning")
            .count()
    }

    pub fn summary(&self) -> String {
        let status = match (self.timed_out, self.exit_code) {
            (true, _) => "TIMED OUT".to_string(),
            (false, Some(0)) => "OK".to_string(),
            (false, Some(code)) => format!("FAILED (exit {code})"),
            (false, None) => "KILLED".to_string(),
        };
        let mut parts = vec![format!(
            "{} {}: {status} in {:.1}s",
            self.language,
            self.kind.label(),
            self.duration_ms as f64 / 1000.0
        )];
        if self.kind == VerifyKind::Test {
            parts.push(format!(
                "{} passed, {} failed",
                self.tests_passed, self.tests_failed
            ));
        }
        if !self.diagnostics.is_empty() {
            parts.push(format!(
                "{} error(s), {} warning(s)",
                self.errors(),
                self.warnings()
            ));
        }
        parts.join("; ")
    }

    /// Human/agent readable report: summary, diagnostics, test failures, then the raw tail
    /// only when nothing structured explains a failure.
    pub fn render(&self, max_items: usize) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "$ {}\n{}\n",
            self.command.join(" "),
            self.summary()
        ));
        for d in self.diagnostics.iter().take(max_items) {
            out.push_str("  ");
            out.push_str(&d.render());
            out.push('\n');
        }
        if self.diagnostics.len() > max_items {
            out.push_str(&format!(
                "  ... {} more diagnostic(s)\n",
                self.diagnostics.len() - max_items
            ));
        }
        for f in self.failures.iter().take(max_items) {
            out.push_str(&format!(
                "--- FAILED {} ---\n{}\n",
                f.name,
                f.output.trim_end()
            ));
        }
        if self.failures.len() > max_items {
            out.push_str(&format!(
                "... {} more failed test(s)\n",
                self.failures.len() - max_items
            ));
        }
        if !self.ok() && self.diagnostics.is_empty() && self.failures.is_empty() {
            out.push_str("--- output tail ---\n");
            out.push_str(self.tail.trim_end());
            out.push('\n');
        }
        out
    }
}

/// The command a verification kind maps to for `language`.
pub fn plan_command(language: &str, kind: VerifyKind, filter: Option<&str>) -> Result<Vec<String>> {
    let mut cmd: Vec<String> = match (language, kind) {
        ("rust", VerifyKind::Check) => vec![
            "cargo",
            "check",
            "--workspace",
            "--all-targets",
            "--message-format=json",
        ],
        ("rust", VerifyKind::Lint) => vec![
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--message-format=json",
            "--",
            "-D",
            "warnings",
        ],
        ("rust", VerifyKind::Test) => vec!["cargo", "test", "--workspace"],
        ("go", VerifyKind::Check) => vec!["go", "build", "./..."],
        ("go", VerifyKind::Lint) => vec!["go", "vet", "./..."],
        ("go", VerifyKind::Test) => vec!["go", "test", "-json", "./..."],
        ("typescript", VerifyKind::Check) => vec!["npx", "tsc", "--noEmit", "--pretty", "false"],
        ("typescript", VerifyKind::Lint) => vec!["npx", "eslint", ".", "-f", "unix"],
        ("typescript", VerifyKind::Test) => vec!["npm", "test", "--silent", "--"],
        ("python", VerifyKind::Check) => vec!["basedpyright", "--outputjson"],
        ("python", VerifyKind::Lint) => vec!["ruff", "check", ".", "--output-format", "concise"],
        ("python", VerifyKind::Test) => vec!["python3", "-m", "pytest", "-q", "-rf"],
        ("cpp", VerifyKind::Check) => vec![
            "sh",
            "-c",
            "cmake -S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON >/dev/null && cmake --build build",
        ],
        ("cpp", VerifyKind::Test) => vec!["ctest", "--test-dir", "build", "--output-on-failure"],
        ("swift", VerifyKind::Check) => vec!["swift", "build"],
        ("swift", VerifyKind::Test) => vec!["swift", "test"],
        _ => {
            return Err(anyhow!(
                "no {} command for language {language}",
                kind.label()
            ));
        }
    }
    .into_iter()
    .map(str::to_string)
    .collect();
    if let Some(filter) = filter.filter(|f| !f.is_empty()) {
        match (language, kind) {
            ("rust", VerifyKind::Test) => cmd.push(filter.to_string()),
            ("go", VerifyKind::Test) => {
                cmd.push("-run".to_string());
                cmd.push(filter.to_string());
            }
            ("python", VerifyKind::Test) => {
                cmd.push("-k".to_string());
                cmd.push(filter.to_string());
            }
            ("cpp", VerifyKind::Test) => {
                cmd.push("-R".to_string());
                cmd.push(filter.to_string());
            }
            ("swift", VerifyKind::Test) => {
                cmd.push("--filter".to_string());
                cmd.push(filter.to_string());
            }
            _ => {}
        }
    }
    Ok(cmd)
}

/// Parses one `cargo --message-format=json` line into a diagnostic (None for non-diagnostics
/// and for the trailing "N warnings emitted" summaries).
pub fn parse_cargo_json_line(line: &str) -> Option<Diagnostic> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("reason")?.as_str()? != "compiler-message" {
        return None;
    }
    let message = value.get("message")?;
    let level = message.get("level")?.as_str()?.to_string();
    if level != "error" && level != "warning" {
        return None;
    }
    let spans = message.get("spans").and_then(|s| s.as_array());
    let primary = spans.and_then(|spans| {
        spans
            .iter()
            .find(|s| {
                s.get("is_primary")
                    .and_then(|p| p.as_bool())
                    .unwrap_or(false)
            })
            .or_else(|| spans.first())
    });
    // No span: "aborting due to N previous errors", "N warnings emitted".
    let primary = primary?;
    Some(Diagnostic {
        level,
        code: message
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|c| c.as_str())
            .map(str::to_string),
        message: message.get("message")?.as_str()?.to_string(),
        file: primary
            .get("file_name")
            .and_then(|f| f.as_str())
            .map(str::to_string),
        line: primary.get("line_start").and_then(|l| l.as_u64()),
        column: primary.get("column_start").and_then(|c| c.as_u64()),
    })
}

/// Parses rustc's human-readable output (`error[E0425]: ...` followed by `--> file:line:col`).
pub fn parse_rustc_text(text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut pending: Option<Diagnostic> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        let header = line
            .strip_prefix("error")
            .map(|rest| ("error", rest))
            .or_else(|| line.strip_prefix("warning").map(|rest| ("warning", rest)));
        if let Some((level, rest)) = header
            && let Some((code_part, msg)) = rest.split_once(": ")
            && (code_part.is_empty() || (code_part.starts_with('[') && code_part.ends_with(']')))
        {
            if let Some(d) = pending.take() {
                out.push(d);
            }
            if msg.starts_with("aborting due to")
                || msg.contains("warning(s) emitted")
                || msg.contains("warnings emitted")
                || msg.starts_with("could not compile")
                || msg.starts_with("build failed")
            {
                continue;
            }
            pending = Some(Diagnostic {
                level: level.to_string(),
                code: (!code_part.is_empty())
                    .then(|| code_part.trim_matches(['[', ']']).to_string()),
                message: msg.to_string(),
                file: None,
                line: None,
                column: None,
            });
            continue;
        }
        if let Some(d) = pending.as_mut()
            && d.file.is_none()
            && let Some(loc) = line.trim_start().strip_prefix("--> ")
        {
            let mut parts = loc.rsplitn(3, ':');
            let col = parts.next().and_then(|c| c.parse().ok());
            let ln = parts.next().and_then(|l| l.parse().ok());
            let file = parts.next().map(str::to_string);
            if let Some(file) = file {
                d.file = Some(file);
                d.line = ln;
                d.column = col;
            }
        }
    }
    if let Some(d) = pending {
        out.push(d);
    }
    out
}

/// Parses `cargo test` (libtest) output: per-test results and failure output blocks.
pub fn parse_cargo_test_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures = Vec::new();
    let mut current: Option<TestFailure> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("test result: ")
            && let Some((_, counts)) = rest.split_once(". ")
        {
            for part in counts.split("; ") {
                let mut it = part.split_whitespace();
                if let (Some(n), Some(what)) = (it.next(), it.next())
                    && let Ok(n) = n.parse::<u64>()
                {
                    match what {
                        "passed" => passed += n,
                        "failed" => failed += n,
                        _ => {}
                    }
                }
            }
            continue;
        }
        if let Some(name) = line
            .strip_prefix("---- ")
            .and_then(|r| r.strip_suffix(" stdout ----"))
        {
            if let Some(f) = current.take() {
                failures.push(f);
            }
            current = Some(TestFailure {
                name: name.to_string(),
                output: String::new(),
            });
            continue;
        }
        if line == "failures:" || line.starts_with("test result:") {
            if let Some(f) = current.take() {
                failures.push(f);
            }
            continue;
        }
        if let Some(f) = current.as_mut() {
            f.output.push_str(line);
            f.output.push('\n');
        }
    }
    if let Some(f) = current {
        failures.push(f);
    }
    (passed, failed, failures)
}

/// Parses `path:line:col: (error|warning|note): message` lines as emitted by clang, gcc,
/// cmake builds, ruff (`--output-format concise`) and eslint (`-f unix`).
pub fn parse_colon_diagnostics(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let mut parts = line.splitn(4, ':');
            let file = parts.next()?.trim();
            let ln = parts.next()?.trim().parse::<u64>().ok()?;
            let col_part = parts.next()?.trim();
            let rest = parts.next()?.trim();
            if file.is_empty() || file.contains(' ') {
                return None;
            }
            let (col, message) = match col_part.parse::<u64>() {
                Ok(col) => (Some(col), rest.to_string()),
                Err(_) => (None, format!("{col_part}: {rest}")),
            };
            let (level, message) = if let Some(m) = message.strip_prefix("error:") {
                ("error", m.trim().to_string())
            } else if let Some(m) = message.strip_prefix("warning:") {
                ("warning", m.trim().to_string())
            } else if let Some(m) = message.strip_prefix("fatal error:") {
                ("error", m.trim().to_string())
            } else if message.starts_with("note:") {
                return None;
            } else {
                ("error", message)
            };
            Some(Diagnostic {
                level: level.to_string(),
                code: None,
                message,
                file: Some(file.to_string()),
                line: Some(ln),
                column: col,
            })
        })
        .collect()
}

/// Parses `tsc --pretty false` lines: `src/a.ts(12,5): error TS2322: message`.
pub fn parse_tsc_text(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| {
            let (loc, rest) = line.split_once("): ")?;
            let (file, pos) = loc.rsplit_once('(')?;
            let (ln, col) = pos.split_once(',')?;
            let (level, rest) = rest.split_once(' ')?;
            let (code, message) = rest.split_once(": ")?;
            Some(Diagnostic {
                level: level.to_string(),
                code: Some(code.to_string()),
                message: message.to_string(),
                file: Some(file.trim().to_string()),
                line: ln.parse().ok(),
                column: col.parse().ok(),
            })
        })
        .collect()
}

/// Parses `basedpyright --outputjson`: `generalDiagnostics[]` with file, range and severity.
pub fn parse_pyright_json(text: &str) -> Vec<Diagnostic> {
    let Some(start) = text.find('{') else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text[start..]) else {
        return Vec::new();
    };
    value
        .get("generalDiagnostics")
        .and_then(|d| d.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|d| {
                    let level = d.get("severity")?.as_str()?;
                    if level != "error" && level != "warning" {
                        return None;
                    }
                    Some(Diagnostic {
                        level: level.to_string(),
                        code: d.get("rule").and_then(|r| r.as_str()).map(str::to_string),
                        message: d.get("message")?.as_str()?.to_string(),
                        file: d.get("file").and_then(|f| f.as_str()).map(str::to_string),
                        line: d
                            .pointer("/range/start/line")
                            .and_then(|l| l.as_u64())
                            .map(|l| l + 1),
                        column: d
                            .pointer("/range/start/character")
                            .and_then(|c| c.as_u64())
                            .map(|c| c + 1),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses pytest `-q -rf` output: the `FAILED path::test - message` summary lines and the
/// final `N passed, M failed` line.
pub fn parse_pytest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("FAILED ") {
            let (name, msg) = rest.split_once(" - ").unwrap_or((rest, ""));
            failures.push(TestFailure {
                name: name.trim().to_string(),
                output: msg.trim().to_string(),
            });
        }
        if line.contains(" passed") || line.contains(" failed") {
            for part in line.trim_matches(|c| c == '=' || c == ' ').split(", ") {
                let mut it = part.split_whitespace();
                if let (Some(n), Some(what)) = (it.next(), it.next())
                    && let Ok(n) = n.parse::<u64>()
                {
                    match what.trim_end_matches(|c: char| !c.is_alphabetic()) {
                        "passed" => passed = n,
                        "failed" => failed = n,
                        _ => {}
                    }
                }
            }
        }
    }
    (passed, failed, failures)
}

/// Parses `go build` / `go vet` output lines of the form `path/file.go:12:34: message`.
pub fn parse_go_text(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                return None;
            }
            let mut parts = line.splitn(4, ':');
            let file = parts.next()?;
            let ln = parts.next()?.trim().parse::<u64>().ok()?;
            let rest = parts.next()?;
            let (col, message) = match (rest.trim().parse::<u64>(), parts.next()) {
                (Ok(col), Some(msg)) => (Some(col), msg.trim().to_string()),
                _ => (None, rest.trim().to_string()),
            };
            if !file.ends_with(".go") {
                return None;
            }
            Some(Diagnostic {
                level: "error".to_string(),
                code: None,
                message,
                file: Some(file.to_string()),
                line: Some(ln),
                column: col,
            })
        })
        .collect()
}

/// Parses `go test -json` events into pass/fail counts and per-test failure output.
pub fn parse_go_test_json(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut outputs: std::collections::BTreeMap<String, String> = Default::default();
    let mut failures = Vec::new();
    for line in text.lines() {
        let Ok(ev) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(test) = ev.get("Test").and_then(|t| t.as_str()) else {
            continue;
        };
        let key = format!(
            "{}.{}",
            ev.get("Package").and_then(|p| p.as_str()).unwrap_or(""),
            test
        );
        match ev.get("Action").and_then(|a| a.as_str()) {
            Some("output") => {
                outputs
                    .entry(key)
                    .or_default()
                    .push_str(ev.get("Output").and_then(|o| o.as_str()).unwrap_or(""));
            }
            Some("pass") => passed += 1,
            Some("fail") => {
                failed += 1;
                failures.push(TestFailure {
                    name: key.clone(),
                    output: outputs.remove(&key).unwrap_or_default(),
                });
            }
            _ => {}
        }
    }
    (passed, failed, failures)
}

/// Parses XCTest (`swift test`) output on macOS and Linux: `Test Case '-[Suite test]' passed
/// (0.001 seconds)` / `Test Case 'Suite.test' failed`, assertion lines `file:line: error:
/// -[Suite test] : message`, plus swift-testing `✔ Test "name" passed` / `✘ Test "name" failed`.
pub fn parse_xctest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut assertions: Vec<(String, String)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("Test Case '") {
            let Some((name, status)) = rest.split_once("' ") else {
                continue;
            };
            let name = name
                .trim_start_matches("-[")
                .trim_end_matches(']')
                .replace(' ', ".");
            if status.starts_with("passed") {
                passed += 1;
            } else if status.starts_with("failed") {
                failed += 1;
                let output = assertions
                    .iter()
                    .filter(|(test, _)| *test == name)
                    .map(|(_, msg)| msg.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                failures.push(TestFailure { name, output });
            }
            continue;
        }
        if let Some(pos) = line.find(": error: -[") {
            let location = &line[..pos];
            let rest = &line[pos + ": error: -[".len()..];
            if let Some((test, message)) = rest.split_once("] : ") {
                assertions.push((
                    test.replace(' ', "."),
                    format!("{location}: {}", message.trim()),
                ));
            }
            continue;
        }
        // swift-testing: `✔ Test "adds"() passed after 0.001 seconds.`
        if let Some(rest) = line
            .strip_prefix("✔ Test ")
            .or_else(|| line.strip_prefix("✘ Test "))
        {
            if rest.starts_with("run ") {
                continue;
            }
            let name = rest
                .split(" passed")
                .next()
                .and_then(|n| n.split(" failed").next())
                .unwrap_or(rest)
                .trim_matches('"')
                .to_string();
            if line.starts_with('✔') {
                passed += 1;
            } else {
                failed += 1;
                failures.push(TestFailure {
                    name,
                    output: line.to_string(),
                });
            }
        }
    }
    (passed, failed, failures)
}

/// Parses `ctest --output-on-failure` summaries: `1/3 Test #1: name ...... Passed` /
/// `***Failed`, with the failing test's output captured until the next test line.
pub fn parse_ctest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    let flush = |current: &mut Option<(String, Vec<String>)>, failures: &mut Vec<TestFailure>| {
        if let Some((name, lines)) = current.take() {
            failures.push(TestFailure {
                name,
                output: lines.join("\n"),
            });
        }
    };
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        let is_test_line = trimmed
            .split_once(' ')
            .is_some_and(|(n, rest)| n.contains('/') && rest.starts_with("Test #"));
        if is_test_line {
            flush(&mut current, &mut failures);
            let name = trimmed
                .split_once(": ")
                .map(|(_, r)| r.split(" .").next().unwrap_or(r).trim().to_string())
                .unwrap_or_default();
            if trimmed.ends_with("Passed") || trimmed.contains(" Passed ") {
                passed += 1;
            } else if trimmed.contains("Failed") || trimmed.contains("Timeout") {
                failed += 1;
                current = Some((name, Vec::new()));
            }
            continue;
        }
        if trimmed.starts_with("% tests passed") || trimmed.contains("% tests passed,") {
            flush(&mut current, &mut failures);
            continue;
        }
        if let Some((_, lines)) = current.as_mut() {
            lines.push(line.to_string());
        }
    }
    flush(&mut current, &mut failures);
    (passed, failed, failures)
}

/// Rewrites diagnostic file paths that the tool printed as absolute server paths into
/// checkout-relative ones.
fn relativize_diagnostics(diagnostics: &mut [Diagnostic], server_root: &str) {
    if server_root.is_empty() {
        return;
    }
    let prefix = format!("{}/", server_root.trim_end_matches('/'));
    for diagnostic in diagnostics {
        if let Some(file) = diagnostic.file.as_mut()
            && let Some(rel) = file.strip_prefix(&prefix)
        {
            *file = rel.to_string();
        }
    }
}

/// Whether `root` is an Xcode project or workspace rather than a SwiftPM package: then
/// `xcodebuild` drives builds and tests (needed for app bundles, simulators and UI tests).
pub fn has_xcode_project(root: &Path) -> bool {
    if root.join("Package.swift").exists() {
        return false;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".xcodeproj") || name.ends_with(".xcworkspace")
            })
        })
        .unwrap_or(false)
}

/// `xcodebuild build` / `xcodebuild test` for an Xcode project: the scheme is `filter` when
/// given, otherwise the first scheme `xcodebuild -list` reports; iOS targets run on the
/// first available iPhone simulator, everything else on the Mac.
pub fn plan_xcode_command(kind: VerifyKind, scheme: Option<&str>) -> Result<Vec<String>> {
    let action = match kind {
        VerifyKind::Check => "build",
        VerifyKind::Test => "test",
        VerifyKind::Lint => return Err(anyhow!("no lint command for Xcode projects")),
    };
    let scheme_expr = match scheme.filter(|s| !s.is_empty()) {
        Some(s) => format!("'{}'", s.replace('\'', "'\\''")),
        None => "\"$(xcodebuild -list -json 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); d=d.get(\"project\") or d.get(\"workspace\"); print(d[\"schemes\"][0])')\"".to_string(),
    };
    let script = format!(
        "scheme={scheme_expr}; \
if xcodebuild -showBuildSettings -scheme \"$scheme\" 2>/dev/null | grep -q 'SDKROOT.*iPhoneOS'; then \
  dest=\"platform=iOS Simulator,name=$(xcrun simctl list devices available | grep -m1 -o 'iPhone[^(]*' | sed 's/ *$//')\"; \
else dest='platform=macOS'; fi; \
xcodebuild {action} -scheme \"$scheme\" -destination \"$dest\" -quiet 2>&1"
    );
    Ok(vec!["sh".to_string(), "-c".to_string(), script])
}

/// Runs the verification remotely and parses its output.
pub async fn run_verify(
    remote: SocketAddr,
    root: &Path,
    kind: VerifyKind,
    filter: Option<&str>,
    timeout_secs: u64,
) -> Result<VerifyReport> {
    let language = expected_engine(root)
        .ok_or_else(|| anyhow!("no Cargo.toml or go.mod at {}", root.display()))?;
    let command = if language == "swift" && has_xcode_project(root) {
        plan_xcode_command(kind, filter)?
    } else {
        plan_command(language, kind, filter)?
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut tail = TailBuffer::new(8 * 1024);
    let outcome = run_remote(
        remote,
        root,
        command.clone(),
        vec![
            ("CARGO_TERM_COLOR".to_string(), "never".to_string()),
            ("NO_COLOR".to_string(), "1".to_string()),
        ],
        timeout_secs,
        false,
        |is_stderr, data| {
            tail.push(data);
            if is_stderr {
                stderr.extend_from_slice(data);
            } else {
                stdout.extend_from_slice(data);
            }
        },
    )
    .await?;
    if let Some(err) = &outcome.exit.error {
        return Err(anyhow!("remote {} failed to start: {err}", kind.label()));
    }
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);

    let mut diagnostics = Vec::new();
    let (mut tests_passed, mut tests_failed, mut failures) = (0, 0, Vec::new());
    match (language, kind) {
        ("rust", VerifyKind::Check) | ("rust", VerifyKind::Lint) => {
            diagnostics.extend(stdout.lines().filter_map(parse_cargo_json_line));
        }
        ("rust", VerifyKind::Test) => {
            diagnostics.extend(parse_rustc_text(&stderr));
            let (p, f, fails) = parse_cargo_test_text(&stdout);
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("go", VerifyKind::Test) => {
            diagnostics.extend(parse_go_text(&stderr));
            let (p, f, fails) = parse_go_test_json(&stdout);
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("go", _) => {
            diagnostics.extend(parse_go_text(&stderr));
            diagnostics.extend(parse_go_text(&stdout));
        }
        ("typescript", VerifyKind::Check) => {
            diagnostics.extend(parse_tsc_text(&stdout));
            diagnostics.extend(parse_tsc_text(&stderr));
        }
        ("python", VerifyKind::Check) => {
            diagnostics.extend(parse_pyright_json(&stdout));
        }
        ("python", VerifyKind::Test) => {
            let (p, f, fails) = parse_pytest_text(&stdout);
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("swift", VerifyKind::Test) => {
            let combined = format!("{stdout}\n{stderr}");
            diagnostics.extend(parse_colon_diagnostics(&stderr));
            diagnostics.retain(|d| !d.message.starts_with("-["));
            let (p, f, fails) = parse_xctest_text(&combined);
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("cpp", VerifyKind::Test) => {
            let (p, f, fails) = parse_ctest_text(&stdout);
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        _ => {
            diagnostics.extend(parse_colon_diagnostics(&stderr));
            diagnostics.extend(parse_colon_diagnostics(&stdout));
        }
    }
    diagnostics.dedup();
    relativize_diagnostics(&mut diagnostics, &outcome.exit.server_workspace_root);
    for failure in &mut failures {
        let prefix = format!(
            "{}/",
            outcome.exit.server_workspace_root.trim_end_matches('/')
        );
        if prefix.len() > 1 {
            failure.output = failure.output.replace(&prefix, "");
        }
    }

    Ok(VerifyReport {
        kind,
        language: language.to_string(),
        command,
        exit_code: outcome.exit.exit_code,
        timed_out: outcome.exit.timed_out,
        duration_ms: outcome.exit.duration_ms,
        diagnostics,
        tests_passed,
        tests_failed,
        failures,
        tail: tail.text(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_xcodebuild_commands() {
        let cmd = plan_xcode_command(VerifyKind::Test, Some("Tako")).unwrap();
        assert_eq!(&cmd[..2], &["sh".to_string(), "-c".to_string()]);
        assert!(cmd[2].contains("scheme='Tako'"));
        assert!(cmd[2].contains("xcodebuild test -scheme"));
        let cmd = plan_xcode_command(VerifyKind::Check, None).unwrap();
        assert!(cmd[2].contains("xcodebuild -list -json"));
        assert!(cmd[2].contains("xcodebuild build -scheme"));
        assert!(plan_xcode_command(VerifyKind::Lint, None).is_err());
        let temp = tempfile::tempdir().unwrap();
        assert!(!has_xcode_project(temp.path()));
        std::fs::create_dir_all(temp.path().join("App.xcodeproj")).unwrap();
        assert!(has_xcode_project(temp.path()));
        std::fs::write(temp.path().join("Package.swift"), "").unwrap();
        assert!(!has_xcode_project(temp.path()));
    }

    #[test]
    fn parses_xctest_output() {
        let text = "Test Case '-[SignalTests.SignalTests testDoubles]' started.\n\
/srv/ws/Tests/SignalTests/SignalTests.swift:6: error: -[SignalTests.SignalTests testDoubles] : XCTAssertEqual failed: (\"42\") is not equal to (\"43\")\n\
Test Case '-[SignalTests.SignalTests testDoubles]' failed (0.411 seconds).\n\
Test Case 'OtherTests.testOk' passed (0.001 seconds).\n\
\t Executed 2 tests, with 1 failure (0 unexpected) in 0.4 (0.4) seconds\n\
✔ Test run with 0 tests in 0 suites passed after 0.001 seconds.\n";
        let (passed, failed, failures) = parse_xctest_text(text);
        assert_eq!((passed, failed), (1, 1));
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].name, "SignalTests.SignalTests.testDoubles");
        assert!(
            failures[0]
                .output
                .contains("SignalTests.swift:6: XCTAssertEqual failed")
        );
    }

    #[test]
    fn parses_ctest_output() {
        let text = "Test project /srv/ws/build\n\
    Start 1: adds\n\
1/2 Test #1: adds .............................   Passed    0.01 sec\n\
    Start 2: fails\n\
2/2 Test #2: fails ............................***Failed    0.02 sec\n\
expected 42, got 43\n\
\n\
50% tests passed, 1 tests failed out of 2\n";
        let (passed, failed, failures) = parse_ctest_text(text);
        assert_eq!((passed, failed), (1, 1));
        assert_eq!(failures[0].name, "fails");
        assert!(failures[0].output.contains("expected 42, got 43"));
    }

    #[test]
    fn relativizes_server_paths() {
        let mut diagnostics = vec![Diagnostic {
            level: "error".into(),
            code: None,
            message: "boom".into(),
            file: Some("/srv/ws/src/a.cpp".into()),
            line: Some(1),
            column: None,
        }];
        relativize_diagnostics(&mut diagnostics, "/srv/ws");
        assert_eq!(diagnostics[0].file.as_deref(), Some("src/a.cpp"));
    }

    #[test]
    fn cargo_json_diagnostic() {
        let line = r#"{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0425"},"message":"cannot find value `x` in this scope","spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":9,"is_primary":true}]}}"#;
        let d = parse_cargo_json_line(line).unwrap();
        assert_eq!(
            d.render(),
            "error: [E0425] cannot find value `x` in this scope (src/lib.rs:3:9)"
        );
        let summary = r#"{"reason":"compiler-message","message":{"level":"warning","message":"2 warnings emitted","spans":[]}}"#;
        assert!(parse_cargo_json_line(summary).is_none());
        assert!(parse_cargo_json_line(r#"{"reason":"build-finished","success":true}"#).is_none());
    }

    #[test]
    fn rustc_text_diagnostics() {
        let text = "error[E0308]: mismatched types\n  --> crates/a/src/lib.rs:12:5\n   |\nwarning: unused variable: `y`\n --> src/main.rs:4:9\nerror: aborting due to 1 previous error\n";
        let d = parse_rustc_text(text);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].code.as_deref(), Some("E0308"));
        assert_eq!(d[0].file.as_deref(), Some("crates/a/src/lib.rs"));
        assert_eq!((d[0].line, d[0].column), (Some(12), Some(5)));
        assert_eq!(d[1].level, "warning");
        assert_eq!(d[1].file.as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn cargo_test_text() {
        let text = "running 3 tests\ntest a::ok ... ok\ntest a::bad ... FAILED\ntest a::also ... ok\n\nfailures:\n\n---- a::bad stdout ----\nthread 'a::bad' panicked at src/lib.rs:5:9:\nassertion failed: 1 == 2\n\n\nfailures:\n    a::bad\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        let (p, f, fails) = parse_cargo_test_text(text);
        assert_eq!((p, f), (2, 1));
        assert_eq!(fails.len(), 1);
        assert_eq!(fails[0].name, "a::bad");
        assert!(fails[0].output.contains("assertion failed: 1 == 2"));
    }

    #[test]
    fn go_text_and_json() {
        let d = parse_go_text("# prod/cmd\ncmd/main.go:10:2: undefined: foo\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].render(), "error: undefined: foo (cmd/main.go:10:2)");
        let events = r#"{"Action":"run","Package":"p","Test":"TestA"}
{"Action":"output","Package":"p","Test":"TestA","Output":"    a_test.go:7: boom\n"}
{"Action":"fail","Package":"p","Test":"TestA","Elapsed":0}
{"Action":"pass","Package":"p","Test":"TestB","Elapsed":0}
{"Action":"fail","Package":"p","Elapsed":0.1}"#;
        let (p, f, fails) = parse_go_test_json(events);
        assert_eq!((p, f), (1, 1));
        assert_eq!(fails[0].name, "p.TestA");
        assert!(fails[0].output.contains("boom"));
    }

    #[test]
    fn colon_tsc_pyright_pytest_parsers() {
        let d = parse_colon_diagnostics(
            "src/a.cpp:12:5: error: no member named 'x'\nsrc/b.cpp:3:1: warning: unused\nnote: ignored\n",
        );
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].render(), "error: no member named 'x' (src/a.cpp:12:5)");
        assert_eq!(d[1].level, "warning");
        let t = parse_tsc_text(
            "src/index.ts(7,3): error TS2322: Type 'string' is not assignable to type 'number'.\n",
        );
        assert_eq!(t.len(), 1);
        assert_eq!(
            t[0].render(),
            "error: [TS2322] Type 'string' is not assignable to type 'number'. (src/index.ts:7:3)"
        );
        let py = parse_pyright_json(
            r#"{"generalDiagnostics":[{"file":"/w/a.py","severity":"error","message":"boom","range":{"start":{"line":4,"character":2}},"rule":"reportGeneralTypeIssues"}],"summary":{}}"#,
        );
        assert_eq!(
            py[0].render(),
            "error: [reportGeneralTypeIssues] boom (/w/a.py:5:3)"
        );
        let (p, f, fails) = parse_pytest_text(
            "FAILED tests/test_a.py::test_x - AssertionError: nope\n===== 1 failed, 3 passed in 0.10s =====\n",
        );
        assert_eq!((p, f), (3, 1));
        assert_eq!(fails[0].name, "tests/test_a.py::test_x");
        assert!(
            plan_command("swift", VerifyKind::Test, Some("Foo"))
                .unwrap()
                .ends_with(&["--filter".to_string(), "Foo".to_string()])
        );
        assert!(plan_command("cpp", VerifyKind::Lint, None).is_err());
    }

    #[test]
    fn plans_and_summary() {
        assert_eq!(
            plan_command("rust", VerifyKind::Test, Some("sync::"))
                .unwrap()
                .last()
                .unwrap(),
            "sync::"
        );
        assert_eq!(
            plan_command("go", VerifyKind::Test, Some("TestA")).unwrap()[3..],
            ["./...", "-run", "TestA"]
        );
        assert!(plan_command("ruby", VerifyKind::Check, None).is_err());
        let report = VerifyReport {
            kind: VerifyKind::Test,
            language: "rust".into(),
            command: vec!["cargo".into(), "test".into()],
            exit_code: Some(101),
            timed_out: false,
            duration_ms: 1500,
            diagnostics: vec![],
            tests_passed: 2,
            tests_failed: 1,
            failures: vec![TestFailure {
                name: "a::bad".into(),
                output: "boom\n".into(),
            }],
            tail: String::new(),
        };
        assert_eq!(
            report.summary(),
            "rust test: FAILED (exit 101) in 1.5s; 2 passed, 1 failed"
        );
        assert!(report.render(10).contains("--- FAILED a::bad ---\nboom"));
    }
}
