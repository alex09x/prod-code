/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;
use std::path::PathBuf;

/// How deep to build nested workspace types before falling back to `Default::default()`.
pub const DEFAULT_DEPTH: u32 = 2;

/// Options for fixture and mock generation.
#[derive(Debug, Clone, Default)]
pub struct FixtureOptions {
    pub depth: u32,
    pub verify: bool,
    pub hint: Option<PathBuf>,
    pub randomized: bool,
    pub mock: bool,
    pub language: Option<Language>,
}

/// A generated fixture and what the analyzer said about it.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub type_name: String,
    /// The value expression, formatted over several lines.
    pub value: String,
    /// The file that declares the type, relative to the workspace root.
    pub file: String,
    /// Types that could not be built field by field and fell back to `Default::default()`.
    pub fallbacks: Vec<String>,
    /// What the analyzer reported for the generated code, empty when it is clean.
    pub diagnostics: Vec<String>,
    pub verified: bool,
    /// Language code block tag (e.g. "rust", "go", "typescript", "python", "cpp", "swift").
    pub language: &'static str,
    /// Whether this output is a mock implementation.
    pub is_mock: bool,
    /// Complete formatted snippet ready to paste.
    pub snippet: String,
}

impl Fixture {
    pub fn render(&self) -> String {
        let label = if self.is_mock { "mock" } else { "fixture" };
        let mut out = format!(
            "{label} for `{}` (declared in {})\n\n```{}\n{}\n```\n",
            self.type_name, self.file, self.language, self.snippet
        );
        if !self.fallbacks.is_empty() {
            let mut names = self.fallbacks.clone();
            names.sort();
            names.dedup();
            let fallback_label = match self.language {
                "rust" => "`Default::default()`",
                "go" => "zero-value",
                "python" => "`None`",
                _ => "default fallback",
            };
            out.push_str(&format!(
                "\n{fallback_label} stands in for: {} (not declared in this workspace, or deeper than the depth limit)\n",
                names.join(", ")
            ));
        }
        match (self.verified, self.diagnostics.is_empty()) {
            (true, true) => out.push_str("\nthe analyzer accepts it: 0 errors\n"),
            (true, false) => {
                out.push_str("\nthe analyzer rejects it:\n");
                for d in &self.diagnostics {
                    out.push_str(&format!("  {d}\n"));
                }
            }
            (false, _) => out.push_str("\nnot verified (pass `verify: true` to type-check it)\n"),
        }
        out
    }
}

/// `SliceReport` -> `slice_report`.
pub fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The shape of a type as the analyzer printed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// A struct with named fields: `(name, type)`.
    Record(Vec<(String, String)>),
    /// A tuple struct: the field types in order.
    Tuple(Vec<String>),
    /// A unit struct.
    Unit,
    /// An enum with each variant's value-bearing shape intact.
    Enum(Vec<EnumVariant>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumVariant {
    pub name: String,
    pub shape: Shape,
}

pub(crate) fn strip_visibility(line: &str) -> &str {
    let t = line.trim_start();
    let Some(rest) = t.strip_prefix("pub") else {
        return t;
    };
    // `public_key` and other identifiers beginning with `pub` are not visibility.
    let separated = rest.is_empty() || rest.starts_with(char::is_whitespace);
    if !separated && !rest.starts_with('(') {
        return t;
    }
    let rest = rest.trim_start();
    if let Some(scope) = rest.strip_prefix('(')
        && let Some((scope, tail)) = scope.split_once(')')
        && (matches!(scope.trim(), "crate" | "super" | "self")
            || scope
                .trim()
                .strip_prefix("in")
                .is_some_and(|s| s.starts_with(char::is_whitespace)))
    {
        return tail.trim_start();
    }
    // After a plain `pub`, parentheses may belong to the field's tuple type.
    // Only the recognized scopes above are restricted visibility.
    if separated { rest } else { t }
}

/// Splits Rust commas outside delimiters, keeping generics and payloads whole.
pub(crate) fn split_top_level(text: &str) -> Vec<String> {
    use super::builder::lexer::{Kind, Source};

    // Keep unreadable input intact for the caller to reject or report as a fallback.
    let Ok(source) = Source::new(text) else {
        return vec![text.to_string()];
    };
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, token) in source.tokens.iter().enumerate() {
        if token.kind != Kind::Punct {
            continue;
        }
        // In particular, `->` is one token, not an angle-bracket closer.
        match source.t(i) {
            "<" | "(" | "[" | "{" => depth += 1,
            ">" | ")" | "]" | "}" => depth -= 1,
            "," if depth == 0 => {
                out.push(text[start..token.start].to_string());
                start = token.end;
            }
            _ => {}
        }
    }
    out.push(text[start..].to_string());
    out
}

/// The generic argument of `Wrapper<T>`, if this is one.
pub(crate) fn inner_of<'a>(ty: &'a str, wrapper: &str) -> Option<&'a str> {
    let t = ty.trim();
    let name = t.split('<').next()?.trim().rsplit("::").next()?;
    if name != wrapper {
        return None;
    }
    let open = t.find('<')?;
    let close = t.rfind('>')?;
    Some(t[open + 1..close].trim())
}
