//! Polyglot caller type-annotation migration for extracted interfaces/traits (Roadmap 7.1.2).
//!
//! When an interface or trait is extracted from a class or struct, callers that only invoke
//! methods belonging to the extracted interface/trait have their type annotations migrated
//! from the concrete type to the interface/trait across the file and workspace.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::move_polyglot::{insert_or_merge_py_import, insert_or_merge_ts_import, is_symbol_used};
use crate::parameter_object::Language;
use crate::pull_push::find_matching_brace;
use crate::signature_polyglot::collect_workspace_sources;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerMigration {
    pub var_name: String,
    pub original_type: String,
    pub new_type: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ReplacementCandidate {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) replacement: String,
    pub(crate) migration: CallerMigration,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Computes a boolean mask indicating code bytes (true) versus comments/strings (false).
pub fn lexical_code_mask(text: &str, lang: Language) -> Vec<bool> {
    let bytes = text.as_bytes();
    let mut code = vec![true; bytes.len()];
    let mut i = 0usize;

    while i < bytes.len() {
        if lang == Language::Python && bytes[i] == b'#' {
            let end = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
            code[i..end].fill(false);
            i = end;
            continue;
        }

        if lang != Language::Python && bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            let end = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
            code[i..end].fill(false);
            i = end;
            continue;
        }

        if lang != Language::Python && bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let mut j = i + 2;
            let mut depth = 1usize;
            while j < bytes.len() && depth > 0 {
                if bytes[j] == b'/' && bytes.get(j + 1) == Some(&b'*') {
                    depth += 1;
                    j += 2;
                } else if bytes[j] == b'*' && bytes.get(j + 1) == Some(&b'/') {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            code[i..j].fill(false);
            i = j;
            continue;
        }

        // Strings
        let is_quote = if lang == Language::Python {
            bytes[i] == b'"' || bytes[i] == b'\''
        } else if lang == Language::TypeScript || lang == Language::JavaScript {
            bytes[i] == b'"' || bytes[i] == b'\'' || bytes[i] == b'`'
        } else {
            bytes[i] == b'"'
        };

        if is_quote {
            let quote = bytes[i];
            let mut j = i + 1;
            while j < bytes.len() {
                if bytes[j] == b'\\' {
                    j = (j + 2).min(bytes.len());
                } else if bytes[j] == quote {
                    j += 1;
                    break;
                } else {
                    j += 1;
                }
            }
            code[i..j].fill(false);
            i = j;
            continue;
        }

        i += 1;
    }

    code
}

/// Finds the end of a Python function body based on indentation.
fn find_python_body_close(text: &str, colon_pos: usize) -> usize {
    let line_start = text[..colon_pos].rfind('\n').map_or(0, |p| p + 1);
    let def_line = &text[line_start..colon_pos];
    let def_indent = def_line.len() - def_line.trim_start().len();

    let rest = &text[colon_pos + 1..];
    let mut current_offset = colon_pos + 1;
    for line in rest.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            current_offset += line.len() + 1;
            continue;
        }
        let line_indent = line.len() - line.trim_start().len();
        if line_indent <= def_indent {
            return current_offset;
        }
        current_offset += line.len() + 1;
    }
    text.len()
}

/// Checks whether all usages of `var_name` in `scope` only call/access methods in `extracted_methods`.
fn verify_all_usages_safe(
    text: &str,
    scope_start: usize,
    scope_end: usize,
    var_name: &str,
    extracted_methods: &[String],
    lang: Language,
    mask: &[bool],
) -> bool {
    let scope_text = &text[scope_start..scope_end];
    let var_bytes = var_name.as_bytes();

    let mut i = 0usize;
    while i < scope_text.len() {
        let abs_pos = scope_start + i;
        if abs_pos + var_bytes.len() <= text.len()
            && &text.as_bytes()[abs_pos..abs_pos + var_bytes.len()] == var_bytes
            && mask[abs_pos]
        {
            // Boundary checks: previous character
            let prev_ok = if abs_pos == 0 {
                true
            } else {
                let prev = text.as_bytes()[abs_pos - 1];
                !is_ident(prev as char) && prev != b'.'
            };

            // Boundary checks: next character
            let next_pos = abs_pos + var_bytes.len();
            let next_ok = if next_pos >= text.len() {
                true
            } else {
                let next = text.as_bytes()[next_pos];
                !is_ident(next as char)
            };

            if prev_ok && next_ok {
                // Inspect what follows var_name (skipping whitespace)
                let after = &text[next_pos..scope_end];
                let trimmed = after.trim_start();

                let member_opt = match lang {
                    Language::Cpp | Language::C => trimmed
                        .strip_prefix("->")
                        .or_else(|| trimmed.strip_prefix('.'))
                        .map(str::trim_start),
                    Language::TypeScript | Language::JavaScript => trimmed
                        .strip_prefix("?.")
                        .or_else(|| trimmed.strip_prefix('.'))
                        .map(str::trim_start),
                    _ => trimmed.strip_prefix('.').map(str::trim_start),
                };

                let Some(member_str) = member_opt else {
                    // Used as a bare value, passed as argument, returned, or assigned -> unsafe to migrate!
                    return false;
                };

                // Extract the member identifier name
                let member_name: String = member_str.chars().take_while(|c| is_ident(*c)).collect();
                if member_name.is_empty() {
                    return false;
                }

                if !extracted_methods.iter().any(|m| m == &member_name) {
                    // Accessed an unextracted method or field -> unsafe to migrate!
                    return false;
                }
            }
        }
        i += 1;
    }

    true
}

/// Finds all candidate parameter/variable migrations in a source text.
pub(crate) fn find_caller_migrations(
    text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
) -> Vec<ReplacementCandidate> {
    let mask = lexical_code_mask(text, lang);
    let mut candidates = Vec::new();

    match lang {
        Language::TypeScript | Language::JavaScript => {
            // Find `ident\s*(\?)?\s*:\s*type_name\b`
            for (pos, _) in text.match_indices(type_name) {
                if !mask[pos] {
                    continue;
                }
                // Check word boundaries
                let before_c = text[..pos].chars().next_back().unwrap_or(' ');
                let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
                if is_ident(before_c) || is_ident(after_c) {
                    continue;
                }
                // Do not match arrays, generics, or unions
                if matches!(after_c, '[' | '<' | '>' | '|' | '&') {
                    continue;
                }

                // Check preceding `:`
                let before = text[..pos].trim_end();
                if !before.ends_with(':') {
                    continue;
                }
                let before_colon = before[..before.len() - 1].trim_end();
                let before_colon = before_colon.trim_end_matches('?').trim_end();

                // Extract var_name
                let var_name: String = before_colon
                    .chars()
                    .rev()
                    .take_while(|c| is_ident(*c))
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                if var_name.is_empty()
                    || matches!(
                        var_name.as_str(),
                        "return"
                            | "function"
                            | "class"
                            | "interface"
                            | "type"
                            | "case"
                            | "default"
                            | "export"
                            | "import"
                    )
                {
                    continue;
                }

                // Ensure it's not a return type (preceded by `)`)
                let prefix_var = before_colon[..before_colon.len() - var_name.len()].trim_end();
                if prefix_var.ends_with(')') || prefix_var.ends_with("=>") {
                    continue;
                }

                // Determine scope
                let scope_opt = if let Some(open_paren) = text[..pos].rfind('(') {
                    if let Some(close_paren) = text[open_paren..].find(')') {
                        let abs_close = open_paren + close_paren;
                        if pos < abs_close {
                            // Inside parameter list: find `{` after `)`
                            if let Some(brace_offset) = text[abs_close..].find('{') {
                                let open_brace = abs_close + brace_offset;
                                find_matching_brace(text, open_brace)
                                    .map(|close_brace| (open_brace + 1, close_brace))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                let (scope_start, scope_end) = match scope_opt {
                    Some(s) => s,
                    None => {
                        // Check if it's a variable declaration in an enclosing block
                        if let Some(open_brace) = text[..pos].rfind('{') {
                            if let Some(close_brace) = find_matching_brace(text, open_brace) {
                                (pos + type_name.len(), close_brace)
                            } else {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    }
                };

                if verify_all_usages_safe(
                    text,
                    scope_start,
                    scope_end,
                    &var_name,
                    extracted_methods,
                    lang,
                    &mask,
                ) {
                    candidates.push(ReplacementCandidate {
                        start: pos,
                        end: pos + type_name.len(),
                        replacement: interface_name.to_string(),
                        migration: CallerMigration {
                            var_name,
                            original_type: type_name.to_string(),
                            new_type: interface_name.to_string(),
                        },
                    });
                }
            }
        }
        Language::Python => {
            // Find `ident:\s*type_name\b` or `ident:\s*"type_name"`
            for (pos, _) in text.match_indices(type_name) {
                // Word boundaries
                let before_c = text[..pos].chars().next_back().unwrap_or(' ');
                let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
                if is_ident(before_c) || is_ident(after_c) {
                    continue;
                }
                if matches!(after_c, '[' | '|' | ']') {
                    continue;
                }

                let is_quoted = before_c == '"' || before_c == '\'';
                let check_start = if is_quoted { pos - 1 } else { pos };
                let before = text[..check_start].trim_end();
                if !before.ends_with(':') {
                    continue;
                }
                let before_colon = before[..before.len() - 1].trim_end();
                let var_name: String = before_colon
                    .chars()
                    .rev()
                    .take_while(|c| is_ident(*c))
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                if var_name.is_empty()
                    || matches!(
                        var_name.as_str(),
                        "self" | "cls" | "def" | "class" | "return" | "yield"
                    )
                {
                    continue;
                }

                let prefix_var = before_colon[..before_colon.len() - var_name.len()].trim_end();
                if prefix_var.ends_with(')') || prefix_var.ends_with("->") {
                    continue;
                }

                // Scope: find the def header's colon and the indented body
                let _def_pos = match text[..pos].rfind("def ") {
                    Some(d) => d,
                    None => continue,
                };
                let colon_pos = match text[pos..].find(':') {
                    Some(c) => pos + c,
                    None => continue,
                };
                let scope_end = find_python_body_close(text, colon_pos);
                let scope_start = colon_pos + 1;

                if verify_all_usages_safe(
                    text,
                    scope_start,
                    scope_end,
                    &var_name,
                    extracted_methods,
                    lang,
                    &mask,
                ) {
                    candidates.push(ReplacementCandidate {
                        start: pos,
                        end: pos + type_name.len(),
                        replacement: interface_name.to_string(),
                        migration: CallerMigration {
                            var_name,
                            original_type: type_name.to_string(),
                            new_type: interface_name.to_string(),
                        },
                    });
                }
            }
        }
        Language::Go => {
            // Find `ident *type_name` or `ident type_name` inside `func`
            for (pos, _) in text.match_indices(type_name) {
                if !mask[pos] {
                    continue;
                }
                let before_c = text[..pos].chars().next_back().unwrap_or(' ');
                let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
                if is_ident(before_c) || is_ident(after_c) {
                    continue;
                }
                if after_c == '[' {
                    continue;
                }

                let has_pointer = before_c == '*';
                let start_idx = if has_pointer { pos - 1 } else { pos };
                let before_type = text[..start_idx].trim_end();

                let var_name: String = before_type
                    .chars()
                    .rev()
                    .take_while(|c| is_ident(*c))
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                if var_name.is_empty()
                    || matches!(
                        var_name.as_str(),
                        "func" | "type" | "var" | "const" | "return" | "struct" | "interface"
                    )
                {
                    continue;
                }

                // Scope: enclosing func
                let func_pos = match text[..start_idx].rfind("func ") {
                    Some(f) => f,
                    None => continue,
                };
                // Make sure pos is in parameter list of that func
                let open_paren = match text[func_pos..].find('(') {
                    Some(p) => func_pos + p,
                    None => continue,
                };
                let close_paren = match text[open_paren..].find(')') {
                    Some(p) => open_paren + p,
                    None => continue,
                };

                let is_receiver = text[func_pos + 5..open_paren].trim().is_empty();
                let (param_open, param_close) = if is_receiver {
                    if start_idx <= close_paren {
                        // Method receiver in Go cannot be an interface type
                        continue;
                    }
                    let next_open = match text[close_paren..].find('(') {
                        Some(p) => close_paren + p,
                        None => continue,
                    };
                    let next_close = match text[next_open..].find(')') {
                        Some(p) => next_open + p,
                        None => continue,
                    };
                    (next_open, next_close)
                } else {
                    (open_paren, close_paren)
                };

                if start_idx < param_open || start_idx > param_close {
                    continue;
                }

                let open_brace = match text[param_close..].find('{') {
                    Some(b) => param_close + b,
                    None => continue,
                };
                let close_brace = match find_matching_brace(text, open_brace) {
                    Some(b) => b,
                    None => continue,
                };

                if verify_all_usages_safe(
                    text,
                    open_brace + 1,
                    close_brace,
                    &var_name,
                    extracted_methods,
                    lang,
                    &mask,
                ) {
                    candidates.push(ReplacementCandidate {
                        start: start_idx,
                        end: pos + type_name.len(),
                        replacement: interface_name.to_string(),
                        migration: CallerMigration {
                            var_name,
                            original_type: if has_pointer {
                                format!("*{type_name}")
                            } else {
                                type_name.to_string()
                            },
                            new_type: interface_name.to_string(),
                        },
                    });
                }
            }
        }
        Language::Cpp | Language::C => {
            // Find `const type_name& ident`, `type_name* ident`, `const type_name* ident`, etc.
            for (pos, _) in text.match_indices(type_name) {
                if !mask[pos] {
                    continue;
                }
                let before_c = text[..pos].chars().next_back().unwrap_or(' ');
                let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
                if is_ident(before_c) || is_ident(after_c) {
                    continue;
                }

                // Check preceding `const`
                let before_type = text[..pos].trim_end();
                let (has_const, start_idx) = if before_type.ends_with("const") {
                    let const_start = before_type.len() - 5;
                    let before_const = &before_type[..const_start];
                    if before_const.is_empty() || !is_ident(before_const.chars().next_back().unwrap()) {
                        (true, const_start)
                    } else {
                        (false, pos)
                    }
                } else {
                    (false, pos)
                };

                // Check following `&` or `*`
                let after_type = &text[pos + type_name.len()..];
                let trimmed_after = after_type.trim_start();
                let (has_ref, has_ptr, rest) = if let Some(r) = trimmed_after.strip_prefix('&') {
                    (true, false, r.trim_start())
                } else if let Some(p) = trimmed_after.strip_prefix('*') {
                    (false, true, p.trim_start())
                } else {
                    (false, false, trimmed_after)
                };

                let var_name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
                if var_name.is_empty()
                    || matches!(
                        var_name.as_str(),
                        "class" | "struct" | "void" | "int" | "double" | "return"
                    )
                {
                    continue;
                }

                // End of type annotation span
                let end_idx = pos + type_name.len()
                    + (after_type.len() - trimmed_after.len())
                    + usize::from(has_ref || has_ptr);

                // Scope: find `{` after `)`
                let open_paren = match text[..pos].rfind('(') {
                    Some(p) => p,
                    None => continue,
                };
                let close_paren = match text[pos..].find(')') {
                    Some(p) => pos + p,
                    None => continue,
                };
                if pos < open_paren || pos > close_paren {
                    continue;
                }
                let open_brace = match text[close_paren..].find('{') {
                    Some(b) => close_paren + b,
                    None => continue,
                };
                let close_brace = match find_matching_brace(text, open_brace) {
                    Some(b) => b,
                    None => continue,
                };

                if verify_all_usages_safe(
                    text,
                    open_brace + 1,
                    close_brace,
                    &var_name,
                    extracted_methods,
                    lang,
                    &mask,
                ) {
                    let replacement = if has_ptr {
                        if has_const {
                            format!("const {interface_name}*")
                        } else {
                            format!("{interface_name}*")
                        }
                    } else if has_const || !has_ref {
                        format!("const {interface_name}&")
                    } else {
                        format!("{interface_name}&")
                    };

                    candidates.push(ReplacementCandidate {
                        start: start_idx,
                        end: end_idx,
                        replacement: replacement.clone(),
                        migration: CallerMigration {
                            var_name,
                            original_type: text[start_idx..end_idx].trim().to_string(),
                            new_type: replacement,
                        },
                    });
                }
            }
        }
        Language::Swift => {
            // Find `ident:\s*(any\s+)?type_name\b`
            for (pos, _) in text.match_indices(type_name) {
                if !mask[pos] {
                    continue;
                }
                let before_c = text[..pos].chars().next_back().unwrap_or(' ');
                let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
                if is_ident(before_c) || is_ident(after_c) {
                    continue;
                }
                if matches!(after_c, '[' | '?') {
                    continue;
                }

                let before_type = text[..pos].trim_end();
                let before_type = before_type.strip_suffix("any").unwrap_or(before_type).trim_end();
                if !before_type.ends_with(':') {
                    continue;
                }
                let before_colon = before_type[..before_type.len() - 1].trim_end();
                let var_name: String = before_colon
                    .chars()
                    .rev()
                    .take_while(|c| is_ident(*c))
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                if var_name.is_empty()
                    || matches!(
                        var_name.as_str(),
                        "self" | "Self" | "func" | "class" | "struct" | "protocol" | "return"
                    )
                {
                    continue;
                }

                // Scope: find `{` after parameter list or declaration
                let open_brace = match text[pos..].find('{') {
                    Some(b) => pos + b,
                    None => continue,
                };
                let close_brace = match find_matching_brace(text, open_brace) {
                    Some(b) => b,
                    None => continue,
                };

                if verify_all_usages_safe(
                    text,
                    open_brace + 1,
                    close_brace,
                    &var_name,
                    extracted_methods,
                    lang,
                    &mask,
                ) {
                    candidates.push(ReplacementCandidate {
                        start: pos,
                        end: pos + type_name.len(),
                        replacement: interface_name.to_string(),
                        migration: CallerMigration {
                            var_name,
                            original_type: type_name.to_string(),
                            new_type: interface_name.to_string(),
                        },
                    });
                }
            }
        }
        Language::Rust => {
            if type_name.contains('<') || interface_name.contains('<') {
                return candidates;
            }
            // Find `ident:\s*&mut\s+type_name\b`, `ident:\s*&type_name\b`, `ident:\s*type_name\b`
            for (pos, _) in text.match_indices(type_name) {
                if !mask[pos] {
                    continue;
                }
                let before_c = text[..pos].chars().next_back().unwrap_or(' ');
                let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
                if is_ident(before_c) || is_ident(after_c) {
                    continue;
                }
                if matches!(after_c, '<' | '[' | ':') {
                    continue;
                }

                let before_type = text[..pos].trim_end();
                let (has_ref, has_mut, type_start) = if let Some(r) = before_type.strip_suffix("&mut") {
                    (true, true, pos - (before_type.len() - r.len()))
                } else if let Some(r) = before_type.strip_suffix('&') {
                    (true, false, pos - (before_type.len() - r.len()))
                } else {
                    (false, false, pos)
                };

                let before_ref = text[..type_start].trim_end();
                if !before_ref.ends_with(':') {
                    continue;
                }
                let before_colon = before_ref[..before_ref.len() - 1].trim_end();
                let var_name: String = before_colon
                    .chars()
                    .rev()
                    .take_while(|c| is_ident(*c))
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                if var_name.is_empty()
                    || matches!(
                        var_name.as_str(),
                        "self" | "Self" | "fn" | "let" | "mut" | "pub" | "return"
                    )
                {
                    continue;
                }

                // Scope: find `{` after parameter list
                let open_paren = match text[..pos].rfind('(') {
                    Some(p) => p,
                    None => continue,
                };
                let close_paren = match text[pos..].find(')') {
                    Some(p) => pos + p,
                    None => continue,
                };
                if pos < open_paren || pos > close_paren {
                    continue;
                }
                let open_brace = match text[close_paren..].find('{') {
                    Some(b) => close_paren + b,
                    None => continue,
                };
                let close_brace = match find_matching_brace(text, open_brace) {
                    Some(b) => b,
                    None => continue,
                };

                if verify_all_usages_safe(
                    text,
                    open_brace + 1,
                    close_brace,
                    &var_name,
                    extracted_methods,
                    lang,
                    &mask,
                ) {
                    let replacement = if has_mut {
                        format!("&mut impl {interface_name}")
                    } else if has_ref {
                        format!("&impl {interface_name}")
                    } else {
                        format!("impl {interface_name}")
                    };

                    candidates.push(ReplacementCandidate {
                        start: type_start,
                        end: pos + type_name.len(),
                        replacement: replacement.clone(),
                        migration: CallerMigration {
                            var_name,
                            original_type: text[type_start..pos + type_name.len()].trim().to_string(),
                            new_type: replacement,
                        },
                    });
                }
            }
        }
        Language::Java => {}
    }

    candidates
}

/// Migrates caller type annotations in a single file text.
pub fn migrate_caller_annotations(
    text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
) -> (String, Vec<CallerMigration>) {
    let mut candidates =
        find_caller_migrations(text, type_name, interface_name, extracted_methods, lang);
    if candidates.is_empty() {
        return (text.to_string(), Vec::new());
    }

    // Sort descending by start offset to apply edits without shifting offsets
    candidates.sort_by_key(|c| std::cmp::Reverse(c.start));

    let mut out = text.to_string();
    let mut migrations = Vec::new();

    for c in candidates {
        out.replace_range(c.start..c.end, &c.replacement);
        migrations.push(c.migration);
    }

    migrations.reverse();
    (out, migrations)
}

/// Migrates caller type annotations across the declaring file and external workspace files.
pub fn migrate_callers_in_workspace(
    root: &Path,
    declaring_file: &Path,
    declaring_text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
) -> Result<Vec<(PathBuf, String)>> {
    let mut results = Vec::new();

    // 1. Declaring file
    let (migrated_declaring, _) = migrate_caller_annotations(
        declaring_text,
        type_name,
        interface_name,
        extracted_methods,
        lang,
    );
    results.push((declaring_file.to_path_buf(), migrated_declaring));

    // 2. Other workspace files
    let sources = collect_workspace_sources(root, lang);
    for other in sources {
        if other == *declaring_file {
            continue;
        }
        let Ok(other_text) = std::fs::read_to_string(&other) else {
            continue;
        };
        if !is_symbol_used(&other_text, type_name) {
            continue;
        }

        let (mut new_other_text, migrations) = migrate_caller_annotations(
            &other_text,
            type_name,
            interface_name,
            extracted_methods,
            lang,
        );

        if !migrations.is_empty() {
            // Add appropriate imports
            match lang {
                Language::TypeScript | Language::JavaScript => {
                    let rel = crate::move_polyglot::relative_import_specifier(&other, declaring_file);
                    let (with_import, _) =
                        insert_or_merge_ts_import(&new_other_text, interface_name, &rel);
                    new_other_text = with_import;
                }
                Language::Python => {
                    let mod_spec =
                        crate::move_polyglot::python_module_specifier(&other, declaring_file, root);
                    let (with_import, _) =
                        insert_or_merge_py_import(&new_other_text, interface_name, &mod_spec);
                    new_other_text = with_import;
                }
                Language::Cpp | Language::C => {
                    let header_name = declaring_file
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if !header_name.is_empty() && !new_other_text.contains(&header_name) {
                        new_other_text.insert_str(0, &format!("#include \"{header_name}\"\n"));
                    }
                }
                Language::Rust => {
                    // Handled via module spelled_from
                }
                _ => {}
            }

            results.push((other, new_other_text));
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ts_caller_migration() {
        let ts = r#"export function handleUser(svc: UserService, other: UserService) {
    svc.getUser("123");
    svc.deleteUser("123");
    other.getUser("456");
    other.internalDb();
}
"#;
        let methods = vec!["getUser".to_string(), "deleteUser".to_string()];
        let (out, migrations) = migrate_caller_annotations(
            ts,
            "UserService",
            "IUserService",
            &methods,
            Language::TypeScript,
        );

        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].var_name, "svc");
        assert!(out.contains("svc: IUserService"));
        assert!(out.contains("other: UserService"));
    }

    #[test]
    fn test_go_caller_migration() {
        let go = r#"package user

func ProcessUser(svc *UserService) {
    svc.GetUser("1")
}

func AuditUser(svc *UserService) {
    _ = svc.db
}
"#;
        let methods = vec!["GetUser".to_string()];
        let (out, migrations) =
            migrate_caller_annotations(go, "UserService", "UserReader", &methods, Language::Go);

        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].var_name, "svc");
        assert!(out.contains("func ProcessUser(svc UserReader) {"));
        assert!(out.contains("func AuditUser(svc *UserService) {"));
    }

    #[test]
    fn test_python_caller_migration() {
        let py = r#"def transfer(acc: AccountService, other: AccountService):
    acc.deposit(100.0)
    acc.withdraw(50.0)
    other.secret_key()
"#;
        let methods = vec!["deposit".to_string(), "withdraw".to_string()];
        let (out, migrations) = migrate_caller_annotations(
            py,
            "AccountService",
            "AccountProtocol",
            &methods,
            Language::Python,
        );

        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].var_name, "acc");
        assert!(out.contains("acc: AccountProtocol"));
        assert!(out.contains("other: AccountService"));
    }

    #[test]
    fn test_cpp_caller_migration() {
        let cpp = r#"double calculate(const Shape& s, const Shape& other) {
    double a = s.area();
    double b = other.id;
    return a;
}
"#;
        let methods = vec!["area".to_string()];
        let (out, migrations) =
            migrate_caller_annotations(cpp, "Shape", "IShape", &methods, Language::Cpp);

        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].var_name, "s");
        assert!(out.contains("double calculate(const IShape& s, const Shape& other) {"));
    }

    #[test]
    fn test_swift_caller_migration() {
        let swift = r#"func executePayment(service: PaymentService, other: PaymentService) {
    _ = service.pay(amount: 100.0)
    print(other.secret)
}
"#;
        let methods = vec!["pay".to_string()];
        let (out, migrations) = migrate_caller_annotations(
            swift,
            "PaymentService",
            "PaymentProtocol",
            &methods,
            Language::Swift,
        );

        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].var_name, "service");
        assert!(out.contains("service: PaymentProtocol"));
        assert!(out.contains("other: PaymentService"));
    }

    #[test]
    fn test_rust_caller_migration() {
        let rust = r#"fn print_area(r: &Rect, other: &Rect) -> f64 {
    let a = r.area();
    let b = other.w;
    a
}
"#;
        let methods = vec!["area".to_string()];
        let (out, migrations) =
            migrate_caller_annotations(rust, "Rect", "Measure", &methods, Language::Rust);

        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].var_name, "r");
        assert!(out.contains("r: &impl Measure"));
        assert!(out.contains("other: &Rect"));
    }
}
