/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::session::LspSession;
use crate::verify::{VerifyKind, VerifyReport, run_verify};
use anyhow::Result;
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;

use super::assertions::parse_assertion_evidence;
use super::locations::{git_diff_of, locations_in_with_hint, suggested};
use super::types::{DossierReport, FailureDossier, FailureSite, Suspect};

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
            let panic_line = sites.first().map(|s| s.line).or_else(|| {
                locations_in_with_hint(root, &failure.output, &failure.name)
                    .first()
                    .map(|(_, l)| *l)
            });
            let expression = assertion.as_ref().and_then(|a| a.expression.clone());
            dossiers.push(FailureDossier {
                test: failure.name.clone(),
                output: failure.output.clone(),
                sites,
                suspects: Vec::new(),
                assertion,
                panic_line,
                expression,
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
        let is_callable = matches!(kind, 6 | 9 | 12)
            || (matches!(kind, 7 | 8 | 13 | 14)
                && sym.get("detail").and_then(|d| d.as_str()).is_some_and(|d| {
                    d.contains("=>") || d.contains("function") || d.contains('(')
                }));
        if is_callable
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
