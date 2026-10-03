//! Type-directed expression synthesis (roadmap 9.2).
//!
//! Synthesizes valid in-scope expressions and 1-2 hop accessor chains
//! that produce a requested target type (e.g. `AccountId`, `String`, `Option<T>`),
//! preventing LLM agents from hallucinating method chains or parameter names.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProposedExpression {
    pub expression: String,
    pub source_variable: String,
    pub confidence: u32, // 1-100
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExpressionSynthesisReport {
    pub target_type: String,
    pub location: String,
    pub candidate_count: usize,
    pub candidates: Vec<ProposedExpression>,
}

/// Proposes expressions in the current scope that evaluate to `target_type`.
pub fn propose_expressions_in_scope(
    workspace_root: &Path,
    file_rel_path: &str,
    target_line: u32,
    target_type: &str,
) -> Result<ExpressionSynthesisReport> {
    let abs_path = if Path::new(file_rel_path).is_absolute() {
        Path::new(file_rel_path).to_path_buf()
    } else {
        workspace_root.join(file_rel_path)
    };

    let content = std::fs::read_to_string(&abs_path)
        .with_context(|| format!("Failed to read file {}", abs_path.display()))?;

    let lines: Vec<&str> = content.lines().collect();
    let current_line_idx = (target_line.saturating_sub(1)) as usize;

    // Collect in-scope variables: function parameters and prior `let` bindings
    let in_scope_vars = extract_in_scope_variables(&lines, current_line_idx);

    let clean_target = target_type.trim();
    let mut candidates = Vec::new();

    // 1. Direct type matches
    for (var_name, var_type) in &in_scope_vars {
        if types_match(var_type, clean_target) {
            candidates.push(ProposedExpression {
                expression: var_name.clone(),
                source_variable: var_name.clone(),
                confidence: 95,
                rationale: format!("Exact type match: `{var_name}` is declared as `{var_type}`"),
            });
        }
    }

    // 2. Reference / dereference conversions
    for (var_name, var_type) in &in_scope_vars {
        if clean_target.starts_with('&') && clean_target.trim_start_matches('&').trim() == var_type {
            candidates.push(ProposedExpression {
                expression: format!("&{var_name}"),
                source_variable: var_name.clone(),
                confidence: 90,
                rationale: format!("Borrow: references `{var_name}` as `{clean_target}`"),
            });
        }
        if var_type.starts_with('&') && var_type.trim_start_matches('&').trim() == clean_target {
            candidates.push(ProposedExpression {
                expression: format!("*{var_name}"),
                source_variable: var_name.clone(),
                confidence: 85,
                rationale: format!("Dereference: dereferences `{var_name}` to `{clean_target}`"),
            });
            candidates.push(ProposedExpression {
                expression: format!("{var_name}.clone()"),
                source_variable: var_name.clone(),
                confidence: 80,
                rationale: format!("Clone: clones referenced `{var_name}` into owned value"),
            });
        }
    }

    // 3. String conversions
    if clean_target == "String" {
        for (var_name, var_type) in &in_scope_vars {
            if var_type == "&str" || var_type.contains("str") {
                candidates.push(ProposedExpression {
                    expression: format!("{var_name}.to_string()"),
                    source_variable: var_name.clone(),
                    confidence: 90,
                    rationale: format!("Converts string slice `{var_name}` to owned String"),
                });
            } else if var_type.contains("Path") {
                candidates.push(ProposedExpression {
                    expression: format!("{var_name}.display().to_string()"),
                    source_variable: var_name.clone(),
                    confidence: 85,
                    rationale: format!("Formats path `{var_name}` as String"),
                });
            } else if var_type != "String" && (var_type.contains("id") || var_type.contains("Id") || is_primitive(var_type)) {
                candidates.push(ProposedExpression {
                    expression: format!("{var_name}.to_string()"),
                    source_variable: var_name.clone(),
                    confidence: 75,
                    rationale: format!("Formats value `{var_name}` into String"),
                });
            }
        }
    }

    // 4. Option wrapping: Some(x)
    if clean_target.starts_with("Option<") && clean_target.ends_with('>') {
        let inner_type = &clean_target[7..clean_target.len() - 1].trim();
        for (var_name, var_type) in &in_scope_vars {
            if types_match(var_type, inner_type) {
                candidates.push(ProposedExpression {
                    expression: format!("Some({var_name})"),
                    source_variable: var_name.clone(),
                    confidence: 88,
                    rationale: format!("Wraps `{var_name}` into Option::Some"),
                });
            }
        }
        candidates.push(ProposedExpression {
            expression: "None".to_string(),
            source_variable: "None".to_string(),
            confidence: 60,
            rationale: "Empty option variant".to_string(),
        });
    }

    // 5. Result wrapping: Ok(x)
    if clean_target.starts_with("Result<") && clean_target.ends_with('>') {
        let inner = &clean_target[7..clean_target.len() - 1];
        let inner_ok = inner.split(',').next().unwrap_or(inner).trim();
        for (var_name, var_type) in &in_scope_vars {
            if types_match(var_type, inner_ok) {
                candidates.push(ProposedExpression {
                    expression: format!("Ok({var_name})"),
                    source_variable: var_name.clone(),
                    confidence: 88,
                    rationale: format!("Wraps `{var_name}` into Result::Ok"),
                });
            }
        }
    }

    // 6. 1-hop accessor chains based on naming heuristics
    let target_lower = clean_target.to_lowercase();
    for var_name in in_scope_vars.keys() {
        if target_lower.contains("id") || clean_target == "u64" || clean_target == "usize" || clean_target == "String" {
            candidates.push(ProposedExpression {
                expression: format!("{var_name}.id"),
                source_variable: var_name.clone(),
                confidence: 70,
                rationale: format!("Direct field access `{var_name}.id`"),
            });
            candidates.push(ProposedExpression {
                expression: format!("{var_name}.get_id()"),
                source_variable: var_name.clone(),
                confidence: 65,
                rationale: format!("Getter invocation `{var_name}.get_id()`"),
            });
        }
        if clean_target == "bool" {
            candidates.push(ProposedExpression {
                expression: format!("!{var_name}"),
                source_variable: var_name.clone(),
                confidence: 70,
                rationale: format!("Boolean negation `!{var_name}`"),
            });
            candidates.push(ProposedExpression {
                expression: format!("{var_name}.is_empty()"),
                source_variable: var_name.clone(),
                confidence: 65,
                rationale: format!("Collection emptiness check on `{var_name}`"),
            });
        }
    }

    // Deduplicate and rank candidates by confidence
    candidates.sort_by_key(|c| std::cmp::Reverse(c.confidence));
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| seen.insert(c.expression.clone()));

    Ok(ExpressionSynthesisReport {
        target_type: clean_target.to_string(),
        location: format!("{}:{}", file_rel_path, target_line),
        candidate_count: candidates.len(),
        candidates,
    })
}

fn types_match(a: &str, b: &str) -> bool {
    let clean_a = a.trim().trim_start_matches('&').trim();
    let clean_b = b.trim().trim_start_matches('&').trim();
    clean_a == clean_b
}

fn is_primitive(t: &str) -> bool {
    matches!(
        t,
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "f32" | "f64" | "bool"
    )
}

/// Extracts parameters and local let bindings from lines leading up to current line.
fn extract_in_scope_variables(lines: &[&str], current_line_idx: usize) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    let scan_start = current_line_idx.saturating_sub(200);

    // Look backwards to find enclosing function header
    for i in (scan_start..=current_line_idx.min(lines.len().saturating_sub(1))).rev() {
        let line = lines[i].trim();
        if (line.starts_with("fn ") || line.starts_with("pub fn ") || line.starts_with("pub async fn ") || line.starts_with("async fn ") || line.starts_with("def ") || line.starts_with("func "))
            && line.contains('(')
        {
            // Parse parameters across lines until closing paren
            let mut param_text = String::new();
            if let Some(rest) = line.split('(').nth(1) {
                param_text.push_str(rest);
                if !rest.contains(')') {
                    for next_line in &lines[(i + 1)..=current_line_idx.min(lines.len().saturating_sub(1))] {
                        let next_line = next_line.trim();
                        param_text.push(' ');
                        param_text.push_str(next_line);
                        if next_line.contains(')') {
                            break;
                        }
                    }
                }
            }
            if let Some(params) = param_text.split(')').next() {
                for p in params.split(',') {
                    let p_trim = p.trim();
                    if p_trim.contains(':') {
                        let mut parts = p_trim.split(':');
                        let name = parts.next().unwrap_or("").trim().trim_start_matches("mut ");
                        let ty = parts.next().unwrap_or("").trim();
                        if !name.is_empty() && name != "self" && name != "&self" && name != "&mut self" {
                            vars.insert(name.to_string(), ty.to_string());
                        }
                    }
                }
            }
            break;
        }
    }

    // Parse local bindings in the scope up to current_line_idx
    for line in &lines[scan_start..current_line_idx.min(lines.len())] {
        let line = line.trim();
        if line.starts_with("let ") {
            let rest = line.trim_start_matches("let ").trim_start_matches("mut ");
            if let Some(name_part) = rest.split(['=', ':']).next() {
                let var_name = name_part.trim();
                let ty = if line.contains(':') && line.find(':') < line.find('=') {
                    line.split(':').nth(1).and_then(|s| s.split('=').next()).unwrap_or("").trim()
                } else if line.contains("String::new") || line.contains(".to_string()") {
                    "String"
                } else if line.contains("Vec::new") || line.contains("vec![") {
                    "Vec"
                } else if line.contains("true") || line.contains("false") {
                    "bool"
                } else {
                    "var"
                };

                if !var_name.is_empty() && !var_name.contains(' ') {
                    vars.insert(var_name.to_string(), ty.to_string());
                }
            }
        }
    }

    vars
}

/// Formats the synthesis report into a clean readable output.
pub fn format_expression_synthesis_report(report: &ExpressionSynthesisReport) -> String {
    let mut out = String::new();
    out.push_str("⚡ prod-code Type-Directed Expression Synthesis\n");
    out.push_str("────────────────────────────────────────────────────\n");
    out.push_str(&format!(
        "Target Type: `{}` at {}\nCandidates Found: {}\n\n",
        report.target_type, report.location, report.candidate_count
    ));

    if report.candidates.is_empty() {
        out.push_str("No in-scope expressions found producing target type.\n");
        return out;
    }

    out.push_str("Ranked Candidate Expressions:\n");
    for (i, cand) in report.candidates.iter().enumerate() {
        out.push_str(&format!(
            "  {}. `{}` (Confidence: {}%)\n     └─ {}\n",
            i + 1,
            cand.expression,
            cand.confidence,
            cand.rationale
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_expression_synthesis() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("main.rs");
        let code = r#"
fn test_handler(user_id: u64, name: &str, is_admin: bool) {
    let extra_token = "secret";
    // Target line:
    let output = 42;
}
"#;
        std::fs::write(&file, code).unwrap();

        let report = propose_expressions_in_scope(dir.path(), "main.rs", 5, "u64").unwrap();
        assert!(!report.candidates.is_empty());
        assert_eq!(report.candidates[0].expression, "user_id");

        let report_str = propose_expressions_in_scope(dir.path(), "main.rs", 5, "String").unwrap();
        let has_to_string = report_str.candidates.iter().any(|c| c.expression == "name.to_string()");
        assert!(has_to_string);
    }
}
