/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::parser::extract_used_variables;

/// Internal representation of a parsed line in the function.
#[derive(Debug, Clone)]
pub(crate) struct ParsedLine {
    pub(crate) line: u32,
    pub(crate) raw_text: String,
    pub(crate) indent: String,
    #[allow(dead_code)]
    pub(crate) indent_level: usize,
    pub(crate) trimmed: String,
    pub(crate) clean_code: String,
    pub(crate) defined_vars: Vec<String>,
    pub(crate) used_vars: Vec<String>,
    pub(crate) is_control: bool,
    #[allow(dead_code)]
    pub(crate) is_assignment: bool,
    pub(crate) is_mutation: bool,
    pub(crate) control_parent: Option<u32>,
}

/// Checks if a trimmed line is a control flow statement.
pub(crate) fn is_control_statement(trimmed: &str) -> bool {
    let prefixes = [
        "if ", "if(", "else if ", "else if(", "elif ", "elif(", "else", "else{", "else {", "for ",
        "for(", "while ", "while(", "loop {", "loop{", "match ", "match(", "switch ", "switch(",
        "case ", "guard ",
    ];
    prefixes.iter().any(|p| trimmed.starts_with(p))
}

/// Parses defined variables and used variables from a single line.
pub(crate) fn parse_line_def_use(
    trimmed: &str,
    clean_code: &str,
) -> (Vec<String>, Vec<String>, bool, bool) {
    let mut defined = Vec::new();
    let mut used = Vec::new();
    let mut is_assignment = false;
    let mut is_mutation = false;

    // For loop patterns across languages:
    // Rust: `for &x in items {`, `for (i, x) in ...`
    // Python: `for x in items:`
    // TS/JS: `for (const x of items)` / `for (let x of items)`
    // Go: `for i, v := range items`
    if trimmed.starts_with("for ") || trimmed.starts_with("for(") {
        if let Some(in_idx) = clean_code.find(" in ") {
            let header = &clean_code[..in_idx];
            let after_in = &clean_code[in_idx + 4..];
            let var_part = header
                .trim_start_matches("for")
                .trim()
                .trim_start_matches('(')
                .trim_start_matches("let ")
                .trim_start_matches("const ")
                .trim_start_matches("var ")
                .trim_start_matches('&');
            defined.extend(extract_used_variables(var_part));
            used.extend(extract_used_variables(after_in));
            return (defined, used, true, false);
        } else if let Some(of_idx) = clean_code.find(" of ") {
            let header = &clean_code[..of_idx];
            let after_of = &clean_code[of_idx + 4..];
            let var_part = header
                .trim_start_matches("for")
                .trim()
                .trim_start_matches('(')
                .trim_start_matches("let ")
                .trim_start_matches("const ");
            defined.extend(extract_used_variables(var_part));
            used.extend(extract_used_variables(after_of));
            return (defined, used, true, false);
        } else if let Some(range_idx) = clean_code.find("range ") {
            let header = &clean_code[..range_idx];
            let after_range = &clean_code[range_idx + 6..];
            let var_part = header
                .trim_start_matches("for")
                .trim()
                .trim_end_matches(":=")
                .trim();
            defined.extend(extract_used_variables(var_part));
            used.extend(extract_used_variables(after_range));
            return (defined, used, true, false);
        }
    }

    // Detect mutation patterns: x += ..., x -= ..., x.push(...), etc.
    let compound_ops = ["+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>="];
    for op in compound_ops {
        if let Some(idx) = clean_code.find(op) {
            let lhs = clean_code[..idx].trim();
            let rhs = clean_code[idx + op.len()..].trim();
            let lhs_ids = extract_used_variables(lhs);
            if let Some(target_var) = lhs_ids.first() {
                defined.push(target_var.clone());
                used.push(target_var.clone());
                is_mutation = true;
            }
            used.extend(extract_used_variables(rhs));
            return (defined, used, true, is_mutation);
        }
    }

    // Detect mutating calls: e.g. `items.push(val)`, `map.insert(k, v)`, `list.append(x)`
    if let Some(dot_idx) = clean_code.find('.') {
        let lhs = clean_code[..dot_idx].trim();
        let rest = &clean_code[dot_idx + 1..];
        let mutating_methods = [
            "push(", "insert(", "append(", "extend(", "update(", "remove(", "clear(", "add(",
            "set(", "put(", "pop(", "sort(",
        ];
        if mutating_methods.iter().any(|m| rest.starts_with(m)) {
            let lhs_ids = extract_used_variables(lhs);
            if let Some(var) = lhs_ids.last() {
                defined.push(var.clone());
                used.push(var.clone());
                is_mutation = true;
                used.extend(extract_used_variables(rest));
                return (defined, used, true, is_mutation);
            }
        }
    }

    // Go short assignment: a, b := rhs
    if let Some(idx) = clean_code.find(":=") {
        let lhs = clean_code[..idx].trim();
        let rhs = clean_code[idx + 2..].trim();
        defined.extend(extract_used_variables(lhs));
        used.extend(extract_used_variables(rhs));
        return (defined, used, true, false);
    }

    // Explicit declaration with assignment:
    // Rust: `let [mut] var [: Type] = rhs;`
    // TS/JS: `const|let|var var [: Type] = rhs;`
    // Swift: `let|var var [: Type] = rhs;`
    let decl_prefixes = ["let mut ", "let ", "const ", "var "];
    for p in decl_prefixes {
        if let Some(stripped) = trimmed.strip_prefix(p) {
            if let Some(eq_idx) = stripped.find('=') {
                let lhs = stripped[..eq_idx].trim();
                let rhs = stripped[eq_idx + 1..].trim();
                let var_part = if let Some(colon) = lhs.find(':') {
                    &lhs[..colon]
                } else {
                    lhs
                };
                defined.extend(extract_used_variables(var_part));
                used.extend(extract_used_variables(rhs));
                return (defined, used, true, false);
            } else {
                defined.extend(extract_used_variables(stripped));
                return (defined, used, true, false);
            }
        }
    }

    // Plain assignment: `lhs = rhs` (where `=` is not `==`, `<=`, `>=`, `!=`)
    if let Some(eq_idx) = clean_code.find('=') {
        let before_eq = &clean_code[..eq_idx];
        let after_eq = &clean_code[eq_idx + 1..];
        let is_comparison = before_eq.ends_with('!')
            || before_eq.ends_with('<')
            || before_eq.ends_with('>')
            || after_eq.starts_with('=');
        if !is_comparison && !trimmed.starts_with("if ") && !trimmed.starts_with("while ") {
            let lhs = before_eq.trim();
            let rhs = after_eq.trim();
            let lhs_ids = extract_used_variables(lhs);
            if let Some(var) = lhs_ids.first() {
                defined.push(var.clone());
                is_assignment = true;
                if lhs.contains('[') {
                    used.extend(lhs_ids);
                    is_mutation = true;
                }
                used.extend(extract_used_variables(rhs));
                return (defined, used, is_assignment, is_mutation);
            }
        }
    }

    // Default: no assignment detected; all identifiers are used.
    used.extend(extract_used_variables(clean_code));
    (defined, used, is_assignment, is_mutation)
}
