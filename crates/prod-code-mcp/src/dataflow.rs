//! Intra-function backward data-flow and control-dependency program slicing (Roadmap 7.3).
//!
//! Given a function body and a slicing criterion `(target_line, target_var)`, this module
//! computes the minimal set of statements that affect the target value by traversing
//! data dependencies (def-use chains) and control dependencies (conditional branches and loops)
//! backwards within the function.
//!
//! Produces an explicit completeness contract (`Complete`, `Bounded`, or `Incomplete`)
//! so callers can distinguish mathematically verified dependency closures from partial cuts.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

/// Completeness contract for intra-function data-flow slicing (Roadmap 7.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SliceCompleteness {
    /// Full data-flow and control-dependency graph resolved with complete evidence.
    Complete,
    /// Bounded by explicit line budget or statement cutoff.
    Bounded {
        max_statements: usize,
        actual_statements: usize,
    },
    /// Incomplete analysis due to unresolvable dynamic scoping, missing targets, or unparseable syntax.
    Incomplete { gap: String },
}

impl SliceCompleteness {
    pub fn is_complete(&self) -> bool {
        matches!(self, SliceCompleteness::Complete)
    }

    pub fn label(&self) -> &'static str {
        match self {
            SliceCompleteness::Complete => "COMPLETE",
            SliceCompleteness::Bounded { .. } => "BOUNDED",
            SliceCompleteness::Incomplete { .. } => "INCOMPLETE",
        }
    }
}

/// One statement in the intra-function slice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceStatement {
    /// 1-based line number in original file.
    pub line: u32,
    /// Exact trimmed source line.
    pub text: String,
    /// Leading indentation string for formatted reconstruction.
    pub indent: String,
    /// Reason this statement is included in the slice.
    pub reason: String,
    /// Whether this is a control dependency (e.g. if/while/match condition).
    pub is_control: bool,
    /// Variables defined / mutated by this statement.
    pub defined_vars: Vec<String>,
    /// Variables used / referenced by this statement.
    pub used_vars: Vec<String>,
}

/// Result of intra-function data-flow slicing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataFlowSlice {
    pub function_name: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub target_line: u32,
    pub target_var: Option<String>,
    pub total_lines: usize,
    pub retained_lines: usize,
    pub reduction_percent: f64,
    pub completeness: SliceCompleteness,
    pub statements: Vec<SliceStatement>,
    pub formatted_slice: String,
}

/// Known keywords across common languages (Rust, Go, Python, TS/JS, C/C++, Swift)
/// that are not variable identifiers.
const KEYWORDS: &[&str] = &[
    // Rust
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
    "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true",
    "type", "unsafe", "use", "where", "while", "yield",
    // Types & builtins
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
    "f32", "f64", "bool", "char", "str", "String", "Vec", "Option", "Result", "Some", "None",
    "Ok", "Err",
    // Go
    "chan", "defer", "go", "goto", "import", "interface", "map", "package", "range", "select",
    "switch", "case", "default", "nil", "int", "int8", "int16", "int32", "int64", "uint",
    "uint8", "uint16", "uint32", "uint64", "uintptr", "float32", "float64", "complex64",
    "complex128", "byte", "rune", "string", "error", "make", "new", "len", "cap", "append",
    "copy", "close", "delete", "panic", "recover", "print", "println",
    // Python
    "and", "assert", "class", "def", "del", "elif", "except", "finally", "from", "global",
    "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "try", "with", "True", "False",
    "None",
    // TS / JS
    "catch", "debugger", "delete", "do", "export", "finally", "function", "instanceof", "new",
    "switch", "this", "throw", "typeof", "var", "void", "null", "undefined", "NaN", "Infinity",
    // Common C/C++/Swift
    "auto", "catch", "class", "guard", "func", "var", "throw", "throws", "try", "catch",
    "override", "private", "public", "internal", "fileprivate",
];

/// Strips comments and string literals from a line of code to allow safe identifier scanning.
fn strip_literals_and_comments(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        // Line comments: // or # or --
        if (chars[i] == '/' && i + 1 < n && chars[i + 1] == '/')
            || (chars[i] == '-' && i + 1 < n && chars[i + 1] == '-')
            || (chars[i] == '#' && (i == 0 || chars[i - 1].is_whitespace()))
        {
            break;
        }

        // String literals: "...", '...', `...`
        if chars[i] == '"' || chars[i] == '\'' || chars[i] == '`' {
            let quote = chars[i];
            i += 1;
            while i < n {
                if chars[i] == '\\' && i + 1 < n {
                    i += 2;
                } else if chars[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            out.push(' ');
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Extracts identifier tokens from text, skipping property and method member accesses (e.g. `.price`).
fn extract_used_variables(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();

            // Check if this word is preceded by '.' (property access like `obj.prop` or `obj.method()`)
            let mut j = start;
            let mut is_property = false;
            while j > 0 {
                j -= 1;
                if chars[j] == '.' {
                    // Check if it's '..' range in Rust; '..' is not property access
                    is_property = !(j > 0 && chars[j - 1] == '.');
                    break;
                } else if !chars[j].is_whitespace() {
                    break;
                }
            }

            if !is_property && !KEYWORDS.contains(&word.as_str()) && !word.chars().all(|c| c.is_ascii_digit()) {
                ids.push(word);
            }
        } else {
            i += 1;
        }
    }
    ids
}

/// Extracts parameter names from a function signature line across common languages.
fn extract_function_parameters(signature: &str) -> Vec<String> {
    let mut params = Vec::new();
    let Some(open_paren) = signature.find('(') else {
        return extract_used_variables(signature);
    };
    let Some(close_paren) = signature.rfind(')') else {
        return extract_used_variables(signature);
    };
    if open_paren >= close_paren {
        return extract_used_variables(signature);
    }
    let param_section = &signature[open_paren + 1..close_paren];
    for raw_part in param_section.split(',') {
        let part = raw_part.trim();
        if part.is_empty() || part == "self" || part == "&self" || part == "&mut self" {
            continue;
        }
        // Extract the parameter name
        // Rust / TS: `name: Type`
        let param_name = if let Some(colon) = part.find(':') {
            part[..colon].trim()
        } else {
            // Go: `name Type` or Python: `name`
            part.split_whitespace().next().unwrap_or(part)
        };
        // Clean leading modifiers like mut, &, etc.
        let cleaned = param_name.trim_start_matches("mut ").trim_start_matches('&').trim();
        for id in extract_used_variables(cleaned) {
            params.push(id);
        }
    }
    params
}

/// Internal representation of a parsed line in the function.
#[derive(Debug, Clone)]
struct ParsedLine {
    line: u32,
    raw_text: String,
    indent: String,
    #[allow(dead_code)]
    indent_level: usize,
    trimmed: String,
    clean_code: String,
    defined_vars: Vec<String>,
    used_vars: Vec<String>,
    is_control: bool,
    #[allow(dead_code)]
    is_assignment: bool,
    is_mutation: bool,
    control_parent: Option<u32>,
}

/// Checks if a trimmed line is a control flow statement.
fn is_control_statement(trimmed: &str) -> bool {
    let prefixes = [
        "if ", "if(", "else if ", "else if(", "elif ", "elif(", "else", "else{", "else {",
        "for ", "for(", "while ", "while(", "loop {", "loop{", "match ", "match(",
        "switch ", "switch(", "case ", "guard ",
    ];
    prefixes.iter().any(|p| trimmed.starts_with(p))
}

/// Parses defined variables and used variables from a single line.
fn parse_line_def_use(trimmed: &str, clean_code: &str) -> (Vec<String>, Vec<String>, bool, bool) {
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
            let var_part = header.trim_start_matches("for").trim().trim_start_matches('(').trim_start_matches("let ").trim_start_matches("const ").trim_start_matches("var ").trim_start_matches('&');
            defined.extend(extract_used_variables(var_part));
            used.extend(extract_used_variables(after_in));
            return (defined, used, true, false);
        } else if let Some(of_idx) = clean_code.find(" of ") {
            let header = &clean_code[..of_idx];
            let after_of = &clean_code[of_idx + 4..];
            let var_part = header.trim_start_matches("for").trim().trim_start_matches('(').trim_start_matches("let ").trim_start_matches("const ");
            defined.extend(extract_used_variables(var_part));
            used.extend(extract_used_variables(after_of));
            return (defined, used, true, false);
        } else if let Some(range_idx) = clean_code.find("range ") {
            let header = &clean_code[..range_idx];
            let after_range = &clean_code[range_idx + 6..];
            let var_part = header.trim_start_matches("for").trim().trim_end_matches(":=").trim();
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
            "push(", "insert(", "append(", "extend(", "update(", "remove(", "clear(",
            "add(", "set(", "put(", "pop(", "sort(",
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
        Path::new(file_rel).extension().and_then(|extension| extension.to_str()),
        Some(
            "rs" | "go" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "c" | "h" | "cc"
                | "cpp" | "hpp" | "cxx" | "java" | "kt" | "kts" | "cs" | "scala" | "swift"
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
                statement_reasons
                    .entry(i)
                    .or_insert_with(|| "Control dependency: guards execution of dependent statements".to_string());
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
        if (pl.trimmed.starts_with("} else") || pl.trimmed.starts_with("else") || pl.trimmed.starts_with("elif"))
            && pl.control_parent.is_some_and(|cp| retained_indices.iter().any(|&ri| parsed_lines[ri].line == cp))
        {
            // Check if any statement inside this else branch was retained
            let branch_retained = (i + 1..parsed_lines.len())
                .take_while(|&k| parsed_lines[k].control_parent == pl.control_parent)
                .any(|k| retained_indices.contains(&k));
            if branch_retained {
                retained_indices.insert(i);
                statement_reasons.entry(i).or_insert_with(|| "Control branch: alternate execution path".to_string());
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
    let mut slice_statements: Vec<SliceStatement> = Vec::new();
    let mut formatted = String::new();

    let total_lines = parsed_lines.len();
    let retained_count = retained_indices.len();
    let reduction_percent = if total_lines > 0 {
        100.0 - (retained_count as f64 * 100.0 / total_lines as f64)
    } else {
        0.0
    };

    formatted.push_str(&format!(
        "// === INTRA-FUNCTION DATA-FLOW SLICE: `{}` ({}:{}-{}) ===\n",
        function_name, file_rel, start_line, end_line
    ));
    formatted.push_str(&format!(
        "// Completeness: {} | Target: line {} | Retained: {}/{} lines ({:.0}% reduction)\n",
        completeness.label(),
        target_line,
        retained_count,
        total_lines,
        reduction_percent
    ));
    if let SliceCompleteness::Incomplete { ref gap } = completeness {
        formatted.push_str(&format!("// Missing evidence gap: {gap}\n"));
    }
    formatted.push_str("// ============================================================================\n");

    let mut prev_idx: Option<usize> = None;

    for &idx in &retained_indices {
        let pl = &parsed_lines[idx];

        if let Some(prev) = prev_idx {
            let omitted = idx.saturating_sub(prev + 1);
            if omitted > 0 {
                formatted.push_str(&format!(
                    "{}    // ... [sliced away {} statement(s) not affecting target] ...\n",
                    pl.indent, omitted
                ));
            }
        }
        prev_idx = Some(idx);

        let reason = statement_reasons
            .get(&idx)
            .cloned()
            .unwrap_or_else(|| "Retained statement".to_string());

        slice_statements.push(SliceStatement {
            line: pl.line,
            text: pl.trimmed.clone(),
            indent: pl.indent.clone(),
            reason: reason.clone(),
            is_control: pl.is_control,
            defined_vars: pl.defined_vars.clone(),
            used_vars: pl.used_vars.clone(),
        });

        formatted.push_str(&format!(
            "{:<4} | {}    // [{}]\n",
            pl.line, pl.raw_text, reason
        ));
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intra_function_linear_arithmetic() {
        let code = r#"fn calculate(a: i32, b: i32) -> i32 {
    let x = a + 1;
    let unused_val = 100;
    let y = x * 2;
    let unrelated = unused_val + 10;
    println!("log: {}", unrelated);
    y
}"#;
        let slice = slice_intra_function(code, 1, 8, "calculate", "src/calc.rs", Some(7), Some("y"));
        assert!(slice.completeness.is_complete());
        assert_eq!(slice.retained_lines, 5); // fn sig, let x, let y, y, }
        let retained_text = slice.formatted_slice;
        assert!(retained_text.contains("let x = a + 1;"));
        assert!(retained_text.contains("let y = x * 2;"));
        assert!(!retained_text.contains("unused_val"));
        assert!(!retained_text.contains("unrelated"));
        assert!(retained_text.contains("COMPLETE"));
    }

    #[test]
    fn test_intra_function_conditional_branches() {
        let code = r#"fn process(input: i32, flag: bool) -> i32 {
    let mut res = 0;
    let mut debug_count = 0;
    if flag {
        res = input * 10;
        debug_count += 1;
    } else {
        res = input + 5;
        debug_count += 2;
    }
    println!("debug: {}", debug_count);
    res
}"#;
        let slice = slice_intra_function(code, 1, 14, "process", "src/proc.rs", Some(13), Some("res"));
        assert!(slice.completeness.is_complete());
        let text = slice.formatted_slice;
        assert!(text.contains("if flag {"));
        assert!(text.contains("res = input * 10;"));
        assert!(text.contains("res = input + 5;"));
        assert!(!text.contains("debug_count"));
        assert!(!text.contains("println!"));
    }

    #[test]
    fn test_intra_function_mutation_and_loops() {
        let code = r#"fn sum_positive(items: &[i32]) -> i32 {
    let mut total = 0;
    let mut skipped = 0;
    for &x in items {
        if x > 0 {
            total += x;
        } else {
            skipped += 1;
        }
    }
    log_metric(skipped);
    total
}"#;
        let slice = slice_intra_function(code, 1, 13, "sum_positive", "src/sum.rs", Some(12), Some("total"));
        assert!(slice.completeness.is_complete());
        let text = slice.formatted_slice;
        assert!(text.contains("total += x;"));
        assert!(text.contains("if x > 0 {"));
        assert!(!text.contains("skipped"));
        assert!(!text.contains("log_metric"));
    }

    #[test]
    fn test_intra_function_go_syntax() {
        let code = r#"func Process(id string, amount float64, fast bool) float64 {
    logger := log.New()
    logger.Print(id)
    discount := 0.0
    if amount > 100.0 {
        discount = amount * 0.1
    }
    fee := 5.0
    if fast {
        fee = 15.0
    }
    total := amount - discount + fee
    metrics.Incr()
    return total
}"#;
        let slice = slice_intra_function(code, 1, 15, "Process", "main.go", Some(14), Some("total"));
        assert!(slice.completeness.is_complete());
        let text = slice.formatted_slice;
        assert!(text.contains("discount := 0.0"));
        assert!(text.contains("fee := 5.0"));
        assert!(text.contains("total := amount - discount + fee"));
        assert!(!text.contains("logger"));
        assert!(!text.contains("metrics"));
    }

    #[test]
    fn test_intra_function_python_syntax() {
        let code = r#"def calculate_payout(user_id, base_salary, performance_score):
    audit_log(user_id, "start")
    multiplier = 1.0
    if performance_score > 90:
        multiplier = 1.5
    else:
        multiplier = 1.1
    payout = base_salary * multiplier
    send_notification(user_id, payout)
    return payout
"#;
        let slice = slice_intra_function(code, 1, 10, "calculate_payout", "service.py", Some(10), Some("payout"));
        assert!(slice.completeness.is_complete());
        let text = slice.formatted_slice;
        assert!(text.contains("multiplier = 1.0"));
        assert!(text.contains("if performance_score > 90:"));
        assert!(text.contains("payout = base_salary * multiplier"));
        assert!(!text.contains("audit_log"));
        assert!(!text.contains("send_notification"));
    }

    #[test]
    fn test_intra_function_typescript_syntax() {
        let code = r#"function getDiscountedPrice(item: Item, user: User, isVIP: boolean): number {
    console.log("Checking item", item.id);
    const base = item.price;
    let rate = 0.05;
    if (isVIP) {
        rate = 0.20;
    }
    const finalPrice = base * (1 - rate);
    trackAnalytics("price_computed", finalPrice);
    return finalPrice;
}"#;
        let slice = slice_intra_function(code, 1, 11, "getDiscountedPrice", "pricing.ts", Some(10), Some("finalPrice"));
        assert!(slice.completeness.is_complete());
        let text = slice.formatted_slice;
        assert!(text.contains("const base = item.price;"));
        assert!(text.contains("if (isVIP) {"));
        assert!(text.contains("const finalPrice = base * (1 - rate);"));
        assert!(!text.contains("console.log"));
        assert!(!text.contains("trackAnalytics"));
    }

    #[test]
    fn test_intra_function_auto_target_return() {
        let code = r#"fn compute_total(price: f64, tax_rate: f64) -> f64 {
    let subtotal = price;
    let unused_counter = 42;
    let tax = subtotal * tax_rate;
    let unneeded_str = format!("counter={}", unused_counter);
    let total = subtotal + tax;
    total
}"#;
        let slice = slice_intra_function(code, 1, 8, "compute_total", "src/math.rs", None, None);
        assert!(slice.completeness.is_complete());
        let text = slice.formatted_slice;
        assert!(text.contains("let subtotal = price;"));
        assert!(text.contains("let tax = subtotal * tax_rate;"));
        assert!(text.contains("let total = subtotal + tax;"));
        assert!(!text.contains("unused_counter"));
        assert!(!text.contains("unneeded_str"));
    }

    #[test]
    fn test_intra_function_incomplete_evidence() {
        let code = r#"fn compute(x: i32) -> i32 {
    let y = x + external_magic_value;
    y
}"#;
        let slice = slice_intra_function(code, 1, 4, "compute", "src/magic.rs", Some(3), Some("y"));
        assert!(!slice.completeness.is_complete());
        assert!(matches!(slice.completeness, SliceCompleteness::Incomplete { .. }));
        assert!(slice.formatted_slice.contains("INCOMPLETE"));
    }
}
