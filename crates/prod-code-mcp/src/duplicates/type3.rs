/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{HashMap, HashSet};

use super::dsu::{CandidateWindow, DisjointSet};
use super::normalize::{hash_slice, hash_string, multiset_jaccard};
use super::types::{CloneGroup, CodeCloneOccurrence};

const MAX_TOTAL_COMPARISONS: usize = 500_000;
const MAX_BUCKET_CANDIDATES: usize = 200;

/// Type-3 Gapped and Reordered clone detection.
pub(crate) fn detect_type3(
    files_content: &[(String, Vec<String>, Vec<String>)],
    min_lines: usize,
) -> (Vec<CloneGroup>, bool) {
    let mut windows: Vec<CandidateWindow> = Vec::new();
    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut approximate = false;

    for (file_idx, (_, _, norm_lines)) in files_content.iter().enumerate() {
        if norm_lines.len() >= min_lines {
            for i in 0..=(norm_lines.len() - min_lines) {
                let window = &norm_lines[i..i + min_lines];
                let non_empty: Vec<String> =
                    window.iter().filter(|l| !l.is_empty()).cloned().collect();
                if non_empty.len() > min_lines / 2 {
                    let win_idx = windows.len();
                    windows.push(CandidateWindow {
                        file_idx,
                        start_line: (i + 1) as u32,
                        end_line: (i + min_lines) as u32,
                        norm_statements: non_empty.clone(),
                    });

                    let mut stmt_hashes: Vec<u64> =
                        non_empty.iter().map(|s| hash_string(s)).collect();
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

    let mut raw_groups = Vec::new();
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
                    line_count: min_lines,
                    occurrences: occ_items,
                });
            }
        }
    }

    (raw_groups, approximate)
}
