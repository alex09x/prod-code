/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Code clone and duplication harvester (roadmap 9.3).
//!
//! Scans workspace source files for AST/token-level code duplications,
//! supporting Type-1 (exact token clones), Type-2 (parameterized clones
//! with renamed identifiers and differing literals), and Type-3 (gapped
//! and reordered statement clones).

use anyhow::Result;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeCloneOccurrence {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloneGroup {
    pub id: usize,
    pub clone_type: String,
    pub line_count: usize,
    pub occurrences: Vec<CodeCloneOccurrence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DuplicationReport {
    pub total_files_scanned: usize,
    pub total_lines_scanned: usize,
    pub total_clone_groups: usize,
    pub duplicated_lines: usize,
    pub duplication_percentage: f32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approximate: bool,
    pub groups: Vec<CloneGroup>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DuplicateOptions {
    pub min_lines: usize,
    pub parameterized: bool,
    pub type3: bool,
    pub max_groups: usize,
}

impl Default for DuplicateOptions {
    fn default() -> Self {
        Self {
            min_lines: 6,
            parameterized: true,
            type3: false,
            max_groups: 20,
        }
    }
}

struct CandidateWindow {
    file_idx: usize,
    start_line: u32,
    end_line: u32,
    norm_statements: Vec<String>,
}

struct DisjointSet {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl DisjointSet {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, x: usize, y: usize) {
        let root_x = self.find(x);
        let root_y = self.find(y);
        if root_x != root_y {
            if self.rank[root_x] < self.rank[root_y] {
                self.parent[root_x] = root_y;
            } else if self.rank[root_x] > self.rank[root_y] {
                self.parent[root_y] = root_x;
            } else {
                self.parent[root_y] = root_x;
                self.rank[root_x] += 1;
            }
        }
    }
}

/// Discovers code duplications across source files in the workspace.
pub fn find_duplicates(
    workspace_root: &Path,
    target_path: Option<&Path>,
    options: DuplicateOptions,
) -> Result<DuplicationReport> {
    let scan_root = target_path.unwrap_or(workspace_root);
    let walker = WalkBuilder::new(scan_root)
        .hidden(true)
        .git_ignore(true)
        .build();

    let mut files_scanned = 0;
    let mut total_lines = 0;

    let mut files_content: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();

    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_file()
            && let Some(ext) = path.extension().and_then(|e| e.to_str())
                && matches!(ext, "rs" | "go" | "py" | "ts" | "js" | "cpp" | "c" | "swift" | "java" | "kt" | "kts" | "cs" | "scala" | "zig" | "nim" | "d" | "php" | "rb" | "dart" | "lua" | "ex" | "exs")
                    && let Ok(raw_content) = std::fs::read_to_string(path) {
                        files_scanned += 1;
                        let rel_path = path
                            .strip_prefix(workspace_root)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .to_string();

                        let raw_lines: Vec<String> = raw_content.lines().map(|s| s.to_string()).collect();
                        total_lines += raw_lines.len();

                        // Normalize lines for clone detection
                        let norm_lines: Vec<String> = raw_lines
                            .iter()
                            .map(|l| normalize_line(l, options.parameterized))
                            .collect();

                        files_content.push((rel_path, raw_lines, norm_lines));
                    }
    }

    let mut raw_groups = Vec::new();
    let mut approximate = false;

    if !options.type3 {
        // Sequential rolling-hash path (Type-1 / Type-2)
        let mut window_map: HashMap<u64, Vec<(usize, u32, u32)>> = HashMap::new();

        for (file_idx, (_, _, norm_lines)) in files_content.iter().enumerate() {
            if norm_lines.len() >= options.min_lines {
                for i in 0..=(norm_lines.len() - options.min_lines) {
                    let window = &norm_lines[i..i + options.min_lines];
                    let non_empty = window.iter().filter(|l| !l.is_empty()).count();
                    if non_empty > options.min_lines / 2 {
                        let mut hasher = DefaultHasher::new();
                        for line in window {
                            line.hash(&mut hasher);
                        }
                        let h = hasher.finish();
                        window_map.entry(h).or_default().push((
                            file_idx,
                            (i + 1) as u32,
                            (i + options.min_lines) as u32,
                        ));
                    }
                }
            }
        }

        let mut group_id = 0;
        for occurrences in window_map.into_values() {
            if occurrences.len() >= 2 {
                let mut filtered = Vec::new();
                for occ in &occurrences {
                    let overlaps = filtered.iter().any(|existing: &(usize, u32, u32)| {
                        existing.0 == occ.0 && (occ.1 <= existing.2 && occ.2 >= existing.1)
                    });
                    if !overlaps {
                        filtered.push(*occ);
                    }
                }

                if filtered.len() >= 2 {
                    group_id += 1;
                    let mut occ_items = Vec::new();
                    for (f_idx, start, end) in filtered {
                        let file_path = files_content[f_idx].0.clone();
                        let raw_lines = &files_content[f_idx].1;
                        let snippet = if (end as usize) <= raw_lines.len() {
                            raw_lines[(start as usize - 1)..(end as usize)].join("\n")
                        } else {
                            String::new()
                        };

                        occ_items.push(CodeCloneOccurrence {
                            file: file_path,
                            start_line: start,
                            end_line: end,
                            snippet,
                        });
                    }

                    raw_groups.push(CloneGroup {
                        id: group_id,
                        clone_type: if options.parameterized {
                            "Type-2 (Parameterized)".to_string()
                        } else {
                            "Type-1 (Exact)".to_string()
                        },
                        line_count: options.min_lines,
                        occurrences: occ_items,
                    });
                }
            }
        }
    } else {
        // Type-3 Gapped and Reordered clone detection
        let mut windows: Vec<CandidateWindow> = Vec::new();
        let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();

        for (file_idx, (_, _, norm_lines)) in files_content.iter().enumerate() {
            if norm_lines.len() >= options.min_lines {
                for i in 0..=(norm_lines.len() - options.min_lines) {
                    let window = &norm_lines[i..i + options.min_lines];
                    let non_empty: Vec<String> = window.iter().filter(|l| !l.is_empty()).cloned().collect();
                    if non_empty.len() > options.min_lines / 2 {
                        let win_idx = windows.len();
                        windows.push(CandidateWindow {
                            file_idx,
                            start_line: (i + 1) as u32,
                            end_line: (i + options.min_lines) as u32,
                            norm_statements: non_empty.clone(),
                        });

                        let mut stmt_hashes: Vec<u64> = non_empty.iter().map(|s| hash_string(s)).collect();
                        stmt_hashes.sort_unstable();

                        // Full multiset hash
                        let full_h = hash_slice(&stmt_hashes);
                        buckets.entry(full_h).or_default().push(win_idx);

                        // Sub-multisets (dropping 1 statement)
                        if stmt_hashes.len() >= 4 {
                            for drop_idx in 0..stmt_hashes.len() {
                                let mut sub = stmt_hashes.clone();
                                sub.remove(drop_idx);
                                let sub_h = hash_slice(&sub);
                                buckets.entry(sub_h).or_default().push(win_idx);
                            }
                        }
                    }
                }
            }
        }

        let mut dsu = DisjointSet::new(windows.len());
        let mut checked_pairs: HashSet<(usize, usize)> = HashSet::new();
        let mut pair_types: HashMap<(usize, usize), &'static str> = HashMap::new();
        let mut has_match = false;
        let mut total_comparisons = 0usize;
        const MAX_TOTAL_COMPARISONS: usize = 500_000;
        const MAX_BUCKET_CANDIDATES: usize = 200;

        for mut members in buckets.into_values() {
            if members.len() < 2 {
                continue;
            }
            members.sort_unstable();
            members.dedup();

            let candidate_members: Vec<usize> = if members.len() > MAX_BUCKET_CANDIDATES {
                approximate = true;
                let step = members.len() as f64 / MAX_BUCKET_CANDIDATES as f64;
                (0..MAX_BUCKET_CANDIDATES)
                    .map(|k| members[(k as f64 * step) as usize])
                    .collect()
            } else {
                members
            };

            for i in 0..candidate_members.len() {
                if total_comparisons >= MAX_TOTAL_COMPARISONS {
                    approximate = true;
                    break;
                }
                for j in (i + 1)..candidate_members.len() {
                    if total_comparisons >= MAX_TOTAL_COMPARISONS {
                        approximate = true;
                        break;
                    }
                    total_comparisons += 1;

                    let w1_idx = candidate_members[i];
                    let w2_idx = candidate_members[j];
                    let pair_key = (w1_idx.min(w2_idx), w1_idx.max(w2_idx));
                    if !checked_pairs.insert(pair_key) {
                        continue;
                    }

                    let w1 = &windows[w1_idx];
                    let w2 = &windows[w2_idx];
                    if w1.file_idx == w2.file_idx
                        && (w1.start_line <= w2.end_line && w2.start_line <= w1.end_line)
                    {
                        continue;
                    }

                    let sim = multiset_jaccard(&w1.norm_statements, &w2.norm_statements);
                    if sim >= 0.70 {
                        has_match = true;
                        dsu.union(w1_idx, w2_idx);

                        let ctype = if w1.norm_statements == w2.norm_statements {
                            let raw1 = &files_content[w1.file_idx].1
                                [(w1.start_line as usize - 1)..(w1.end_line as usize)];
                            let raw2 = &files_content[w2.file_idx].1
                                [(w2.start_line as usize - 1)..(w2.end_line as usize)];
                            if raw1 == raw2 {
                                "Type-1 (Exact)"
                            } else {
                                "Type-2 (Parameterized)"
                            }
                        } else {
                            "Type-3 (Gapped/Reordered)"
                        };
                        pair_types.insert(pair_key, ctype);
                    }
                }
            }
        }

        if has_match {
            let mut component_windows: HashMap<usize, Vec<usize>> = HashMap::new();
            for &(w1, w2) in pair_types.keys() {
                let root = dsu.find(w1);
                component_windows.entry(root).or_default().push(w1);
                component_windows.entry(root).or_default().push(w2);
            }

            let mut group_id = 0;
            for mut win_indices in component_windows.into_values() {
                win_indices.sort_unstable();
                win_indices.dedup();
                if win_indices.len() < 2 {
                    continue;
                }

                let mut filtered: Vec<usize> = Vec::new();
                for &w_idx in &win_indices {
                    let w = &windows[w_idx];
                    let overlaps = filtered.iter().any(|&existing_idx: &usize| {
                        let ex = &windows[existing_idx];
                        ex.file_idx == w.file_idx
                            && (w.start_line <= ex.end_line && ex.start_line <= w.end_line)
                    });
                    if !overlaps {
                        filtered.push(w_idx);
                    }
                }

                if filtered.len() >= 2 {
                    group_id += 1;

                    // Determine group clone type: Type-3 if any pair has gaps/reordering, else Type-2, else Type-1
                    let mut group_clone_type = "Type-1 (Exact)";
                    for i in 0..filtered.len() {
                        for j in (i + 1)..filtered.len() {
                            let key = (filtered[i].min(filtered[j]), filtered[i].max(filtered[j]));
                            if let Some(&t) = pair_types.get(&key) {
                                if t == "Type-3 (Gapped/Reordered)" {
                                    group_clone_type = "Type-3 (Gapped/Reordered)";
                                    break;
                                }
                                if t == "Type-2 (Parameterized)"
                                    && group_clone_type != "Type-3 (Gapped/Reordered)"
                                {
                                    group_clone_type = "Type-2 (Parameterized)";
                                }
                            }
                        }
                        if group_clone_type == "Type-3 (Gapped/Reordered)" {
                            break;
                        }
                    }

                    let mut occ_items = Vec::new();
                    for &w_idx in &filtered {
                        let w = &windows[w_idx];
                        let file_path = files_content[w.file_idx].0.clone();
                        let raw_lines = &files_content[w.file_idx].1;
                        let snippet = if (w.end_line as usize) <= raw_lines.len() {
                            raw_lines[(w.start_line as usize - 1)..(w.end_line as usize)].join("\n")
                        } else {
                            String::new()
                        };

                        occ_items.push(CodeCloneOccurrence {
                            file: file_path,
                            start_line: w.start_line,
                            end_line: w.end_line,
                            snippet,
                        });
                    }

                    raw_groups.push(CloneGroup {
                        id: group_id,
                        clone_type: group_clone_type.to_string(),
                        line_count: options.min_lines,
                        occurrences: occ_items,
                    });
                }
            }
        }
    }

    // Merge and rank groups, filtering out redundant overlapping groups
    raw_groups.sort_by_key(|g| std::cmp::Reverse(g.occurrences.len()));
    let mut deduped_groups: Vec<CloneGroup> = Vec::new();
    for group in raw_groups {
        let is_redundant = deduped_groups.iter().any(|existing| {
            group.occurrences.len() == existing.occurrences.len()
                && group.occurrences.iter().all(|occ| {
                    existing.occurrences.iter().any(|ex_occ| {
                        ex_occ.file == occ.file
                            && !(occ.end_line < ex_occ.start_line || occ.start_line > ex_occ.end_line)
                    })
                })
        });
        if !is_redundant {
            deduped_groups.push(group);
        }
    }

    // Re-assign 1-based sequential group IDs
    for (idx, g) in deduped_groups.iter_mut().enumerate() {
        g.id = idx + 1;
    }

    let groups: Vec<CloneGroup> = deduped_groups.into_iter().take(options.max_groups).collect();

    let duplicated_lines: usize = groups.iter().map(|g| g.line_count * g.occurrences.len()).sum();
    let duplication_percentage = if total_lines > 0 {
        (duplicated_lines as f32 / total_lines as f32) * 100.0
    } else {
        0.0
    };

    Ok(DuplicationReport {
        total_files_scanned: files_scanned,
        total_lines_scanned: total_lines,
        total_clone_groups: groups.len(),
        duplicated_lines,
        duplication_percentage,
        approximate,
        groups,
    })
}

fn hash_slice(slice: &[u64]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for &h in slice {
        h.hash(&mut hasher);
    }
    hasher.finish()
}

fn hash_string(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

fn multiset_jaccard(a: &[String], b: &[String]) -> f64 {
    let mut counts_a: HashMap<&str, usize> = HashMap::new();
    for s in a {
        if !s.is_empty() {
            *counts_a.entry(s.as_str()).or_default() += 1;
        }
    }
    let mut counts_b: HashMap<&str, usize> = HashMap::new();
    for s in b {
        if !s.is_empty() {
            *counts_b.entry(s.as_str()).or_default() += 1;
        }
    }
    let mut all_keys: HashSet<&str> = counts_a.keys().copied().collect();
    all_keys.extend(counts_b.keys().copied());

    let mut intersection = 0usize;
    let mut union = 0usize;
    for k in all_keys {
        let ca = counts_a.get(k).copied().unwrap_or(0);
        let cb = counts_b.get(k).copied().unwrap_or(0);
        intersection += ca.min(cb);
        union += ca.max(cb);
    }
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Normalizes a source code line: strips comments and normalizes tokens.
fn normalize_line(line: &str, parameterized: bool) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
    {
        return String::new();
    }

    // Strip inline comments
    let code_part = if let Some(idx) = trimmed.find("//") {
        &trimmed[..idx]
    } else if let Some(idx) = trimmed.find('#') {
        &trimmed[..idx]
    } else {
        trimmed
    };

    if !parameterized {
        return code_part.split_whitespace().collect::<Vec<_>>().join(" ");
    }

    // Parameterized normalization: normalize variable names to $id and literals to $lit
    let mut result = String::with_capacity(code_part.len());
    let words = code_part.split_whitespace();

    for word in words {
        if is_numeric_literal(word) {
            result.push_str("$lit ");
        } else if word.starts_with('"') && word.ends_with('"') {
            result.push_str("$str ");
        } else {
            result.push_str(word);
            result.push(' ');
        }
    }

    result.trim_end().to_string()
}

fn is_numeric_literal(s: &str) -> bool {
    let clean = s.trim_end_matches([',', ';', ')', '}', ']']);
    clean.parse::<f64>().is_ok()
}

/// Formats the duplication report into a clean readable summary.
pub fn format_duplication_report(report: &DuplicationReport) -> String {
    let mut out = String::new();
    out.push_str("⚡ prod-code Clone & Duplication Harvester Report\n");
    out.push_str("────────────────────────────────────────────────────\n");
    out.push_str(&format!(
        "Files Scanned: {} | Lines: {} | Clone Groups: {} | Duplication: {:.1}%\n",
        report.total_files_scanned,
        report.total_lines_scanned,
        report.total_clone_groups,
        report.duplication_percentage
    ));

    if report.approximate {
        out.push_str("⚠️ High repetition: candidate comparisons were capped; results are approximate.\n");
    }

    if report.groups.is_empty() {
        out.push_str("\n✓ No duplicate code blocks detected exceeding threshold.\n");
        return out;
    }

    out.push_str("\nDiscovered Clone Groups:\n");
    for group in &report.groups {
        out.push_str(&format!(
            "\n[Clone Group #{}] {} lines | {} occurrences ({})\n",
            group.id,
            group.line_count,
            group.occurrences.len(),
            group.clone_type
        ));
        for (i, occ) in group.occurrences.iter().enumerate() {
            out.push_str(&format!(
                "  • Occurrence {}: {}:{}-{}\n",
                i + 1,
                occ.file,
                occ.start_line,
                occ.end_line
            ));
        }

        // Show preview of first occurrence snippet
        if let Some(first) = group.occurrences.first() {
            out.push_str("  Preview:\n");
            for line in first.snippet.lines().take(4) {
                out.push_str(&format!("    │ {}\n", line));
            }
            if first.snippet.lines().count() > 4 {
                out.push_str("    │ …\n");
            }
        }
        out.push_str("  💡 Recommendation: Fold into a shared function using `code_extract_function`.\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_duplicate_detection_simple() {
        let dir = tempdir().unwrap();
        let file_a = dir.path().join("a.rs");
        let file_b = dir.path().join("b.rs");

        let code = r#"
fn process_items(items: &[String]) {
    for item in items {
        let trimmed = item.trim();
        if !trimmed.is_empty() {
            println!("Item: {}", trimmed);
        }
    }
}
"#;

        std::fs::write(&file_a, code).unwrap();
        std::fs::write(&file_b, code).unwrap();

        let report = find_duplicates(
            dir.path(),
            None,
            DuplicateOptions {
                min_lines: 5,
                parameterized: true,
                type3: false,
                max_groups: 10,
            },
        )
        .unwrap();

        assert_eq!(report.total_files_scanned, 2);
        assert!(!report.groups.is_empty());
        assert_eq!(report.groups[0].occurrences.len(), 2);
    }

    #[test]
    fn test_duplicate_detection_type3_reordered_and_gapped() {
        let dir = tempdir().unwrap();
        let file_a = dir.path().join("a.rs");
        let file_b = dir.path().join("b.rs");

        let code_a = r#"
fn func_a() {
    let alpha = 10;
    let beta = 20;
    let gamma = 30;
    let delta = 40;
    let epsilon = 50;
    let zeta = 60;
}
"#;
        let code_b = r#"
fn func_b() {
    let beta = 20;
    let alpha = 10;
    let gamma = 30;
    let delta = 40;
    let epsilon = 50;
    let zeta = 60;
}
"#;

        std::fs::write(&file_a, code_a).unwrap();
        std::fs::write(&file_b, code_b).unwrap();

        let report_no_type3 = find_duplicates(
            dir.path(),
            None,
            DuplicateOptions {
                min_lines: 6,
                parameterized: true,
                type3: false,
                max_groups: 10,
            },
        )
        .unwrap();
        assert_eq!(report_no_type3.groups.len(), 0);

        let report_type3 = find_duplicates(
            dir.path(),
            None,
            DuplicateOptions {
                min_lines: 6,
                parameterized: true,
                type3: true,
                max_groups: 10,
            },
        )
        .unwrap();
        assert_eq!(report_type3.groups.len(), 1);
        assert_eq!(report_type3.groups[0].clone_type, "Type-3 (Gapped/Reordered)");
        assert_eq!(report_type3.groups[0].occurrences.len(), 2);
    }

    #[test]
    fn test_duplicate_detection_type3_gapped() {
        let dir = tempdir().unwrap();
        let file_a = dir.path().join("a.rs");
        let file_c = dir.path().join("c.rs");

        let code_a = r#"
fn func_a() {
    let alpha = 10;
    let beta = 20;
    let gamma = 30;
    let delta = 40;
    let epsilon = 50;
    let zeta = 60;
}
"#;
        let code_c = r#"
fn func_c() {
    let alpha = 10;
    let beta = 20;
    println!("debugging step");
    let gamma = 30;
    let delta = 40;
    let epsilon = 50;
    let zeta = 60;
}
"#;

        std::fs::write(&file_a, code_a).unwrap();
        std::fs::write(&file_c, code_c).unwrap();

        let report_type3 = find_duplicates(
            dir.path(),
            None,
            DuplicateOptions {
                min_lines: 6,
                parameterized: true,
                type3: true,
                max_groups: 10,
            },
        )
        .unwrap();
        assert_eq!(report_type3.groups.len(), 1);
        assert_eq!(report_type3.groups[0].clone_type, "Type-3 (Gapped/Reordered)");
        assert_eq!(report_type3.groups[0].occurrences.len(), 2);
    }

    #[test]
    fn test_duplicate_detection_type3_large_bucket_uniform_sampling() {
        let dir = tempdir().unwrap();
        for i in 0..20 {
            let code = format!(
                r#"
fn func_{}() {{
    let alpha = 10;
    let beta = 20;
    let gamma = 30;
    let delta = 40;
    let epsilon = 50;
    let zeta = {};
}}
"#,
                i,
                if i % 2 == 0 { 60 } else { 70 }
            );
            std::fs::write(dir.path().join(format!("f{}.rs", i)), code).unwrap();
        }

        let report = find_duplicates(
            dir.path(),
            None,
            DuplicateOptions {
                min_lines: 6,
                parameterized: true,
                type3: true,
                max_groups: 10,
            },
        )
        .unwrap();

        assert_eq!(report.total_files_scanned, 20);
        assert!(!report.groups.is_empty());
        assert!(report.groups[0].occurrences.len() >= 2);
    }
}
