/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::line_analyzer::{ParsedLine, is_control_statement, parse_line_def_use};
use super::parser::{extract_function_parameters, strip_literals_and_comments};
use super::types::{DataFlowSlice, SliceCompleteness};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

mod render;

use self::render::{SliceRenderInput, format_slice_statements};

/// Slices statements within a function body backwards from a target criterion.
pub fn slice_intra_function(
    source_text: &str,
    start_line: u32,
    end_line: u32,
    function_name: &str,
    file_rel: &str,
    target_line_opt: Option<u32>,
    target_var_opt: Option<&str>,
) -> DataFlowSlice {
    let all_lines: Vec<&str> = source_text.lines().collect();
    let total_file_lines = all_lines.len();

    let fn_start_idx = (start_line.saturating_sub(1) as usize).min(total_file_lines);
    let fn_end_idx = (end_line as usize).min(total_file_lines);

    if fn_start_idx >= fn_end_idx {
        return DataFlowSlice {
            function_name: function_name.to_string(),
            file: file_rel.to_string(),
            start_line,
            end_line,
            target_line: start_line,
            target_var: target_var_opt.map(str::to_string),
            total_lines: 0,
            retained_lines: 0,
            reduction_percent: 0.0,
            completeness: SliceCompleteness::Incomplete {
                gap: "function line range is empty".to_string(),
            },
            statements: Vec::new(),
            formatted_slice: String::new(),
        };
    }

    // Step 1: Parse function lines and build block/control structure.
    let mut parsed_lines: Vec<ParsedLine> = Vec::new();
    let is_braced_language = matches!(
        Path::new(file_rel)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some(
            "rs" | "go"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "mjs"
                | "c"
                | "h"
                | "cc"
                | "cpp"
                | "hpp"
                | "cxx"
                | "java"
                | "kt"
                | "kts"
                | "cs"
                | "scala"
                | "swift"
        )
    );

    // Control stacks:
    // (line_number, block_indent_or_brace_depth, is_if_statement)
    let mut control_stack: Vec<(u32, usize, bool)> = Vec::new();
    let mut last_if_line: Option<u32> = None;

    for (idx, line_str) in all_lines[fn_start_idx..fn_end_idx].iter().enumerate() {
        let line_num = start_line + idx as u32;
        let indent = line_str
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect::<String>();
        let indent_level = indent.len();
        let trimmed = line_str.trim().to_string();
        let clean_code = strip_literals_and_comments(&trimmed);

        let is_ctrl = is_control_statement(&trimmed);
        let (defined_vars, used_vars, is_assignment, is_mutation) =
            parse_line_def_use(&trimmed, &clean_code);

        // Control stack resolution:
        if is_braced_language && trimmed.starts_with('}') && !control_stack.is_empty() {
            if let Some((l, _, true)) = control_stack.pop() {
                last_if_line = Some(l);
            }
        } else if !is_braced_language {
            // Indentation-based control scoping (e.g. Python)
            while let Some(&(_, stack_indent, is_if)) = control_stack.last() {
                if !trimmed.is_empty() && indent_level <= stack_indent {
                    let popped = control_stack.pop();
                    if is_if && popped.is_some() {
                        last_if_line = popped.map(|(l, _, _)| l);
                    }
                } else {
                    break;
                }
            }
        }

        // Determine control parent:
        let is_else = trimmed.starts_with("else")
            || trimmed.starts_with("} else")
            || trimmed.starts_with("elif");

        let control_parent = if is_else && last_if_line.is_some() {
            // Statements on or in an else/elif branch are control-dependent on the preceding if!
            last_if_line
        } else {
            control_stack.last().map(|(l, _, _)| *l)
        };

        let is_if = trimmed.starts_with("if ") || trimmed.starts_with("if(");
        if is_if {
            last_if_line = Some(line_num);
        }

        parsed_lines.push(ParsedLine {
            line: line_num,
            raw_text: line_str.to_string(),
            indent,
            indent_level,
            trimmed: trimmed.clone(),
            clean_code,
            defined_vars,
            used_vars,
            is_control: is_ctrl,
            is_assignment,
            is_mutation,
            control_parent,
        });

        // Push new block to control stack
        if is_braced_language {
            if (is_ctrl && trimmed.contains('{')) || trimmed.ends_with('{') {
                control_stack.push((line_num, trimmed.len(), is_if));
            }
        } else if is_ctrl && trimmed.ends_with(':') {
            control_stack.push((line_num, indent_level, is_if));
        }
    }

    // Step 2: Determine Slicing Criterion.
    let target_line = target_line_opt.unwrap_or_else(|| {
        for pl in parsed_lines.iter().rev() {
            if pl.trimmed.starts_with("return ") || pl.trimmed.starts_with("return;") {
                return pl.line;
            }
        }
        if parsed_lines.len() > 1 {
            let last_idx = parsed_lines.len() - 1;
            if parsed_lines[last_idx].trimmed == "}" {
                return parsed_lines[last_idx.saturating_sub(1)].line;
            }
        }
        end_line
    });

    let mut needed_vars: HashSet<String> = HashSet::new();
    if let Some(var) = target_var_opt {
        needed_vars.insert(var.to_string());
    }

    let target_idx = parsed_lines
        .iter()
        .position(|p| p.line == target_line)
        .unwrap_or(parsed_lines.len().saturating_sub(1));

    if needed_vars.is_empty() {
        let target_pl = &parsed_lines[target_idx];
        for u in &target_pl.used_vars {
            needed_vars.insert(u.clone());
        }
        for d in &target_pl.defined_vars {
            needed_vars.insert(d.clone());
        }
    }

    // Step 3: Backward traversal for Data Flow and Control Flow dependencies.
    let mut retained_indices: BTreeSet<usize> = BTreeSet::new();
    let mut statement_reasons: BTreeMap<usize, String> = BTreeMap::new();
    let mut pending_control_lines: HashSet<u32> = HashSet::new();

    // Always retain the function signature line (index 0)
    retained_indices.insert(0);
    statement_reasons.insert(0, "Function signature & input parameters".to_string());

    // Retain the target criterion line
    retained_indices.insert(target_idx);
    let target_var_label = if let Some(v) = target_var_opt {
        format!("var `{v}` on line {target_line}")
    } else {
        format!("criterion at line {target_line}")
    };
    statement_reasons.insert(target_idx, format!("Target criterion: {target_var_label}"));

    if let Some(ctrl_parent) = parsed_lines[target_idx].control_parent {
        pending_control_lines.insert(ctrl_parent);
    }

    // Iterate backwards from target_idx down to 1
    for i in (1..=target_idx).rev() {
        let pl = &parsed_lines[i];

        let is_pending_control = pending_control_lines.contains(&pl.line);
        let defines_needed = pl.defined_vars.iter().any(|d| needed_vars.contains(d));

        if is_pending_control || defines_needed {
            retained_indices.insert(i);

            let matching_vars: Vec<String> = pl
                .defined_vars
                .iter()
                .filter(|d| needed_vars.contains(*d))
                .cloned()
                .collect();

            if is_pending_control {
                statement_reasons.entry(i).or_insert_with(|| {
                    "Control dependency: guards execution of dependent statements".to_string()
                });
                pending_control_lines.remove(&pl.line);
            }

            if defines_needed {
                let reason_label = if pl.is_mutation {
                    format!("Data dependency: mutates `{}`", matching_vars.join(", "))
                } else {
                    format!("Data dependency: defines `{}`", matching_vars.join(", "))
                };
                statement_reasons.entry(i).or_insert(reason_label);

                // A definition satisfies and kills a needed variable if it occurs at the root
                // scope or is a loop header definition (e.g. `for &x in items`):
                let is_root_or_loop = pl.control_parent.is_none()
                    || pl.control_parent == Some(start_line)
                    || pl.trimmed.starts_with("for ")
                    || pl.trimmed.starts_with("for(");

                if is_root_or_loop && !pl.is_mutation {
                    for d in &matching_vars {
                        needed_vars.remove(d);
                    }
                }
            }

            for u in &pl.used_vars {
                needed_vars.insert(u.clone());
            }

            if let Some(outer) = pl.control_parent {
                pending_control_lines.insert(outer);
            }
        }
    }

    // If an if statement was retained, also retain its paired else / elif lines if statements inside them were retained
    for i in 1..parsed_lines.len() {
        let pl = &parsed_lines[i];
        if (pl.trimmed.starts_with("} else")
            || pl.trimmed.starts_with("else")
            || pl.trimmed.starts_with("elif"))
            && pl.control_parent.is_some_and(|cp| {
                retained_indices
                    .iter()
                    .any(|&ri| parsed_lines[ri].line == cp)
            })
        {
            // Check if any statement inside this else branch was retained
            let branch_retained = (i + 1..parsed_lines.len())
                .take_while(|&k| parsed_lines[k].control_parent == pl.control_parent)
                .any(|k| retained_indices.contains(&k));
            if branch_retained {
                retained_indices.insert(i);
                statement_reasons
                    .entry(i)
                    .or_insert_with(|| "Control branch: alternate execution path".to_string());
            }
        }
    }

    // Always retain the closing brace of the function if present
    if parsed_lines.last().map(|p| p.trimmed.as_str()) == Some("}") {
        let last_idx = parsed_lines.len() - 1;
        retained_indices.insert(last_idx);
        statement_reasons.insert(last_idx, "Function closing delimiter".to_string());
    }

    // Step 4: Check completeness contract.
    let sig_params = extract_function_parameters(&parsed_lines[0].clean_code);

    let mut unresolved_vars: Vec<String> = Vec::new();
    for v in &needed_vars {
        if !sig_params.contains(v) {
            unresolved_vars.push(v.clone());
        }
    }

    let completeness = if unresolved_vars.is_empty() {
        SliceCompleteness::Complete
    } else {
        SliceCompleteness::Incomplete {
            gap: format!(
                "unresolved identifier(s) `{}` not defined in function parameters or local scope",
                unresolved_vars.join(", ")
            ),
        }
    };

    // Step 5: Build final statements and formatted slice.
    let (slice_statements, formatted, total_lines, retained_count, reduction_percent) =
        format_slice_statements(SliceRenderInput {
            function_name,
            file_rel,
            start_line,
            end_line,
            target_line,
            parsed_lines,
            retained_indices,
            statement_reasons,
            completeness: &completeness,
        });

    DataFlowSlice {
        function_name: function_name.to_string(),
        file: file_rel.to_string(),
        start_line,
        end_line,
        target_line,
        target_var: target_var_opt.map(str::to_string),
        total_lines,
        retained_lines: retained_count,
        reduction_percent,
        completeness,
        statements: slice_statements,
        formatted_slice: formatted,
    }
}
