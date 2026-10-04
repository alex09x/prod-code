//! Replace conditional logic with polymorphism across polyglot languages (Roadmap 7.1.5).
//!
//! Fowler's "Replace Conditional with Polymorphism" refactoring replaces complex
//! `switch` / `match` statements or `if-elif-else` cascades discriminating on a type tag
//! with polymorphic class, interface, protocol, or trait hierarchies.
//!
//! Supports Python, TypeScript / JavaScript, C++, Swift, and Rust.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// A branch within a conditional block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalBranch {
    pub tag: String,
    pub variant_name: String,
    pub body: String,
    pub is_default: bool,
}

/// The kind of conditional syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionalKind {
    Switch,
    Match,
    IfElse,
}

/// A parsed conditional block with its branches.
#[derive(Debug, Clone)]
pub struct ConditionalBlock {
    pub kind: ConditionalKind,
    pub start_offset: usize,
    pub end_offset: usize,
    pub discriminator: String,
    pub branches: Vec<ConditionalBranch>,
    pub indent: String,
}

fn returns_from_conditional(block: &ConditionalBlock) -> Result<bool> {
    anyhow::ensure!(!block.branches.is_empty(), "conditional has no branches");
    let mut returns = Vec::with_capacity(block.branches.len());
    for branch in &block.branches {
        let body = branch.body.trim_start();
        let has_return = contains_word(body, "return");
        let starts_with_return = body.starts_with("return ") || body.starts_with("return\n");
        let terminates_without_return = [
            "throw ",
            "raise ",
            "panic(",
            "panic!",
            "fatalError(",
            "fatalError ",
        ]
        .iter()
        .any(|prefix| body.starts_with(prefix));
        anyhow::ensure!(
            !has_return || starts_with_return,
            "branch control flow is too complex to preserve safely; each branch must return or terminate directly, or none may return"
        );
        returns.push(starts_with_return || terminates_without_return);
    }
    if returns.iter().all(|value| *value) {
        anyhow::ensure!(
            block.branches.iter().any(|branch| branch.is_default),
            "a returning conditional without a default branch cannot be converted without changing fallthrough behavior"
        );
        Ok(true)
    } else {
        anyhow::ensure!(
            returns.iter().all(|value| !*value),
            "conditional branches mix returns and statements; control flow cannot be preserved safely"
        );
        Ok(false)
    }
}

fn contains_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        (at == 0
            || !text[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_'))
            && !text[at + word.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// Outcome of replacing a conditional with polymorphism.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReplaceConditionalResult {
    pub base_name: String,
    pub method_name: String,
    pub variants: Vec<String>,
    pub files_modified: Vec<String>,
    #[serde(skip)]
    pub overlays: Vec<(PathBuf, String)>,
    pub diff: String,
    pub applied: bool,
    pub verified: bool,
    pub diagnostics: Vec<String>,
}

impl ReplaceConditionalResult {
    pub fn render(&self, max_diff_len: usize) -> String {
        let mut out = format!(
            "`{}` — replaced conditional with polymorphism (method: `{}`)\n",
            self.base_name, self.method_name
        );
        out.push_str(&format!(
            "- variants ({}): {}\n",
            self.variants.len(),
            if self.variants.is_empty() {
                "none".to_string()
            } else {
                self.variants.join(", ")
            }
        ));
        out.push_str(&format!(
            "- files modified ({}): {}\n",
            self.files_modified.len(),
            self.files_modified.join(", ")
        ));
        out.push_str(&format!("- applied: {}\n", self.applied));
        out.push_str(&format!("- verified: {}\n", self.verified));
        if !self.diagnostics.is_empty() {
            out.push_str(&format!("- diagnostics ({}):\n", self.diagnostics.len()));
            for d in &self.diagnostics {
                out.push_str(&format!("  • {d}\n"));
            }
        }
        if !self.diff.is_empty() {
            out.push_str("\nDiff:\n```diff\n");
            if self.diff.len() > max_diff_len {
                out.push_str(&self.diff[..max_diff_len]);
                out.push_str("\n... [truncated]\n");
            } else {
                out.push_str(&self.diff);
            }
            out.push_str("```\n");
        }
        out
    }
}

/// Convert a tag string into a clean PascalCase identifier for class/struct naming.
pub fn tag_to_variant_name(tag: &str) -> String {
    let clean = tag
        .trim()
        .trim_matches(['"', '\'', '`'])
        .trim_start_matches('.')
        .split("::")
        .last()
        .unwrap_or(tag)
        .trim();

    if clean.is_empty() || clean == "default" || clean == "_" {
        return "Default".to_string();
    }

    let is_all_upper = clean
        .chars()
        .all(|c| !c.is_alphabetic() || c.is_uppercase());
    let mut out = String::new();
    let mut capitalize_next = true;
    for c in clean.chars() {
        if c.is_alphanumeric() {
            if c.is_ascii_digit() && out.is_empty() {
                out.push_str("Case");
            }
            if capitalize_next {
                out.extend(c.to_uppercase());
                capitalize_next = false;
            } else if is_all_upper {
                out.extend(c.to_lowercase());
            } else {
                out.push(c);
            }
        } else {
            capitalize_next = true;
        }
    }

    if out.is_empty() {
        "Variant".to_string()
    } else {
        out
    }
}

/// Find line indentation of the line containing `offset`.
pub fn line_indentation(text: &str, offset: usize) -> String {
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    text[line_start..offset]
        .chars()
        .take_while(|c| c.is_whitespace() && *c != '\n')
        .collect()
}

/// Convert 1-based (line, col) to byte offset in text.
pub fn line_col_to_offset(text: &str, line: u32, col: u32) -> usize {
    let mut offset = 0;
    for (i, line_str) in text.split_inclusive('\n').enumerate() {
        if (i as u32 + 1) == line {
            let col_offset = (col.saturating_sub(1) as usize).min(line_str.len());
            return offset + col_offset;
        }
        offset += line_str.len();
    }
    offset
}

fn find_default_label(s: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    while let Some(idx) = s[offset..].find("default") {
        let abs = offset + idx;
        let after = &s[abs + 7..];
        let trimmed_len = after.len() - after.trim_start().len();
        if after[trimmed_len..].starts_with(':') {
            return Some((abs, abs + 7 + trimmed_len + 1));
        }
        offset = abs + 7;
    }
    None
}

fn is_switch_keyword(text: &str, at: usize) -> bool {
    let after = &text[at + "switch".len()..];
    (at == 0
        || !text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_'))
        && !after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        && !crate::inline_parameter::is_in_comment(
            text,
            at,
            crate::parameter_object::Language::TypeScript,
        )
        && !crate::inline_parameter::is_in_string(
            text,
            at,
            crate::parameter_object::Language::TypeScript,
        )
}

/// Parse a `switch` statement in C-like languages (TypeScript, JavaScript, C++, Swift, Go).
pub fn parse_switch_block(text: &str, search_offset: usize) -> Option<ConditionalBlock> {
    let switch_keyword_idx = text
        .match_indices("switch")
        .map(|(at, _)| at)
        .find(|at| *at >= search_offset && is_switch_keyword(text, *at))
        .or_else(|| {
            text.match_indices("switch")
                .map(|(at, _)| at)
                .filter(|at| *at < search_offset && is_switch_keyword(text, *at))
                .last()
        })?;

    let open_brace = text[switch_keyword_idx..].find('{')? + switch_keyword_idx;
    let close_brace = crate::pull_push::find_matching_brace(text, open_brace)?;

    let header = text[switch_keyword_idx..open_brace].trim();
    let discriminator = if let Some(open_paren) = header.find('(')
        && let Some(close_paren) = header.rfind(')')
    {
        header[open_paren + 1..close_paren].trim().to_string()
    } else {
        header.strip_prefix("switch")?.trim().to_string()
    };

    let inner = &text[open_brace + 1..close_brace];
    let indent = line_indentation(text, switch_keyword_idx);

    // Split inner body into cases
    let mut branches = Vec::new();
    let mut pos = 0;

    while pos < inner.len() {
        let rest = &inner[pos..];
        let next_case = rest.find("case ");
        let next_default = find_default_label(rest);

        let (is_default, tag, body_start) = match (next_case, next_default) {
            (Some(c), Some((d, _))) if c < d => {
                let tag_start = pos + c + 5;
                let colon_rel = inner[tag_start..].find(':')?;
                let colon_idx = tag_start + colon_rel;
                let tag = inner[tag_start..colon_idx].trim().to_string();
                (false, tag, colon_idx + 1)
            }
            (Some(_), Some((_, d_end))) => (true, "default".to_string(), pos + d_end),
            (Some(c), None) => {
                let tag_start = pos + c + 5;
                let colon_rel = inner[tag_start..].find(':')?;
                let colon_idx = tag_start + colon_rel;
                let tag = inner[tag_start..colon_idx].trim().to_string();
                (false, tag, colon_idx + 1)
            }
            (None, Some((_, d_end))) => (true, "default".to_string(), pos + d_end),
            (None, None) => break,
        };

        // Body extends to next case/default or end of block
        let rem = &inner[body_start..];
        let next_c = rem.find("case ");
        let next_d = find_default_label(rem).map(|(s, _)| s);
        let next_marker = match (next_c, next_d) {
            (Some(c), Some(d)) => Some(c.min(d)),
            (Some(c), None) => Some(c),
            (None, Some(d)) => Some(d),
            (None, None) => None,
        };
        let body_end = if let Some(m) = next_marker {
            body_start + m
        } else {
            inner.len()
        };

        let raw_body = inner[body_start..body_end].trim();
        // Clean break statements from body
        let clean_body = raw_body
            .lines()
            .filter(|l| {
                let t = l.trim();
                t != "break;" && t != "break"
            })
            .collect::<Vec<_>>()
            .join("\n");

        branches.push(ConditionalBranch {
            variant_name: tag_to_variant_name(&tag),
            tag,
            body: clean_body,
            is_default,
        });

        if body_end <= pos {
            break;
        }
        pos = body_end;
    }

    if branches.is_empty() {
        return None;
    }

    Some(ConditionalBlock {
        kind: ConditionalKind::Switch,
        start_offset: switch_keyword_idx,
        end_offset: close_brace + 1,
        discriminator,
        branches,
        indent,
    })
}

/// Parse an `if / else if / else` or Python `if / elif / else` cascade.
pub fn parse_if_else_block(text: &str, search_offset: usize) -> Option<ConditionalBlock> {
    let if_idx = text[search_offset..]
        .find("if ")
        .map(|idx| search_offset + idx)
        .or_else(|| text[..search_offset].rfind("if "))?;

    let if_line_start = text[..if_idx].rfind('\n').map_or(0, |i| i + 1);
    let indent = line_indentation(text, if_idx);
    let is_python = text[if_idx..].find(':').is_some()
        && (text[if_idx..].find('{').is_none()
            || text[if_idx..].find(':').unwrap() < text[if_idx..].find('{').unwrap());

    if is_python {
        parse_python_if_elif(text, if_line_start, &indent)
    } else {
        parse_curly_if_else(text, if_idx, &indent)
    }
}

fn parse_python_if_elif(text: &str, if_idx: usize, base_indent: &str) -> Option<ConditionalBlock> {
    let mut branches = Vec::new();
    let mut discriminator = String::new();

    let lines: Vec<&str> = text[if_idx..].lines().collect();
    let mut end_line_count = 0;
    let mut current_tag = String::new();
    let mut current_body_lines = Vec::new();
    let mut is_default = false;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let line_indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let base_indent_len = base_indent.len();

        if line_indent == base_indent_len
            && (trimmed.starts_with("if ")
                || trimmed.starts_with("elif ")
                || trimmed.starts_with("else:"))
        {
            // Flush previous branch
            if !current_tag.is_empty() {
                branches.push(ConditionalBranch {
                    variant_name: tag_to_variant_name(&current_tag),
                    tag: current_tag.clone(),
                    body: current_body_lines.join("\n"),
                    is_default,
                });
                current_body_lines.clear();
            }

            if trimmed.starts_with("if ") || trimmed.starts_with("elif ") {
                let cond_part = if let Some(rest) = trimmed.strip_prefix("if ") {
                    rest.strip_suffix(':').unwrap_or(rest).trim()
                } else if let Some(rest) = trimmed.strip_prefix("elif ") {
                    rest.strip_suffix(':').unwrap_or(rest).trim()
                } else {
                    ""
                };

                // Parse `x == "VAL"` or `x == VAL`
                if let Some((lhs, rhs)) = cond_part.split_once("==") {
                    if discriminator.is_empty() {
                        discriminator = lhs.trim().to_string();
                    }
                    current_tag = rhs.trim().to_string();
                    is_default = false;
                } else {
                    current_tag = cond_part.to_string();
                    is_default = false;
                }
            } else if trimmed.starts_with("else:") {
                current_tag = "default".to_string();
                is_default = true;
            }
            end_line_count = idx + 1;
        } else if line_indent > base_indent_len {
            current_body_lines.push(*line);
            end_line_count = idx + 1;
        } else if idx > 0 && !trimmed.is_empty() {
            // Finished the cascade
            break;
        }
    }

    if !current_tag.is_empty() {
        branches.push(ConditionalBranch {
            variant_name: tag_to_variant_name(&current_tag),
            tag: current_tag,
            body: current_body_lines.join("\n"),
            is_default,
        });
    }

    if branches.len() < 2 {
        return None;
    }

    // Compute end offset from end_line_count
    let mut end_offset = if_idx;
    for l in lines.iter().take(end_line_count) {
        end_offset += l.len() + 1;
    }
    end_offset = end_offset.min(text.len());

    Some(ConditionalBlock {
        kind: ConditionalKind::IfElse,
        start_offset: if_idx,
        end_offset,
        discriminator,
        branches,
        indent: base_indent.to_string(),
    })
}

fn parse_curly_if_else(text: &str, if_idx: usize, base_indent: &str) -> Option<ConditionalBlock> {
    let mut branches = Vec::new();
    let mut discriminator = String::new();
    let mut pos = if_idx;
    let mut last_end = if_idx;

    while pos < text.len() {
        let rest = text[pos..].trim_start();
        if rest.starts_with("if ")
            || rest.starts_with("else if ")
            || rest.starts_with("if(")
            || rest.starts_with("else if(")
        {
            let is_else_if = rest.starts_with("else if");
            let after_kw = if is_else_if { &rest[7..] } else { &rest[2..] };
            let open_brace = after_kw.find('{')?;
            let header = after_kw[..open_brace].trim();

            let cond_str = if let Some(op) = header.find('(')
                && let Some(cp) = header.rfind(')')
            {
                &header[op + 1..cp]
            } else {
                header
            };

            let tag = if let Some((lhs, rhs)) = cond_str
                .split_once("===")
                .or_else(|| cond_str.split_once("=="))
            {
                if discriminator.is_empty() {
                    discriminator = lhs.trim().to_string();
                }
                rhs.trim().to_string()
            } else {
                cond_str.trim().to_string()
            };

            let brace_global = text[pos..].find('{')? + pos;
            let close_brace = crate::pull_push::find_matching_brace(text, brace_global)?;
            let body = text[brace_global + 1..close_brace].trim().to_string();

            branches.push(ConditionalBranch {
                variant_name: tag_to_variant_name(&tag),
                tag,
                body,
                is_default: false,
            });

            last_end = close_brace + 1;
            pos = last_end;
        } else if rest.starts_with("else") && (rest[4..].trim_start().starts_with('{')) {
            let brace_global = text[pos..].find('{')? + pos;
            let close_brace = crate::pull_push::find_matching_brace(text, brace_global)?;
            let body = text[brace_global + 1..close_brace].trim().to_string();

            branches.push(ConditionalBranch {
                variant_name: "Default".to_string(),
                tag: "default".to_string(),
                body,
                is_default: true,
            });

            last_end = close_brace + 1;
            break;
        } else {
            break;
        }
    }

    if branches.len() < 2 {
        return None;
    }

    Some(ConditionalBlock {
        kind: ConditionalKind::IfElse,
        start_offset: if_idx,
        end_offset: last_end,
        discriminator,
        branches,
        indent: base_indent.to_string(),
    })
}

/// Parse a Rust `match` statement.
pub fn parse_rust_match(text: &str, search_offset: usize) -> Option<ConditionalBlock> {
    let match_kw_idx = text[search_offset..]
        .find("match ")
        .map(|idx| search_offset + idx)
        .or_else(|| text[..search_offset].rfind("match "))?;

    let open_brace = text[match_kw_idx..].find('{')? + match_kw_idx;
    let close_brace = crate::pull_push::find_matching_brace(text, open_brace)?;

    let header = text[match_kw_idx..open_brace].trim();
    let discriminator = header.strip_prefix("match")?.trim().to_string();

    let inner = &text[open_brace + 1..close_brace];
    let indent = line_indentation(text, match_kw_idx);

    let mut branches = Vec::new();
    let mut pos = 0;

    while pos < inner.len() {
        let rest = &inner[pos..];
        let Some(arrow_idx) = rest.find("=>") else {
            break;
        };
        let pattern_part = rest[..arrow_idx].trim();
        let pattern = pattern_part.lines().last().unwrap_or(pattern_part).trim();
        if pattern.is_empty() {
            break;
        }

        let is_default = pattern == "_";
        let after_arrow = &rest[arrow_idx + 2..];
        let after_arrow_trimmed = after_arrow.trim_start();
        let leading_spaces = after_arrow.len() - after_arrow_trimmed.len();
        let body_start_rel = arrow_idx + 2 + leading_spaces;

        let (body, branch_end_rel) = if after_arrow_trimmed.starts_with('{') {
            let brace_in_inner = pos + body_start_rel;
            let matching = crate::pull_push::find_matching_brace(inner, brace_in_inner)?;
            let b = inner[brace_in_inner + 1..matching].trim().to_string();
            let mut end_rel = matching + 1 - pos;
            if inner[matching + 1..].starts_with(',') {
                end_rel += 1;
            }
            (b, end_rel)
        } else {
            let next_comma = after_arrow.find(',');
            let end_rel = if let Some(c) = next_comma {
                arrow_idx + 2 + c + 1
            } else {
                rest.len()
            };
            let b = rest[arrow_idx + 2..if let Some(c) = next_comma {
                arrow_idx + 2 + c
            } else {
                rest.len()
            }]
                .trim()
                .to_string();
            (b, end_rel)
        };

        branches.push(ConditionalBranch {
            variant_name: tag_to_variant_name(pattern),
            tag: pattern.to_string(),
            body,
            is_default,
        });

        if branch_end_rel == 0 {
            break;
        }
        pos += branch_end_rel;
    }

    if branches.is_empty() {
        return None;
    }

    Some(ConditionalBlock {
        kind: ConditionalKind::Switch,
        start_offset: match_kw_idx,
        end_offset: close_brace + 1,
        discriminator,
        branches,
        indent,
    })
}

/// Core transformation for Python.
pub fn transform_python(
    text: &str,
    block: &ConditionalBlock,
    base_name: &str,
    method_name: &str,
    params: &[String],
    target_var: &str,
) -> Result<String> {
    let mut out = text.to_string();
    let indent = &block.indent;
    let returns_value = returns_from_conditional(block)?;

    let params_str = if params.is_empty() {
        "self".to_string()
    } else {
        format!("self, {}", params.join(", "))
    };
    let call_args = if params.is_empty() {
        "".to_string()
    } else {
        params
            .iter()
            .map(|p| p.split(':').next().unwrap_or(p).trim())
            .collect::<Vec<_>>()
            .join(", ")
    };

    // 1. Generate base class
    let mut classes = Vec::new();
    let base_class = format!(
        "class {base_name}:\n    def {method_name}({params_str}):\n        raise NotImplementedError\n"
    );
    classes.push(base_class);

    // 2. Generate variant subclasses
    for b in &block.branches {
        if b.is_default && b.body.trim().contains("raise") {
            continue; // Skip default error branch from creating a class
        }
        let v_name = format!("{}{base_name}", b.variant_name);
        let indented_body = if b.body.trim().is_empty() {
            "        pass".to_string()
        } else {
            b.body
                .lines()
                .map(|l| format!("        {}", l.trim()))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let cls = format!(
            "class {v_name}({base_name}):\n    def {method_name}({params_str}):\n{indented_body}\n"
        );
        classes.push(cls);
    }

    // 3. Replacement for conditional block
    let return_prefix = if returns_value { "return " } else { "" };
    let replacement = format!("{indent}{return_prefix}{target_var}.{method_name}({call_args})");
    out.replace_range(block.start_offset..block.end_offset, &replacement);

    // 4. Prepend classes right before enclosing function or at top of file
    let class_definitions = format!("{}\n\n", classes.join("\n"));
    out.insert_str(0, &class_definitions);

    Ok(out)
}

/// Core transformation for TypeScript / JavaScript.
pub fn transform_typescript(
    text: &str,
    block: &ConditionalBlock,
    base_name: &str,
    method_name: &str,
    params: &[String],
    return_type: Option<&str>,
    target_var: &str,
) -> Result<String> {
    let mut out = text.to_string();
    let indent = &block.indent;
    let returns_value = returns_from_conditional(block)?;

    let ret_type = return_type.unwrap_or(if returns_value { "any" } else { "void" });
    anyhow::ensure!(
        returns_value || ret_type == "void",
        "a statement conditional cannot be assigned a non-void polymorphic return type"
    );
    let params_str = params.join(", ");
    let call_args = params
        .iter()
        .map(|p| p.split(':').next().unwrap_or(p).trim())
        .collect::<Vec<_>>()
        .join(", ");

    let mut generated = Vec::new();

    // 1. Generate interface
    let iface = format!(
        "export interface {base_name} {{\n    {method_name}({params_str}): {ret_type};\n}}\n"
    );
    generated.push(iface);

    // 2. Generate concrete classes
    for b in &block.branches {
        if b.is_default && b.body.trim().contains("throw") {
            continue;
        }
        let v_name = format!("{}{base_name}", b.variant_name);
        let indented_body = if b.body.trim().is_empty() {
            "        // default implementation".to_string()
        } else {
            b.body
                .lines()
                .map(|l| format!("        {}", l.trim()))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let cls = format!(
            "export class {v_name} implements {base_name} {{\n    {method_name}({params_str}): {ret_type} {{\n{indented_body}\n    }}\n}}\n"
        );
        generated.push(cls);
    }

    // 3. Replacement
    let return_prefix = if returns_value { "return " } else { "" };
    let replacement = format!("{indent}{return_prefix}{target_var}.{method_name}({call_args});");
    out.replace_range(block.start_offset..block.end_offset, &replacement);

    // 4. Prepend declarations at top
    let decls = format!("{}\n\n", generated.join("\n"));
    out.insert_str(0, &decls);

    Ok(out)
}

/// Core transformation for C++.
pub fn transform_cpp(
    text: &str,
    block: &ConditionalBlock,
    base_name: &str,
    method_name: &str,
    params: &[String],
    return_type: Option<&str>,
    target_var: &str,
) -> Result<String> {
    let mut out = text.to_string();
    let indent = &block.indent;
    let returns_value = returns_from_conditional(block)?;

    let ret_type = return_type.unwrap_or("void");
    anyhow::ensure!(
        returns_value || ret_type == "void",
        "a statement conditional cannot be assigned a non-void polymorphic return type"
    );
    let params_str = params.join(", ");
    let call_args = params
        .iter()
        .map(|p| p.split_whitespace().last().unwrap_or(p).trim())
        .collect::<Vec<_>>()
        .join(", ");

    let mut generated = Vec::new();

    // 1. Abstract base class
    let base_cls = format!(
        "class {base_name} {{\npublic:\n    virtual ~{base_name}() = default;\n    virtual {ret_type} {method_name}({params_str}) = 0;\n}};\n"
    );
    generated.push(base_cls);

    // 2. Concrete classes
    for b in &block.branches {
        if b.is_default && b.body.trim().contains("throw") {
            continue;
        }
        let v_name = format!("{}{base_name}", b.variant_name);
        let indented_body = b
            .body
            .lines()
            .map(|l| format!("        {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        let cls = format!(
            "class {v_name} : public {base_name} {{\npublic:\n    {ret_type} {method_name}({params_str}) override {{\n{indented_body}\n    }}\n}};\n"
        );
        generated.push(cls);
    }

    // 3. Replacement
    let arrow_or_dot = if target_var.contains('*') || target_var.contains("ptr") {
        "->"
    } else {
        "."
    };
    let return_prefix = if returns_value { "return " } else { "" };
    let replacement =
        format!("{indent}{return_prefix}{target_var}{arrow_or_dot}{method_name}({call_args});");
    out.replace_range(block.start_offset..block.end_offset, &replacement);

    // 4. Prepend classes
    let decls = format!("{}\n\n", generated.join("\n"));
    out.insert_str(0, &decls);

    Ok(out)
}

/// Core transformation for Swift.
pub fn transform_swift(
    text: &str,
    block: &ConditionalBlock,
    base_name: &str,
    method_name: &str,
    params: &[String],
    return_type: Option<&str>,
    target_var: &str,
) -> Result<String> {
    let mut out = text.to_string();
    let indent = &block.indent;
    let returns_value = returns_from_conditional(block)?;
    anyhow::ensure!(
        returns_value || return_type.is_none_or(|ty| ty == "Void" || ty == "void"),
        "a statement conditional cannot be assigned a non-void polymorphic return type"
    );

    let ret_type_clause = returns_value
        .then_some(return_type)
        .flatten()
        .map(|rt| format!(" -> {rt}"))
        .unwrap_or_default();
    let params_str = params.join(", ");
    let call_args = params
        .iter()
        .map(|p| {
            let name = p.split(':').next().unwrap_or(p).trim();
            format!("{name}: {name}")
        })
        .collect::<Vec<_>>()
        .join(", ");

    let mut generated = Vec::new();

    // 1. Protocol
    let proto = format!(
        "protocol {base_name} {{\n    func {method_name}({params_str}){ret_type_clause}\n}}\n"
    );
    generated.push(proto);

    // 2. Structs implementing protocol
    for b in &block.branches {
        if b.is_default && b.body.trim().contains("fatalError") {
            continue;
        }
        let v_name = format!("{}{base_name}", b.variant_name);
        let indented_body = b
            .body
            .lines()
            .map(|l| format!("        {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        let s = format!(
            "struct {v_name}: {base_name} {{\n    func {method_name}({params_str}){ret_type_clause} {{\n{indented_body}\n    }}\n}}\n"
        );
        generated.push(s);
    }

    // 3. Replacement
    let return_prefix = if returns_value { "return " } else { "" };
    let replacement = format!("{indent}{return_prefix}{target_var}.{method_name}({call_args})");
    out.replace_range(block.start_offset..block.end_offset, &replacement);

    // 4. Prepend protocol and structs
    let decls = format!("{}\n\n", generated.join("\n"));
    out.insert_str(0, &decls);

    Ok(out)
}

/// Core transformation for Rust.
pub fn transform_rust(
    text: &str,
    block: &ConditionalBlock,
    base_name: &str,
    method_name: &str,
    params: &[String],
    return_type: Option<&str>,
    target_var: &str,
) -> Result<String> {
    let mut out = text.to_string();
    let indent = &block.indent;

    let ret_type_clause = return_type
        .map(|rt| format!(" -> {rt}"))
        .unwrap_or_default();
    let params_str = if params.is_empty() {
        "&self".to_string()
    } else {
        format!("&self, {}", params.join(", "))
    };
    let call_args = params
        .iter()
        .map(|p| p.split(':').next().unwrap_or(p).trim())
        .collect::<Vec<_>>()
        .join(", ");

    let mut generated = Vec::new();

    // 1. Trait
    let trt = format!(
        "pub trait {base_name} {{\n    fn {method_name}({params_str}){ret_type_clause};\n}}\n"
    );
    generated.push(trt);

    // 2. Structs + Impl
    for b in &block.branches {
        if b.is_default && b.body.trim().contains("panic") {
            continue;
        }
        let v_name = format!("{}{base_name}", b.variant_name);
        let indented_body = b
            .body
            .lines()
            .map(|l| format!("        {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        let item = format!(
            "pub struct {v_name};\nimpl {base_name} for {v_name} {{\n    fn {method_name}({params_str}){ret_type_clause} {{\n{indented_body}\n    }}\n}}\n"
        );
        generated.push(item);
    }

    // 3. Replacement
    let replacement = format!("{indent}{target_var}.{method_name}({call_args})");
    out.replace_range(block.start_offset..block.end_offset, &replacement);

    // 4. Prepend trait + impls
    let decls = format!("{}\n\n", generated.join("\n"));
    out.insert_str(0, &decls);

    Ok(out)
}

/// Orchestrator for replacing conditional with polymorphism.
#[allow(clippy::too_many_arguments)]
pub async fn replace_conditional_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    base_name: &str,
    method_name: &str,
    params: &[String],
    return_type: Option<&str>,
    target_var_opt: Option<&str>,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ReplaceConditionalResult> {
    anyhow::ensure!(
        line > 0 && col > 0,
        "replace_conditional requires one-based line and character coordinates"
    );
    let file_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;
    let language = crate::lang::language_id_for_path(file).to_string();

    let offset = line_col_to_offset(&file_text, line, col);

    // Locate the conditional block
    let block = parse_switch_block(&file_text, offset)
        .or_else(|| parse_rust_match(&file_text, offset))
        .or_else(|| parse_if_else_block(&file_text, offset))
        .with_context(|| {
            format!(
                "no switch, match, or if-else conditional block found in {} around line {line}",
                file.display()
            )
        })?;

    let target_var = target_var_opt
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .context("`target_var` is required; the discriminator is not necessarily the polymorphic receiver")?;

    let transformed_text = match language.as_str() {
        "python" => transform_python(
            &file_text,
            &block,
            base_name,
            method_name,
            params,
            target_var,
        )?,
        "typescript" | "javascript" => transform_typescript(
            &file_text,
            &block,
            base_name,
            method_name,
            params,
            return_type,
            target_var,
        )?,
        "cpp" | "c" => transform_cpp(
            &file_text,
            &block,
            base_name,
            method_name,
            params,
            return_type,
            target_var,
        )?,
        "swift" => transform_swift(
            &file_text,
            &block,
            base_name,
            method_name,
            params,
            return_type,
            target_var,
        )?,
        "rust" => transform_rust(
            &file_text,
            &block,
            base_name,
            method_name,
            params,
            return_type,
            target_var,
        )?,
        other => bail!("unsupported language for replace_conditional: {other}"),
    };

    let diff = similar::TextDiff::from_lines(&file_text, &transformed_text)
        .unified_diff()
        .context_radius(2)
        .header(&file.to_string_lossy(), &file.to_string_lossy())
        .to_string();

    let overlays = vec![(file.to_path_buf(), transformed_text.clone())];

    // Overlay validation
    let reports = crate::diagnostics::validate_texts(remote, root, &overlays, &[])
        .await
        .unwrap_or_default();
    let mut diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect();

    let mut verified = false;
    if verify == Some("compile") {
        let files_to_compile: Vec<(String, String)> = overlays
            .iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t.clone()))
            .collect();
        let check = crate::compile_check::check(remote, root, &files_to_compile).await?;
        verified = check.passed;
        if !check.passed {
            if !force {
                bail!("compiler verification failed:\n{}", check.errors.join("\n"));
            }
            diagnostics.push(format!("compiler errors: {}", check.errors.join("; ")));
        }
    }

    let has_fatal = !diagnostics.is_empty();
    if has_fatal && !force && apply {
        bail!(
            "refactoring rejected by validation:\n{}",
            diagnostics.join("\n")
        );
    }

    let applied = if apply {
        let mut file_map = BTreeMap::new();
        file_map.insert(file.to_path_buf(), transformed_text);
        let edit = crate::signature::whole_file_edit(&file_map);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        true
    } else {
        false
    };

    let variant_names = block
        .branches
        .iter()
        .map(|b| b.variant_name.clone())
        .collect();

    Ok(ReplaceConditionalResult {
        base_name: base_name.to_string(),
        method_name: method_name.to_string(),
        variants: variant_names,
        files_modified: vec![file.to_string_lossy().into_owned()],
        overlays,
        diff,
        applied,
        verified,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ts_switch_replace_conditional() {
        let ts_code = r#"export function getSpeed(type: string): number {
    switch (type) {
        case "EUROPEAN":
            return 10;
        case "AFRICAN":
            return 8;
        default:
            throw new Error("Unknown");
    }
}
"#;
        let block = parse_switch_block(ts_code, 0).unwrap();
        assert_eq!(block.kind, ConditionalKind::Switch);
        assert_eq!(block.discriminator, "type");
        assert_eq!(block.branches.len(), 3);
        assert_eq!(block.branches[0].variant_name, "European");
        assert_eq!(block.branches[1].variant_name, "African");

        let res = transform_typescript(
            ts_code,
            &block,
            "Bird",
            "getSpeed",
            &[],
            Some("number"),
            "bird",
        )
        .unwrap();

        assert!(res.contains("export interface Bird {"));
        assert!(res.contains("getSpeed(): number;"));
        assert!(res.contains("export class EuropeanBird implements Bird {"));
        assert!(res.contains("return 10;"));
        assert!(res.contains("export class AfricanBird implements Bird {"));
        assert!(res.contains("return 8;"));
        assert!(res.contains("return bird.getSpeed();"));
    }

    #[test]
    fn switch_parser_ignores_keywords_in_comments_strings_and_identifiers() {
        let source = r#"function report(kind: string) {
    const switcheroo = "switch (fake) { case 'bad': }";
    // switch (alsoFake) { case "bad": }
    switch (kind) {
        case "real":
            reportReal();
            break;
        default:
            reportOther();
    }
}
"#;
        let block = parse_switch_block(source, 0).unwrap();
        assert_eq!(block.discriminator, "kind");
        assert_eq!(block.branches.len(), 2);
        assert!(
            block
                .branches
                .iter()
                .all(|branch| !branch.body.contains("bad"))
        );
    }

    #[test]
    fn statement_conditional_keeps_following_execution() {
        let source = r#"function report(kind: string) {
    switch (kind) {
        case "real":
            logReal();
            break;
        default:
            logOther();
    }
    cleanup();
}
"#;
        let block = parse_switch_block(source, 0).unwrap();
        let transformed =
            transform_typescript(source, &block, "Reporter", "report", &[], None, "reporter")
                .unwrap();
        assert!(
            transformed.contains("reporter.report();\n    cleanup();"),
            "{transformed}"
        );
        assert!(
            !transformed.contains("return reporter.report();"),
            "{transformed}"
        );
    }

    #[test]
    fn statement_conditionals_in_other_languages_also_preserve_following_execution() {
        let python = "def run(kind):\n    if kind == 'A':\n        log_a()\n    else:\n        log_other()\n    cleanup()\n";
        let python_block = parse_if_else_block(python, 0).unwrap();
        let transformed =
            transform_python(python, &python_block, "Handler", "handle", &[], "handler").unwrap();
        assert!(transformed.contains("handler.handle()"), "{transformed}");
        assert!(transformed.contains("cleanup()"), "{transformed}");
        assert!(
            !transformed.contains("return handler.handle()"),
            "{transformed}"
        );

        let cpp = "void run(int kind) {\n    switch (kind) {\n        case 1: log_a(); break;\n        default: log_other();\n    }\n    cleanup();\n}\n";
        let cpp_block = parse_switch_block(cpp, 0).unwrap();
        let transformed =
            transform_cpp(cpp, &cpp_block, "Handler", "handle", &[], None, "handler").unwrap();
        assert!(transformed.contains("handler.handle()"), "{transformed}");
        assert!(transformed.contains("cleanup();"), "{transformed}");
        assert!(
            !transformed.contains("return handler.handle()"),
            "{transformed}"
        );

        let swift = "func run(kind: Int) {\n    switch kind {\n    case 1: logA()\n    default: logOther()\n    }\n    cleanup()\n}\n";
        let swift_block = parse_switch_block(swift, 0).unwrap();
        let transformed = transform_swift(
            swift,
            &swift_block,
            "Handler",
            "handle",
            &[],
            None,
            "handler",
        )
        .unwrap();
        assert!(transformed.contains("handler.handle()"), "{transformed}");
        assert!(transformed.contains("cleanup()"), "{transformed}");
        assert!(
            !transformed.contains("return handler.handle()"),
            "{transformed}"
        );
    }

    #[test]
    fn test_python_if_elif_replace_conditional() {
        let py_code = r#"def calculate_pay(employee_type, salary, bonus):
    if employee_type == "ENGINEER":
        return salary
    elif employee_type == "MANAGER":
        return salary + bonus
    else:
        raise ValueError("Unknown")
"#;
        let block = parse_if_else_block(py_code, 0).unwrap();
        assert_eq!(block.kind, ConditionalKind::IfElse);
        assert_eq!(block.discriminator, "employee_type");
        assert_eq!(block.branches.len(), 3);

        let res = transform_python(
            py_code,
            &block,
            "Employee",
            "calculate_pay",
            &["salary".to_string(), "bonus".to_string()],
            "employee",
        )
        .unwrap();

        assert!(res.contains("class Employee:"));
        assert!(res.contains("def calculate_pay(self, salary, bonus):"));
        assert!(res.contains("class EngineerEmployee(Employee):"));
        assert!(res.contains("return salary"));
        assert!(res.contains("class ManagerEmployee(Employee):"));
        assert!(res.contains("return salary + bonus"));
        assert!(res.contains("return employee.calculate_pay(salary, bonus)"));
    }

    #[test]
    fn test_cpp_switch_replace_conditional() {
        let cpp_code = r#"double calculateSpeed(BirdType type) {
    switch (type) {
        case EUROPEAN:
            return 10.0;
        case AFRICAN:
            return 8.0;
        default:
            throw std::invalid_argument("Unknown");
    }
}
"#;
        let block = parse_switch_block(cpp_code, 0).unwrap();
        assert_eq!(block.branches.len(), 3);
        let res = transform_cpp(
            cpp_code,
            &block,
            "Bird",
            "getSpeed",
            &[],
            Some("double"),
            "bird",
        )
        .unwrap();
        assert!(res.contains("class Bird {"));
        assert!(res.contains("virtual double getSpeed() = 0;"));
        assert!(res.contains("class EuropeanBird : public Bird {"));
        assert!(res.contains("return 10.0;"));
        assert!(res.contains("return bird.getSpeed();"));
    }

    #[test]
    fn test_swift_switch_replace_conditional() {
        let swift_code = r#"func getSpeed(type: BirdType) -> Double {
    switch type {
    case .european:
        return 10.0
    case .african:
        return 8.0
    default:
        fatalError("Unknown")
    }
}
"#;
        let block = parse_switch_block(swift_code, 0).unwrap();
        assert_eq!(block.branches.len(), 3);
        let res = transform_swift(
            swift_code,
            &block,
            "Bird",
            "getSpeed",
            &[],
            Some("Double"),
            "bird",
        )
        .unwrap();
        assert!(res.contains("protocol Bird {"));
        assert!(res.contains("func getSpeed() -> Double"));
        assert!(res.contains("struct EuropeanBird: Bird {"));
        assert!(res.contains("return bird.getSpeed()"));
    }

    #[test]
    fn test_rust_match_replace_conditional() {
        let rust_code = r#"fn calculate_speed(bird: &BirdType) -> u32 {
    match bird {
        BirdType::European => 10,
        BirdType::African => 8,
        _ => panic!("Unknown"),
    }
}
"#;
        let block = parse_rust_match(rust_code, 0).unwrap();
        assert_eq!(block.discriminator, "bird");
        assert_eq!(block.branches.len(), 3);
        assert_eq!(block.branches[0].variant_name, "European");
        assert_eq!(block.branches[1].variant_name, "African");
        assert_eq!(block.branches[2].variant_name, "Default");

        let res = transform_rust(
            rust_code,
            &block,
            "Bird",
            "get_speed",
            &[],
            Some("u32"),
            "bird",
        )
        .unwrap();
        assert!(res.contains("pub trait Bird {"));
        assert!(res.contains("fn get_speed(&self) -> u32;"));
        assert!(res.contains("pub struct EuropeanBird;"));
        assert!(res.contains("impl Bird for EuropeanBird {"));
        assert!(res.contains("pub struct AfricanBird;"));
        assert!(res.contains("bird.get_speed()"));
    }
}
