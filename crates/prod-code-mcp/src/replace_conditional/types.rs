/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;

use anyhow::Result;

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

pub fn returns_from_conditional(block: &ConditionalBlock) -> Result<bool> {
    anyhow::ensure!(!block.branches.is_empty(), "conditional has no branches");
    let mut returns = Vec::with_capacity(block.branches.len());
    for branch in &block.branches {
        let body = branch.body.trim_start();
        let has_return = contains_word(body, "return");
        let starts_with_return = if let Some(rest) = body.strip_prefix("return") {
            rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == ';')
        } else {
            false
        };
        let terminates_without_return = [
            "throw ",
            "throw;",
            "raise ",
            "raise\n",
            "raise\r\n",
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

pub fn contains_word(text: &str, word: &str) -> bool {
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
pub fn line_col_to_offset(text: &str, line: u32, col: u32) -> Option<usize> {
    if line == 0 || col == 0 {
        return None;
    }
    let mut line_offset = 0usize;
    for (index, raw_line) in text.split_inclusive('\n').enumerate() {
        if index as u32 + 1 != line {
            line_offset += raw_line.len();
            continue;
        }
        let line_text = raw_line
            .strip_suffix('\n')
            .unwrap_or(raw_line)
            .strip_suffix('\r')
            .unwrap_or_else(|| raw_line.strip_suffix('\n').unwrap_or(raw_line));
        let target_units = (col - 1) as usize;
        let mut units = 0usize;
        for (byte_offset, ch) in line_text.char_indices() {
            if units == target_units {
                return Some(line_offset + byte_offset);
            }
            let next_units = units + ch.len_utf16();
            if target_units < next_units {
                return None; // The requested column splits a UTF-16 surrogate pair.
            }
            units = next_units;
        }
        return (units == target_units).then_some(line_offset + line_text.len());
    }
    None
}
