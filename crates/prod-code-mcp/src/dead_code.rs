/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Dead-code scan (roadmap 8.6): functions, methods and types nobody references, found by
//! asking the analyzer for the references of every symbol in the checkout.
//!
//! Only a successful answer with an empty list of references makes a symbol dead. A request
//! that failed, a `null` (the protocol's "no result", which does not say nothing references it)
//! or an answer of another shape leaves the symbol unverified, never dead, and so out of reach of
//! pruning (#435). The same holds for a file's symbols: a list with an entry that cannot be read
//! leaves the whole file unverified, since a symbol skipped is a symbol never judged.

use crate::session::LspSession;
use anyhow::{Result, anyhow};
use serde::Serialize;
use std::net::SocketAddr;
use std::path::Path;

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DeadItem {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// Exported / public: nothing in this checkout uses it, but something outside might.
    pub exported: bool,
}

/// Options controlling dead-code and reachability scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadCodeOptions {
    pub include_exported: bool,
    pub max_files: usize,
    pub reachability: bool,
}

impl Default for DeadCodeOptions {
    fn default() -> Self {
        Self {
            include_exported: false,
            max_files: 400,
            reachability: false,
        }
    }
}

/// Candidate symbol extracted from document symbols with exact declaration range coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateSymbol {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub col: u32,
    pub range_start: (u32, u32),
    pub range_end: (u32, u32),
}

#[derive(Debug, Clone, Serialize)]
pub struct DeadCodeReport {
    pub language: String,
    pub files_scanned: usize,
    pub symbols_checked: usize,
    pub dead: Vec<DeadItem>,
    /// Methods without direct references: they may still be reached through a trait,
    /// interface or protocol, which reference search does not follow.
    pub methods_unreferenced: Vec<DeadItem>,
    /// Exported symbols without references that were not listed (`include_exported` off).
    pub exported_unreferenced: usize,
    pub truncated: bool,
    /// Files and symbols the analyzer could not answer for: nothing is known about them, so
    /// none is listed as dead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unverified: Vec<Unverified>,
    /// Whole-program reachability summary metrics (when reachability analysis is enabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachability: Option<crate::reachability::ReachabilitySummary>,
    /// Unreachable call clusters and circular dead cycles detected by reachability analysis.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreachable_clusters: Vec<crate::reachability::UnreachableCluster>,
    /// Root entry points identified for reachability analysis.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub root_entry_points: Vec<DeadItem>,
}

/// A file whose symbols, or a symbol whose references, the analyzer did not establish.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Unverified {
    pub file: String,
    /// The symbol; `None` when the file's symbols could not be listed at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 1-based line and column of the symbol's name; 0 for a whole file.
    pub line: u32,
    pub col: u32,
    pub reason: String,
}

impl DeadCodeReport {
    /// Whether every source file and every symbol was judged: nothing cut by the file limit,
    /// nothing the analyzer failed to answer.
    pub fn complete(&self) -> bool {
        !self.truncated && self.unverified.is_empty()
    }

    pub fn render(&self) -> String {
        let mut out = if let Some(ref r) = self.reachability {
            format!(
                "whole-program reachability scan ({}): {} file(s), {} symbol(s) checked, {} root(s), {} reachable, {} unreachable\n",
                self.language,
                self.files_scanned,
                self.symbols_checked,
                r.roots_count,
                r.reachable_count,
                r.unreachable_count,
            )
        } else {
            format!(
                "dead code scan ({}): {} file(s), {} symbol(s) checked, {} unreferenced\n",
                self.language,
                self.files_scanned,
                self.symbols_checked,
                self.dead.len()
            )
        };
        for item in &self.dead {
            out.push_str(&format!(
                "  • {} {}{}  {}:{}:{}\n",
                item.kind,
                item.name,
                if item.exported { " (exported)" } else { "" },
                item.file,
                item.line,
                item.col
            ));
        }
        if !self.unreachable_clusters.is_empty() {
            out.push_str(&format!(
                "unreachable circular dead clusters ({}):\n",
                self.unreachable_clusters.len()
            ));
            for cluster in &self.unreachable_clusters {
                out.push_str(&format!(
                    "  • cluster ({} symbols{}, calls: {}):\n",
                    cluster.symbols.len(),
                    if cluster.cycle { ", cycle detected" } else { "" },
                    cluster
                        .internal_calls
                        .iter()
                        .map(|(from, to)| format!("{from} -> {to}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                for item in &cluster.symbols {
                    out.push_str(&format!(
                        "    - {} {}  {}:{}:{}\n",
                        item.kind, item.name, item.file, item.line, item.col
                    ));
                }
            }
        }
        if !self.methods_unreferenced.is_empty() {
            out.push_str(&format!(
                "methods without direct references ({}; may be reached through a trait / interface):\n",
                self.methods_unreferenced.len()
            ));
            for item in &self.methods_unreferenced {
                out.push_str(&format!(
                    "  • {}{}  {}:{}:{}\n",
                    item.name,
                    if item.exported { " (exported)" } else { "" },
                    item.file,
                    item.line,
                    item.col
                ));
            }
        }
        if self.exported_unreferenced > 0 {
            out.push_str(&format!(
                "{} exported symbol(s) are unreferenced inside the checkout (list them with --include-exported)\n",
                self.exported_unreferenced
            ));
        }
        if self.truncated {
            out.push_str("scan truncated by the file limit\n");
        }
        if !self.unverified.is_empty() {
            out.push_str(&format!(
                "{} could not be checked (the analyzer failed or gave no usable answer), so none is listed as dead:\n",
                self.unverified.len()
            ));
            for u in &self.unverified {
                out.push_str(&match &u.name {
                    Some(name) => format!(
                        "  • {name}  {}:{}:{}: {}\n",
                        u.file, u.line, u.col, u.reason
                    ),
                    None => format!("  • {}: {}\n", u.file, u.reason),
                });
            }
        }
        out
    }
}

/// How many references a `textDocument/references` answer lists, or why it says nothing about
/// them. Only a list, empty or not, is an answer; `null` is the protocol's "no result", which
/// does not tell an unreferenced symbol from one the analyzer did not search for.
pub fn reference_count(answer: Result<serde_json::Value>) -> std::result::Result<usize, String> {
    match answer {
        Ok(serde_json::Value::Array(found)) => Ok(found.len()),
        Ok(serde_json::Value::Null) => Err(
            "textDocument/references answered null, which does not say whether anything references it"
                .to_string(),
        ),
        Ok(other) => Err(crate::impact::unreadable("textDocument/references", &other)),
        Err(e) => Err(format!("{e:#}")),
    }
}

fn kind_name(kind: u64) -> Option<&'static str> {
    Some(match kind {
        5 => "class",
        6 => "method",
        10 => "enum",
        11 => "interface",
        12 => "function",
        23 => "struct",
        _ => return None,
    })
}

/// Whether a source line declares an exported / public item in `language`.
pub fn is_exported(language: &str, name: &str, line: &str) -> bool {
    let t = line.trim_start();
    match language {
        "rust" => t.starts_with("pub ") || t.starts_with("pub("),
        "go" => name.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
        "typescript" => t.starts_with("export "),
        "swift" => t.starts_with("public ") || t.starts_with("open "),
        "cpp" => false,
        "python" => !name.starts_with('_'),
        _ => false,
    }
}

fn is_test_path(language: &str, rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
        || match language {
            "go" => lower.ends_with("_test.go"),
            "python" => lower
                .rsplit('/')
                .next()
                .is_some_and(|b| b.starts_with("test_") || b.ends_with("_test.py")),
            "typescript" => lower.contains(".test.") || lower.contains(".spec."),
            "swift" => lower.ends_with("tests.swift"),
            _ => false,
        }
}

fn extensions(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &["rs"],
        "go" => &["go"],
        "python" => &["py"],
        "typescript" => &["ts", "tsx", "js", "jsx", "mts", "cts"],
        "cpp" => &["c", "cc", "cpp", "cxx", "h", "hh", "hpp"],
        "swift" => &["swift"],
        _ => &[],
    }
}

/// Whether a symbol's container is a trait implementation block (`impl Shape for Circle`):
/// its methods are reached through the trait, which reference search does not follow.
fn in_trait_impl(container: &str) -> bool {
    container
        .split(" > ")
        .any(|c| c.starts_with("impl ") && c.contains(" for "))
}

/// The candidates among a document's symbols, children included. An entry that is not a
/// symbol (no name or kind, a container name or children of the wrong shape, a candidate
/// without a readable position) is an error: skipped, it would leave a symbol unjudged in a
/// scan that claims to be complete.
/// The candidates among a document's symbols, children included, with full declaration spans.
pub fn collect_candidates(
    symbols: &[serde_json::Value],
    out: &mut Vec<CandidateSymbol>,
) -> std::result::Result<(), String> {
    for sym in symbols {
        let malformed = || crate::impact::unreadable("textDocument/documentSymbol", sym);
        let (Some(name), Some(kind)) = (
            sym.get("name").and_then(|n| n.as_str()),
            sym.get("kind").and_then(|k| k.as_u64()),
        ) else {
            return Err(malformed());
        };
        if name.is_empty() || !(1..=26).contains(&kind) {
            return Err(malformed());
        }
        let container = match sym.get("containerName") {
            None | Some(serde_json::Value::Null) => String::new(),
            Some(serde_json::Value::String(c)) => c.to_ascii_lowercase(),
            Some(_) => return Err(malformed()),
        };
        // Items inside a test module are tests, whatever they are called. rust-analyzer labels
        // modules by name ("tests"), other servers by kind and name.
        if container.split(" > ").any(|c| {
            let c = c.trim_start_matches("mod ");
            c == "tests" || c == "test" || c.ends_with("tests")
        }) {
            continue;
        }
        if let Some(kind_name) = kind_name(kind)
            && !name.is_empty()
        {
            let (line, col) = sym
                .get("selectionRange")
                .or_else(|| sym.get("range"))
                .or_else(|| sym.get("location").and_then(|l| l.get("range")))
                .and_then(|range| range.get("start"))
                .and_then(|start| {
                    crate::impact::one_based(start, "line")
                        .zip(crate::impact::one_based(start, "character"))
                })
                .ok_or_else(malformed)?;

            let range_val = sym
                .get("range")
                .or_else(|| sym.get("location").and_then(|l| l.get("range")));
            let range_start = range_val
                .and_then(|r| r.get("start"))
                .and_then(|s| {
                    crate::impact::one_based(s, "line")
                        .zip(crate::impact::one_based(s, "character"))
                })
                .unwrap_or((line, col));
            let range_end = range_val
                .and_then(|r| r.get("end"))
                .and_then(|e| {
                    crate::impact::one_based(e, "line")
                        .zip(crate::impact::one_based(e, "character"))
                })
                .unwrap_or((line, col));

            let kind_name = if kind_name == "method" && in_trait_impl(&container) {
                "trait-method"
            } else {
                kind_name
            };
            out.push(CandidateSymbol {
                name: name.to_string(),
                kind: kind_name.to_string(),
                line,
                col,
                range_start,
                range_end,
            });
        }
        match sym.get("children") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(children)) => collect_candidates(children, out)?,
            Some(_) => return Err(malformed()),
        }
    }
    Ok(())
}

fn collect(
    symbols: &[serde_json::Value],
    out: &mut Vec<(String, String, u32, u32)>,
) -> std::result::Result<(), String> {
    let mut detailed = Vec::new();
    collect_candidates(symbols, &mut detailed)?;
    out.extend(
        detailed
            .into_iter()
            .map(|c| (c.name, c.kind, c.line, c.col)),
    );
    Ok(())
}

/// Scans the checkout at `root` (placed on `remote`) for unreferenced symbols.
pub async fn find_dead_code(
    remote: SocketAddr,
    root: &Path,
    include_exported: bool,
    max_files: usize,
) -> Result<DeadCodeReport> {
    find_dead_code_opts(
        remote,
        root,
        DeadCodeOptions {
            include_exported,
            max_files,
            reachability: false,
        },
    )
    .await
}

/// Scans the checkout at `root` (placed on `remote`) with configurable options
/// (including whole-program graph reachability analysis from entry points).
pub async fn find_dead_code_opts(
    remote: SocketAddr,
    root: &Path,
    options: DeadCodeOptions,
) -> Result<DeadCodeReport> {
    let language = crate::sync::expected_engine(root)
        .ok_or_else(|| anyhow!("no project manifest at {}", root.display()))?
        .to_string();
    let exts = extensions(&language);
    let mut files: Vec<String> = crate::sync::scan_workspace_files(root, None)?
        .into_iter()
        .map(|d| d.relative_path)
        .filter(|p| {
            Path::new(p)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e))
        })
        .filter(|p| !is_test_path(&language, p))
        .collect();
    files.sort();
    let truncated = files.len() > options.max_files;
    files.truncate(options.max_files);

    let mut session = LspSession::open(remote, root, None).await?;
    let mut report = DeadCodeReport {
        language: language.clone(),
        files_scanned: 0,
        symbols_checked: 0,
        dead: Vec::new(),
        methods_unreferenced: Vec::new(),
        exported_unreferenced: 0,
        truncated,
        unverified: Vec::new(),
        reachability: None,
        unreachable_clusters: Vec::new(),
        root_entry_points: Vec::new(),
    };
    let whole_file = |file: &str, reason: String| Unverified {
        file: file.to_string(),
        name: None,
        line: 0,
        col: 0,
        reason,
    };

    if options.reachability {
        let mut graph = crate::reachability::ReachabilityGraph::new();
        struct CandidateMeta {
            rel: String,
            abs: std::path::PathBuf,
            name: String,
            line: u32,
            col: u32,
            graph_idx: usize,
        }
        let mut all_candidates: Vec<CandidateMeta> = Vec::new();

        // Pass 1: Parse symbols from files and populate symbol declarations in reachability graph
        for rel in &files {
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
                let exported = is_exported(&language, bare, source_line);
                let (is_root, root_reason) = crate::reachability::is_root_entry_point(
                    &language,
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
                            malformed_reference = Some(format!("a reference location has an invalid URI `{uri}`"));
                            break;
                        }
                        let Some(range) = r.get("range") else {
                            malformed_reference = Some(format!("a reference location for `{uri}` has no range"));
                            break;
                        };
                        let Some(start) = range.get("start") else {
                            malformed_reference = Some(format!("a reference location for `{uri}` has no range start"));
                            break;
                        };
                        let Some(end) = range.get("end") else {
                            malformed_reference = Some(format!("a reference location for `{uri}` has no range end"));
                            break;
                        };
                        let start_pos = crate::impact::one_based(start, "line")
                            .zip(crate::impact::one_based(start, "character"));
                        let end_pos = crate::impact::one_based(end, "line")
                            .zip(crate::impact::one_based(end, "character"));
                        let (Some((r_line, r_col)), Some((end_line, end_col))) = (start_pos, end_pos) else {
                            malformed_reference = Some(format!("a reference location for `{uri}` has malformed coordinates"));
                            break;
                        };
                        if (end_line, end_col) < (r_line, r_col) {
                            malformed_reference = Some(format!("a reference location for `{uri}` has a reversed range"));
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
                    let unattributed = graph.record_references(
                        cand.graph_idx,
                        &ref_locs,
                        |ref_file, _line| is_test_path(&language, ref_file),
                    );
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
    } else {
        // Single-pass reference counting scan
        for rel in &files {
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
                // The protocol's "no result": it does not say the file has no symbols.
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
            if let Err(reason) = collect(&symbols, &mut candidates) {
                report.unverified.push(whole_file(rel, reason));
                continue;
            }
            report.files_scanned += 1;
            for (name, kind, line, col) in candidates {
                let bare = name.split('(').next().unwrap_or(&name);
                if matches!(
                    bare,
                    "main" | "init" | "new" | "default" | "drop" | "fmt" | "eq" | "hash" | "clone"
                ) || bare.starts_with("test")
                    || bare.starts_with("Test")
                    || bare.starts_with("__")
                {
                    continue;
                }
                let source_line = lines.get(line as usize - 1).copied().unwrap_or("");
                let exported = is_exported(&language, bare, source_line);
                report.symbols_checked += 1;
                let refs = session
                    .query(
                        &abs,
                        "textDocument/references",
                        serde_json::json!({
                            "textDocument": { "uri": session.uri_for(&abs)? },
                            "position": { "line": line - 1, "character": col - 1 },
                            "context": { "includeDeclaration": false }
                        }),
                    )
                    .await;
                let count = match reference_count(refs) {
                    Ok(count) => count,
                    Err(reason) => {
                        report.unverified.push(Unverified {
                            file: rel.clone(),
                            name: Some(name),
                            line,
                            col,
                            reason,
                        });
                        continue;
                    }
                };
                if count == 0 {
                    let item = DeadItem {
                        name,
                        kind: kind.clone(),
                        file: rel.clone(),
                        line,
                        col,
                        exported,
                    };
                    // Rust inherent methods are checked like functions; trait-impl methods and
                    // methods in languages with interfaces/protocols go to the "maybe" bucket.
                    if kind == "trait-method" || (kind == "method" && language != "rust") {
                        report.methods_unreferenced.push(item);
                    } else if exported && !options.include_exported {
                        report.exported_unreferenced += 1;
                    } else {
                        report.dead.push(item);
                    }
                }
            }
        }
    }
    session.close().await;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_detection() {
        assert!(is_exported("rust", "f", "pub fn f() {}"));
        assert!(!is_exported("rust", "f", "fn f() {}"));
        assert!(is_exported("go", "Compute", "func Compute() {}"));
        assert!(!is_exported("go", "compute", "func compute() {}"));
        assert!(is_exported("typescript", "f", "export function f() {}"));
        assert!(is_exported("swift", "f", "public func f() {}"));
        assert!(is_test_path("go", "pkg/a_test.go"));
        assert!(in_trait_impl("impl Shape for Circle"));
        assert!(!in_trait_impl("impl Circle"));
        assert!(!is_test_path("rust", "src/lib.rs"));
    }

    #[test]
    fn a_malformed_symbol_fails_the_file_instead_of_being_skipped() {
        let at = |line: serde_json::Value| {
            serde_json::json!({ "name": "f", "kind": 12,
                "selectionRange": { "start": { "line": line, "character": 3 } } })
        };
        let mut out = Vec::new();
        assert_eq!(collect(&[at(serde_json::json!(2))], &mut out), Ok(()));
        assert_eq!(out, vec![("f".to_string(), "function".to_string(), 3, 4)]);
        for bad in [
            serde_json::json!(null),
            serde_json::json!({ "kind": 12 }),
            serde_json::json!({ "name": "", "kind": 12 }),
            serde_json::json!({ "name": "f", "kind": 0 }),
            serde_json::json!({ "name": "f", "kind": 99 }),
            serde_json::json!({ "name": "f", "kind": "12" }),
            serde_json::json!({ "name": "f", "kind": 12 }),
            serde_json::json!({ "name": "f", "kind": 12, "containerName": 3 }),
            at(serde_json::json!(-1)),
            at(serde_json::json!(4_294_967_296u64)),
            serde_json::json!({ "name": "S", "kind": 23, "children": "f",
                "selectionRange": { "start": { "line": 0, "character": 0 } } }),
            serde_json::json!({ "name": "m", "kind": 2, "children": [at(serde_json::json!("2"))] }),
        ] {
            let error = collect(std::slice::from_ref(&bad), &mut Vec::new()).unwrap_err();
            assert!(error.contains("cannot read"), "{bad}: {error}");
        }
        assert_eq!(collect(&[], &mut Vec::new()), Ok(()));
    }

    #[test]
    fn only_a_list_counts_references() {
        assert_eq!(reference_count(Ok(serde_json::json!([]))), Ok(0));
        assert_eq!(reference_count(Ok(serde_json::json!([{}, {}]))), Ok(2));
        assert!(
            reference_count(Ok(serde_json::Value::Null))
                .unwrap_err()
                .contains("null")
        );
        assert!(
            reference_count(Ok(serde_json::json!({ "uri": "x" })))
                .unwrap_err()
                .contains("cannot read")
        );
        assert!(
            reference_count(Err(anyhow!("textDocument/references failed: boom")))
                .unwrap_err()
                .contains("boom")
        );
    }
}
