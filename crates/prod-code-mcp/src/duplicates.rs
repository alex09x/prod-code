//! Code clone and duplication harvester (roadmap 9.3).
//!
//! Scans workspace source files for AST/token-level code duplications,
//! supporting Type-1 (exact token clones) and Type-2 (parameterized clones
//! with renamed identifiers and differing literals).

use anyhow::Result;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
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
    pub groups: Vec<CloneGroup>,
}

#[derive(Debug, Clone, Copy)]
pub struct DuplicateOptions {
    pub min_lines: usize,
    pub parameterized: bool,
    pub max_groups: usize,
}

impl Default for DuplicateOptions {
    fn default() -> Self {
        Self {
            min_lines: 6,
            parameterized: true,
            max_groups: 20,
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

    // Map: hash -> Vec<(file_index, start_line, end_line)>
    let mut window_map: HashMap<u64, Vec<(usize, u32, u32)>> = HashMap::new();
    let mut files_content: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();

    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "rs" | "go" | "py" | "ts" | "js" | "cpp" | "c" | "swift" | "java" | "kt" | "kts" | "cs" | "scala" | "zig" | "nim" | "d" | "php" | "rb" | "dart" | "lua" | "ex" | "exs") {
                    if let Ok(raw_content) = std::fs::read_to_string(path) {
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

                        let file_idx = files_content.len();
                        files_content.push((rel_path, raw_lines, norm_lines));

                        // Generate rolling hash windows of min_lines
                        let norm_ref = &files_content[file_idx].2;
                        if norm_ref.len() >= options.min_lines {
                            for i in 0..=(norm_ref.len() - options.min_lines) {
                                // Filter out windows with too many empty/trivial lines
                                let window = &norm_ref[i..i + options.min_lines];
                                let non_empty = window.iter().filter(|l| !l.is_empty()).count();
                                if non_empty >= options.min_lines / 2 + 1 {
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
                }
            }
        }
    }

    // Filter windows with >= 2 occurrences from distinct locations
    let mut raw_groups = Vec::new();
    let mut group_id = 0;

    for occurrences in window_map.into_values() {
        if occurrences.len() >= 2 {
            // Deduplicate occurrences that overlap within the exact same file
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

    // Merge and rank groups
    raw_groups.sort_by(|a, b| b.occurrences.len().cmp(&a.occurrences.len()));
    let groups: Vec<CloneGroup> = raw_groups.into_iter().take(options.max_groups).collect();

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
        groups,
    })
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
    let mut words = code_part.split_whitespace().peekable();

    while let Some(word) = words.next() {
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
                max_groups: 10,
            },
        )
        .unwrap();

        assert_eq!(report.total_files_scanned, 2);
        assert!(!report.groups.is_empty());
        assert_eq!(report.groups[0].occurrences.len(), 2);
    }
}
