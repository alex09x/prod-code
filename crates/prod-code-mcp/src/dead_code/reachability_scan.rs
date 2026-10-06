/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::{collect_candidates, is_exported, is_test_path};
use super::types::{DeadCodeOptions, DeadCodeReport, Unverified};
use crate::session::LspSession;
use anyhow::Result;
use std::path::{Path, PathBuf};

struct CandidateMeta {
    rel: String,
    abs: PathBuf,
    name: String,
    line: u32,
    col: u32,
    graph_idx: usize,
}

pub(crate) async fn run_reachability_scan(
    session: &mut LspSession,
    root: &Path,
    files: &[String],
    language: &str,
    options: DeadCodeOptions,
    report: &mut DeadCodeReport,
) -> Result<()> {
    let mut graph = crate::reachability::ReachabilityGraph::new();
    let mut all_candidates: Vec<CandidateMeta> = Vec::new();
    let whole_file = |file: &str, reason: String| Unverified {
        file: file.to_string(),
        name: None,
        line: 0,
        col: 0,
        reason,
    };

    // Pass 1: Parse symbols from files and populate symbol declarations in reachability graph
    for rel in files {
        let abs = root.join(rel);
        let text = match std::fs::read_to_string(&abs) {
            Ok(text) => text,
            Err(e) => {
                report
                    .unverified
                    .push(whole_file(rel, format!("it cannot be read: {e}")));
                continue;
            }
        };
        let lines: Vec<&str> = text.lines().collect();
        let uri = session.uri_for(&abs)?;
        let symbols = match session
            .query(
                &abs,
                "textDocument/documentSymbol",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await
        {
            Ok(serde_json::Value::Array(symbols)) => symbols,
            Ok(serde_json::Value::Null) => {
                let reason =
                    "textDocument/documentSymbol answered null, so its symbols are unknown";
                report.unverified.push(whole_file(rel, reason.to_string()));
                continue;
            }
            Ok(other) => {
                let reason = crate::impact::unreadable("textDocument/documentSymbol", &other);
                report.unverified.push(whole_file(rel, reason));
                continue;
            }
            Err(e) => {
                report.unverified.push(whole_file(rel, format!("{e:#}")));
                continue;
            }
        };
        let mut candidates = Vec::new();
        if let Err(reason) = collect_candidates(&symbols, &mut candidates) {
            report.unverified.push(whole_file(rel, reason));
            continue;
        }
        report.files_scanned += 1;

        for cand in candidates {
            let bare = cand.name.split('(').next().unwrap_or(&cand.name);
            let source_line = lines.get(cand.line as usize - 1).copied().unwrap_or("");
            let exported = is_exported(language, bare, source_line);
            let (is_root, root_reason) = crate::reachability::is_root_entry_point(
                language,
                rel,
                &cand.name,
                &cand.kind,
                exported,
                options.include_exported,
            );

            let decl = crate::reachability::SymbolDecl {
                key: crate::reachability::SymbolKey::new(rel, &cand.name, cand.line, cand.col),
                kind: cand.kind.clone(),
                range_start: cand.range_start,
                range_end: cand.range_end,
                exported,
                is_root,
                root_reason,
            };
            let graph_idx = graph.add_symbol(decl);

            all_candidates.push(CandidateMeta {
                rel: rel.clone(),
                abs: abs.clone(),
                name: cand.name,
                line: cand.line,
                col: cand.col,
                graph_idx,
            });
        }
    }

    // Pass 2: Query references and record dependency edges
    for cand in &all_candidates {
        report.symbols_checked += 1;
        let refs = session
            .query(
                &cand.abs,
                "textDocument/references",
                serde_json::json!({
                    "textDocument": { "uri": session.uri_for(&cand.abs)? },
                    "position": { "line": cand.line - 1, "character": cand.col - 1 },
                    "context": { "includeDeclaration": false }
                }),
            )
            .await;

        match refs {
            Ok(serde_json::Value::Array(found)) => {
                let mut ref_locs = Vec::with_capacity(found.len());
                let mut malformed_reference = None;
                for r in &found {
                    let Some(uri) = r.get("uri").and_then(|u| u.as_str()) else {
                        malformed_reference = Some("a reference location has no URI".to_string());
                        break;
                    };
                    if url::Url::parse(uri).is_err() {
                        malformed_reference =
                            Some(format!("a reference location has an invalid URI `{uri}`"));
                        break;
                    }
                    let Some(range) = r.get("range") else {
                        malformed_reference =
                            Some(format!("a reference location for `{uri}` has no range"));
                        break;
                    };
                    let Some(start) = range.get("start") else {
                        malformed_reference = Some(format!(
                            "a reference location for `{uri}` has no range start"
                        ));
                        break;
                    };
                    let Some(end) = range.get("end") else {
                        malformed_reference =
                            Some(format!("a reference location for `{uri}` has no range end"));
                        break;
                    };
                    let start_pos = crate::impact::one_based(start, "line")
                        .zip(crate::impact::one_based(start, "character"));
                    let end_pos = crate::impact::one_based(end, "line")
                        .zip(crate::impact::one_based(end, "character"));
                    let (Some((r_line, r_col)), Some((end_line, end_col))) = (start_pos, end_pos)
                    else {
                        malformed_reference = Some(format!(
                            "a reference location for `{uri}` has malformed coordinates"
                        ));
                        break;
                    };
                    if (end_line, end_col) < (r_line, r_col) {
                        malformed_reference = Some(format!(
                            "a reference location for `{uri}` has a reversed range"
                        ));
                        break;
                    }
                    let path_str = crate::remote_fs::uri_to_path(uri);
                    let rel = Path::new(&path_str)
                        .strip_prefix(root)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or(path_str);
                    ref_locs.push((rel, r_line, r_col));
                }
                if let Some(reason) = malformed_reference {
                    graph.mark_unverified(cand.graph_idx);
                    report.unverified.push(Unverified {
                        file: cand.rel.clone(),
                        name: Some(cand.name.clone()),
                        line: cand.line,
                        col: cand.col,
                        reason,
                    });
                    continue;
                }
                let unattributed =
                    graph.record_references(cand.graph_idx, &ref_locs, |ref_file, _line| {
                        is_test_path(language, ref_file)
                    });
                for (ref_file, ref_line, ref_col) in unattributed {
                    report.unverified.push(Unverified {
                        file: cand.rel.clone(),
                        name: Some(cand.name.clone()),
                        line: cand.line,
                        col: cand.col,
                        reason: format!(
                            "reference at {ref_file}:{ref_line}:{ref_col} cannot be attributed to a scanned caller; the symbol is treated as a possible root"
                        ),
                    });
                }
            }
            Ok(serde_json::Value::Null) => {
                report.unverified.push(Unverified {
                    file: cand.rel.clone(),
                    name: Some(cand.name.clone()),
                    line: cand.line,
                    col: cand.col,
                    reason: "textDocument/references answered null".to_string(),
                });
                graph.mark_unverified(cand.graph_idx);
            }
            Ok(other) => {
                report.unverified.push(Unverified {
                    file: cand.rel.clone(),
                    name: Some(cand.name.clone()),
                    line: cand.line,
                    col: cand.col,
                    reason: crate::impact::unreadable("textDocument/references", &other),
                });
                graph.mark_unverified(cand.graph_idx);
            }
            Err(e) => {
                report.unverified.push(Unverified {
                    file: cand.rel.clone(),
                    name: Some(cand.name.clone()),
                    line: cand.line,
                    col: cand.col,
                    reason: format!("{e:#}"),
                });
                graph.mark_unverified(cand.graph_idx);
            }
        }
    }

    // Pass 3: Compute graph reachability from all roots
    let reach_result = graph.compute_reachability();
    report.reachability = Some(reach_result.summary);
    report.root_entry_points = reach_result.roots;
    report.unreachable_clusters = reach_result.unreachable_clusters;

    for item in reach_result.unreachable_items {
        if item.kind == "trait-method" || (item.kind == "method" && language != "rust") {
            report.methods_unreferenced.push(item);
        } else if item.exported && !options.include_exported {
            report.exported_unreferenced += 1;
        } else {
            report.dead.push(item);
        }
    }

    Ok(())
}
