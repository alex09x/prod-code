/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::pattern::{parse_replacement, tokenize_pattern};

/// Supported file extensions for polyglot structural codemods.
pub const CODE_EXTENSIONS: &[&str] = &[
    "rs", "go", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "cpp", "cc", "cxx", "c", "hpp", "h",
    "swift", "java", "kt", "kts", "cs", "scala", "zig", "nim", "d", "php", "rb", "dart", "lua",
    "ex", "exs",
];

/// Directories to skip during workspace-wide codemod traversal.
pub const SKIPPED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    ".build",
    ".cargo",
    "vendor",
    ".svn",
    ".hg",
];

/// Token kinds produced by the structural polyglot tokenizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Ident(String),
    StringLit(String),
    NumberLit(String),
    Punct(String),
    OpenDelim(char),
    CloseDelim(char),
}

/// A source token with exact byte offsets.
#[derive(Debug, Clone)]
pub struct SourceToken {
    pub kind: TokenKind,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// A token in a compiled pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternToken {
    Literal(TokenKind),
    Metavar(String),
}

/// A token in a compiled replacement template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplacementToken {
    Text(String),
    Metavar(String),
}

/// A compiled structural AST pattern for read-only search or codemod matching.
#[derive(Debug, Clone)]
pub struct CompiledPattern {
    pub raw: String,
    pub pattern_tokens: Vec<PatternToken>,
    /// Non-metavariable literal identifiers that MUST exist in a candidate file.
    pub required_literals: Vec<String>,
}

impl CompiledPattern {
    /// Parse and compile a structural pattern string (e.g. `$a.unwrap()`).
    pub fn parse(pattern: &str) -> Result<Self> {
        let pattern_raw = pattern.trim();
        if pattern_raw.is_empty() {
            bail!("pattern cannot be empty");
        }
        let pattern_tokens = tokenize_pattern(pattern_raw)?;
        let mut required_literals = Vec::new();
        for tok in &pattern_tokens {
            if let PatternToken::Literal(TokenKind::Ident(name)) = tok
                && name.len() >= 2
                && !name.starts_with('$')
            {
                required_literals.push(name.clone());
            }
        }
        required_literals.sort();
        required_literals.dedup();

        Ok(Self {
            raw: pattern.to_string(),
            pattern_tokens,
            required_literals,
        })
    }
}

/// A parsed and compiled structural codemod rule: `pattern ==>> replacement`.
#[derive(Debug, Clone)]
pub struct CodemodRule {
    pub raw: String,
    pub pattern_tokens: Vec<PatternToken>,
    pub replacement_tokens: Vec<ReplacementToken>,
    /// Non-metavariable literal identifiers that MUST exist in a candidate file.
    pub required_literals: Vec<String>,
}

impl CodemodRule {
    /// Parse a `pattern ==>> replacement` rule string.
    pub fn parse(rule: &str) -> Result<Self> {
        let parts: Vec<&str> = rule.split("==>>").collect();
        if parts.len() != 2 {
            bail!("a rule must be `pattern ==>> replacement`, got: {rule}");
        }
        let pattern_raw = parts[0].trim();
        let replacement_raw = parts[1].trim();
        if pattern_raw.is_empty() {
            bail!("pattern in rule cannot be empty");
        }

        let pattern = CompiledPattern::parse(pattern_raw)?;
        let replacement_tokens = parse_replacement(replacement_raw);

        Ok(Self {
            raw: rule.to_string(),
            pattern_tokens: pattern.pattern_tokens,
            replacement_tokens,
            required_literals: pattern.required_literals,
        })
    }
}

/// One matched AST span and its computed replacement string.
#[derive(Debug, Clone)]
pub struct CodemodMatch {
    pub start_byte: usize,
    pub end_byte: usize,
    pub replacement: String,
}

pub type MatchBindings = BTreeMap<String, (usize, usize)>;
pub type PatternMatch = (usize, usize, usize, MatchBindings);

/// The result of executing a polyglot structural AST codemod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemodOutcome {
    pub rule: String,
    pub files_scanned: usize,
    pub files_matched: usize,
    pub total_matches: usize,
    pub changed_lines: usize,
    pub diff: String,
    pub rewritten_files: Vec<(PathBuf, String)>,
    pub elapsed_ms: f64,
}

impl CodemodOutcome {
    /// Render human-readable summary and unified diff preview.
    pub fn render(&self, max_diff_chars: usize) -> String {
        let mut text = format!("`{}`\n", self.rule);
        text.push_str(&format!(
            "{} changed line(s) in {} file(s) ({} scanned in {:.2}ms)\n\n",
            self.changed_lines, self.files_matched, self.files_scanned, self.elapsed_ms
        ));

        if self.diff.is_empty() {
            text.push_str("matches nothing\n");
            return text;
        }

        if self.diff.len() > max_diff_chars {
            let cut: String = self.diff.chars().take(max_diff_chars).collect();
            text.push_str(&cut);
            text.push_str("\n… diff truncated\n");
        } else {
            text.push_str(&self.diff);
        }
        text
    }
}

/// Helper to convert a byte offset into 1-based (line, column).
pub fn byte_to_line_col(source: &str, byte_offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in source.char_indices() {
        if i >= byte_offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// One matched AST span in a structural search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StructuralMatchItem {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub matched_text: String,
    pub bindings: BTreeMap<String, String>,
}

/// The result of executing a read-only structural AST search across files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StructuralSearchResult {
    pub pattern: String,
    pub files_scanned: usize,
    pub files_matched: usize,
    pub total_matches: usize,
    pub matches: Vec<StructuralMatchItem>,
    pub elapsed_ms: f64,
}

impl StructuralSearchResult {
    /// Render human-readable summary of structural search matches.
    pub fn render(&self, max_items: usize) -> String {
        let mut text = format!("⚡ prod-code Structural AST Search: `{}`\n", self.pattern);
        text.push_str("────────────────────────────────────────────────────\n");
        text.push_str(&format!(
            "{} match(es) in {} file(s) ({} scanned in {:.2}ms)\n\n",
            self.total_matches, self.files_matched, self.files_scanned, self.elapsed_ms
        ));

        if self.matches.is_empty() {
            text.push_str("✓ No matches found for pattern.\n");
            return text;
        }

        for m in self.matches.iter().take(max_items) {
            let first_line = m
                .matched_text
                .lines()
                .next()
                .unwrap_or(&m.matched_text)
                .trim();
            text.push_str(&format!(
                "  • {}:{}:{}  {}\n",
                m.file, m.line, m.col, first_line
            ));
            if !m.bindings.is_empty() {
                let binds: Vec<String> = m
                    .bindings
                    .iter()
                    .map(|(k, v)| format!("${k} = {v}"))
                    .collect();
                text.push_str(&format!("    └─ [{}]\n", binds.join(", ")));
            }
        }
        if self.matches.len() > max_items {
            text.push_str(&format!(
                "\n  … and {} more match(es) truncated\n",
                self.matches.len() - max_items
            ));
        }
        text
    }
}
