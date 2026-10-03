//! Parser-backed validator for Markdown documentation proposals (#778).

use crate::diagnostics::{DiagnosticsReport, DocDiagnostic};

/// Validates Markdown documentation with optional YAML or TOML frontmatter (#778).
pub fn validate_markdown(shown: &str, text: &str) -> DiagnosticsReport {
    let mut diagnostics = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    let mut body_start_line = 0;

    // 1. Check frontmatter if document starts with `---` (YAML) or `+++` (TOML)
    if let Some(first_line) = lines.first() {
        let trimmed_first = first_line.trim_end();
        if trimmed_first == "---" {
            let mut closing_idx = None;
            for (i, line) in lines.iter().enumerate().skip(1) {
                if line.trim_end() == "---" {
                    closing_idx = Some(i);
                    break;
                }
            }
            match closing_idx {
                Some(end_idx) => {
                    body_start_line = end_idx + 1;
                    validate_yaml_frontmatter(&lines[1..end_idx], &mut diagnostics);
                }
                None => {
                    diagnostics.push(DocDiagnostic {
                        severity: "error".to_string(),
                        message: "unclosed YAML frontmatter: opening `---` on line 1 has no matching closing `---`".to_string(),
                        code: Some("markdown-frontmatter".to_string()),
                        line: 1,
                        col: 1,
                        source: None,
                        end: None,
                        note: None,
                    });
                }
            }
        } else if trimmed_first == "+++" {
            let mut closing_idx = None;
            for (i, line) in lines.iter().enumerate().skip(1) {
                if line.trim_end() == "+++" {
                    closing_idx = Some(i);
                    break;
                }
            }
            match closing_idx {
                Some(end_idx) => {
                    body_start_line = end_idx + 1;
                    let toml_str = lines[1..end_idx].join("\n");
                    if let Err(err) = toml::from_str::<toml::Value>(&toml_str) {
                        let span = err.span();
                        let line_offset = if let Some(span) = span {
                            toml_str[..span.start].chars().filter(|&c| c == '\n').count() as u32
                        } else {
                            0
                        };
                        diagnostics.push(DocDiagnostic {
                            severity: "error".to_string(),
                            message: format!("TOML frontmatter syntax error: {err}"),
                            code: Some("markdown-frontmatter".to_string()),
                            line: 2 + line_offset,
                            col: 1,
                            source: None,
                            end: None,
                            note: None,
                        });
                    }
                }
                None => {
                    diagnostics.push(DocDiagnostic {
                        severity: "error".to_string(),
                        message: "unclosed TOML frontmatter: opening `+++` on line 1 has no matching closing `+++`".to_string(),
                        code: Some("markdown-frontmatter".to_string()),
                        line: 1,
                        col: 1,
                        source: None,
                        end: None,
                        note: None,
                    });
                }
            }
        }
    }

    // 2. Validate Markdown body: code fences and links
    let mut current_fence: Option<(char, usize, u32)> = None;

    for (idx, line) in lines.iter().enumerate().skip(body_start_line) {
        let line_no = (idx + 1) as u32;
        let trimmed_start = line.trim_start();
        let indent = line.len() - trimmed_start.len();

        if indent <= 3 {
            if let Some((c, count, _open_line)) = current_fence {
                let close_count = trimmed_start.chars().take_while(|&ch| ch == c).count();
                let rest = trimmed_start[close_count..].trim();
                if close_count >= count && rest.is_empty() {
                    current_fence = None;
                    continue;
                }
            } else if trimmed_start.starts_with("```") || trimmed_start.starts_with("~~~") {
                let c = trimmed_start.chars().next().unwrap();
                let count = trimmed_start.chars().take_while(|&ch| ch == c).count();
                current_fence = Some((c, count, line_no));
                continue;
            }
        }

        if current_fence.is_some() {
            continue;
        }

        if let Some(link_start) = line.find("](") {
            let before = &line[..link_start];
            if before.contains('[') {
                let after = &line[link_start + 2..];
                if !after.contains(')') {
                    diagnostics.push(DocDiagnostic {
                        severity: "error".to_string(),
                        message: format!("unclosed link destination on line {line_no}; expected `)`"),
                        code: Some("markdown-syntax".to_string()),
                        line: line_no,
                        col: (link_start + 3) as u32,
                        source: None,
                        end: None,
                        note: None,
                    });
                }
            }
        }
    }

    if let Some((c, count, open_line)) = current_fence {
        diagnostics.push(DocDiagnostic {
            severity: "error".to_string(),
            message: format!(
                "unclosed code fence: opened at line {open_line} with `{}`, reaching end of file without closing fence",
                c.to_string().repeat(count)
            ),
            code: Some("markdown-syntax".to_string()),
            line: open_line,
            col: 1,
            source: None,
            end: None,
            note: None,
        });
    }

    DiagnosticsReport {
        file: shown.to_string(),
        errors: diagnostics.iter().filter(|d| d.severity == "error").count(),
        warnings: diagnostics.iter().filter(|d| d.severity == "warning").count(),
        items: diagnostics,
        preexisting: Vec::new(),
        in_derive: Vec::new(),
        auto_trait: Vec::new(),
        hallucinations: Vec::new(),
    }
}

fn validate_yaml_frontmatter(lines: &[&str], diagnostics: &mut Vec<DocDiagnostic>) {
    let mut seen_top_keys = std::collections::HashSet::new();

    for (idx, line) in lines.iter().enumerate() {
        let line_no = (idx + 2) as u32;

        if line.starts_with('\t') || line.contains(":\t") {
            diagnostics.push(DocDiagnostic {
                severity: "error".to_string(),
                message: "YAML frontmatter forbids tab characters for indentation; use spaces".to_string(),
                code: Some("markdown-frontmatter".to_string()),
                line: line_no,
                col: 1,
                source: None,
                end: None,
                note: None,
            });
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if !line.starts_with(' ') && !trimmed.starts_with('-') {
            if let Some((key, _)) = trimmed.split_once(':') {
                let k = key.trim();
                if !k.is_empty()
                    && !seen_top_keys.insert(k.to_string()) {
                        diagnostics.push(DocDiagnostic {
                            severity: "error".to_string(),
                            message: format!("duplicate frontmatter key `{k}`"),
                            code: Some("markdown-frontmatter".to_string()),
                            line: line_no,
                            col: 1,
                            source: None,
                            end: None,
                            note: None,
                        });
                    }
            } else {
                diagnostics.push(DocDiagnostic {
                    severity: "error".to_string(),
                    message: format!(
                        "invalid frontmatter line: expected `key: value` or list item, got `{trimmed}`"
                    ),
                    code: Some("markdown-frontmatter".to_string()),
                    line: line_no,
                    col: 1,
                    source: None,
                    end: None,
                    note: None,
                });
            }
        }

        if !trimmed.ends_with('|') && !trimmed.ends_with('>') {
            let mut double_quotes = 0;
            let mut single_quotes = 0;
            let mut chars = trimmed.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    chars.next();
                } else if c == '"' {
                    double_quotes += 1;
                } else if c == '\'' {
                    single_quotes += 1;
                }
            }
            if double_quotes % 2 != 0 {
                diagnostics.push(DocDiagnostic {
                    severity: "error".to_string(),
                    message: "unclosed double quote `\"` in frontmatter value".to_string(),
                    code: Some("markdown-frontmatter".to_string()),
                    line: line_no,
                    col: 1,
                    source: None,
                    end: None,
                    note: None,
                });
            } else if single_quotes % 2 != 0 {
                diagnostics.push(DocDiagnostic {
                    severity: "error".to_string(),
                    message: "unclosed single quote `'` in frontmatter value".to_string(),
                    code: Some("markdown-frontmatter".to_string()),
                    line: line_no,
                    col: 1,
                    source: None,
                    end: None,
                    note: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_markdown_with_frontmatter_passes() {
        let md = r#"---
title: "Valid Post"
pubDate: 2026-10-02
---

# Title

```rust
fn main() {}
```
"#;
        let report = validate_markdown("post.md", md);
        assert_eq!(report.errors, 0, "{:?}", report.items);
        assert_eq!(report.warnings, 0);
    }

    #[test]
    fn unclosed_frontmatter_reports_error() {
        let md = r#"---
title: "Unclosed"
# No closing delimiter
"#;
        let report = validate_markdown("post.md", md);
        assert_eq!(report.errors, 1);
        assert!(report.items[0].message.contains("unclosed YAML frontmatter"));
    }

    #[test]
    fn unclosed_code_fence_reports_error() {
        let md = r#"# Title

```rust
fn main() {}
"#;
        let report = validate_markdown("post.md", md);
        assert_eq!(report.errors, 1);
        assert!(report.items[0].message.contains("unclosed code fence"));
    }
}
