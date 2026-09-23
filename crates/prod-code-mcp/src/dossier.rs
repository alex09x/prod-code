//! Failure dossier (roadmap 8.2): run the tests (or one filter), and for every failure
//! collect where it happened, the code there, the enclosing function's callers and what
//! changed in that file, so an agent gets the whole picture in one call.

use crate::session::LspSession;
use crate::verify::{VerifyKind, VerifyReport, run_verify};
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
pub struct FailureDossier {
    pub test: String,
    pub output: String,
    pub sites: Vec<FailureSite>,
    /// Changed functions whose callers reach this test, nearest first, with the hops and the
    /// diff of their file when no site above already shows it.
    #[serde(default)]
    pub suspects: Vec<Suspect>,
}

/// A changed function on the path to a failing test.
#[derive(Debug, Clone, Serialize)]
pub struct Suspect {
    pub function: String,
    pub file: String,
    pub line: u32,
    pub hops: usize,
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
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
            dossiers.push(FailureDossier {
                test: failure.name.clone(),
                output: failure.output.clone(),
                sites,
                suspects: Vec::new(),
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
}
