/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use anyhow::Result;
use ignore::WalkBuilder;

use super::normalize::normalize_line;
use super::type3::detect_type3;
use super::types::{CloneGroup, CodeCloneOccurrence, DuplicateOptions, DuplicationReport};

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
            && matches!(
                ext,
                "rs" | "go"
                    | "py"
                    | "ts"
                    | "js"
                    | "cpp"
                    | "c"
                    | "swift"
                    | "java"
                    | "kt"
                    | "kts"
                    | "cs"
                    | "scala"
                    | "zig"
                    | "nim"
                    | "d"
                    | "php"
                    | "rb"
                    | "dart"
                    | "lua"
                    | "ex"
                    | "exs"
            )
            && let Ok(raw_content) = std::fs::read_to_string(path)
        {
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
        let (groups, approx) = detect_type3(&files_content, options.min_lines);
        raw_groups = groups;
        approximate = approx;
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
                            && !(occ.end_line < ex_occ.start_line
                                || occ.start_line > ex_occ.end_line)
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

    let groups: Vec<CloneGroup> = deduped_groups
        .into_iter()
        .take(options.max_groups)
        .collect();

    let duplicated_lines: usize = groups
        .iter()
        .map(|g| g.line_count * g.occurrences.len())
        .sum();
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
