/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashSet;

use super::lang::{Language, mentions};
use super::tokenize::{is_keyword, tokenize_polyglot};
use super::types::{OutputKind, PolyTokenKind};

pub fn analyze_outputs(
    selection: &str,
    scope_after: &str,
    scope_before: &str,
    lang: Language,
) -> OutputKind {
    let trimmed = selection.trim();

    if trimmed.starts_with("return ") || trimmed.starts_with("return\n") {
        return OutputKind::EndsWithReturn;
    }

    let tokens = tokenize_polyglot(trimmed, lang);
    let has_semi = trimmed.contains(';');
    let has_stmt_kw = tokens.iter().any(|t| {
        matches!(
            t.text.as_str(),
            "let"
                | "const"
                | "var"
                | "val"
                | "def"
                | "func"
                | "fun"
                | "if"
                | "while"
                | "for"
                | "return"
                | "import"
        )
    });
    let has_assign = tokens
        .iter()
        .any(|t| matches!(t.text.as_str(), "=" | ":=" | "+=" | "-=" | "*=" | "/="));

    if !has_semi && !has_stmt_kw && !has_assign {
        return OutputKind::Expression(trimmed.to_string());
    }

    let mut assigned = Vec::new();
    let mut seen = HashSet::new();
    for (i, t) in tokens.iter().enumerate() {
        if matches!(t.text.as_str(), "let" | "const" | "var" | "val") && i + 1 < tokens.len() {
            let next = &tokens[i + 1];
            if next.kind == PolyTokenKind::Word
                && !is_keyword(&next.text, lang)
                && seen.insert(next.text.clone())
            {
                assigned.push(next.text.clone());
            }
        } else if matches!(t.text.as_str(), ":=" | "=" | "+=" | "-=" | "*=" | "/=") && i > 0 {
            let prev = &tokens[i - 1];
            if prev.kind == PolyTokenKind::Word
                && !is_keyword(&prev.text, lang)
                && seen.insert(prev.text.clone())
            {
                assigned.push(prev.text.clone());
            }
        }
    }

    let used_after: Vec<String> = assigned
        .into_iter()
        .filter(|v| mentions(scope_after, v))
        .collect();

    match used_after.len() {
        0 => OutputKind::Void,
        1 => {
            let name = used_after[0].clone();
            let is_new = !mentions(scope_before, &name);
            OutputKind::SingleVar { name, is_new }
        }
        _ => OutputKind::MultipleVars(used_after),
    }
}

pub fn reindent_body(body: &str, target_indent_spaces: usize, lang: Language) -> String {
    let lines: Vec<&str> = body.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').count())
        .min()
        .unwrap_or(0);

    let target_prefix = if lang == Language::Go || body.contains('\t') {
        let tabs = target_indent_spaces.div_ceil(4).max(1);
        "\t".repeat(tabs)
    } else {
        " ".repeat(target_indent_spaces)
    };

    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }
        let stripped = if line.len() >= min_indent {
            &line[min_indent..]
        } else {
            line.trim_start()
        };
        out.push_str(&target_prefix);
        out.push_str(stripped);
        if idx + 1 < lines.len() {
            out.push('\n');
        }
    }
    out
}
