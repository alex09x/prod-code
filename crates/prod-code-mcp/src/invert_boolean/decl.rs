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
use anyhow::{Context, Result};

use super::syntax::is_ident;

pub struct PolyglotPredDecl {
    pub fn_name: String,
    pub decl_name_at: usize,
    pub close_paren: usize,
    pub body_open: usize,
    pub body_close: usize,
}

pub fn is_function_decl(line: &str, lang: Language, name: &str) -> bool {
    let trimmed = line.trim();
    match lang {
        Language::Go => {
            if let Some(rest) = trimmed.strip_prefix("func ") {
                if rest.starts_with('(') {
                    if let Some(close) = rest.find(')') {
                        let after_recv = rest[close + 1..].trim_start();
                        after_recv.starts_with(name)
                    } else {
                        false
                    }
                } else {
                    rest.starts_with(name)
                }
            } else {
                false
            }
        }
        Language::Python => {
            let rest = trimmed.strip_prefix("async ").unwrap_or(trimmed);
            rest.strip_prefix("def ")
                .is_some_and(|after| after.trim_start().starts_with(name))
        }
        Language::Swift => {
            trimmed.contains("func ")
                && crate::inline_parameter::extract_decl_name_from_line(trimmed, lang).as_deref()
                    == Some(name)
        }
        Language::TypeScript | Language::JavaScript => {
            let rest = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            let rest = rest.strip_prefix("default ").unwrap_or(rest);
            let rest = rest.strip_prefix("async ").unwrap_or(rest);
            if let Some(after) = rest.strip_prefix("function ") {
                after.trim_start().starts_with(name)
            } else {
                !trimmed.starts_with("const ")
                    && !trimmed.starts_with("let ")
                    && !trimmed.starts_with("var ")
                    && !trimmed.starts_with("return ")
                    && crate::inline_parameter::extract_decl_name_from_line(trimmed, lang)
                        .as_deref()
                        == Some(name)
            }
        }
        Language::Cpp | Language::C => {
            !trimmed.starts_with("return ")
                && !trimmed.contains('=')
                && crate::inline_parameter::extract_decl_name_from_line(trimmed, lang).as_deref()
                    == Some(name)
        }
        _ => false,
    }
}

pub fn find_polyglot_predicate_declaration(
    text: &str,
    lang: Language,
    line: Option<u32>,
    symbol: Option<&str>,
) -> Result<PolyglotPredDecl> {
    let clean_name = symbol
        .map(|s| {
            s.rsplit_once("::")
                .map(|(_, m)| m)
                .or_else(|| s.rsplit_once('.').map(|(_, m)| m))
                .unwrap_or(s)
                .trim()
                .to_string()
        })
        .or_else(|| {
            let l = line?;
            let lines: Vec<&str> = text.lines().collect();
            if l == 0 || l as usize > lines.len() {
                return None;
            }
            let target_idx = (l - 1) as usize;
            let start_idx = target_idx.saturating_sub(3);
            let end_idx = (target_idx + 3).min(lines.len().saturating_sub(1));
            for i in (start_idx..=end_idx).rev() {
                if let Some(name) =
                    crate::inline_parameter::extract_decl_name_from_line(lines[i], lang)
                {
                    return Some(name);
                }
            }
            None
        })
        .context("could not determine predicate name to invert")?;

    let needle_paren = format!("{clean_name}(");
    let needle_space = format!("{clean_name} (");
    let needle_generic = format!("{clean_name}<");

    let mut candidates = Vec::new();
    for (pos, _) in text
        .match_indices(&needle_paren)
        .chain(text.match_indices(&needle_space))
        .chain(text.match_indices(&needle_generic))
    {
        if pos > 0 {
            let prev = text[..pos].chars().next_back().unwrap();
            if is_ident(prev) {
                continue;
            }
        }
        let after_name = pos + clean_name.len();
        let open_paren = match text[after_name..].find('(') {
            Some(p) => after_name + p,
            None => continue,
        };
        let close_paren = match crate::parameter_object::matching_bracket(text, open_paren) {
            Some(p) => p,
            None => continue,
        };

        let (body_open, body_close) = if lang == Language::Python {
            let colon = match text[close_paren..].find(':') {
                Some(c) => close_paren + c,
                None => continue,
            };
            let b_close = crate::inline_parameter::find_python_body_close(text, pos, colon);
            (colon, b_close)
        } else {
            let b_open = match text[close_paren..].find('{') {
                Some(b) => close_paren + b,
                None => continue,
            };
            let b_close = match crate::parameter_object::matching_bracket(text, b_open) {
                Some(b) => b,
                None => continue,
            };
            (b_open, b_close)
        };

        let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
        let line_text = text[line_start..].lines().next().unwrap_or("");
        let is_decl = is_function_decl(line_text, lang, &clean_name);
        let pos_line = text[..pos].split('\n').count() as u32;

        candidates.push((
            is_decl,
            pos_line,
            PolyglotPredDecl {
                fn_name: clean_name.clone(),
                decl_name_at: pos,
                close_paren,
                body_open,
                body_close,
            },
        ));
    }

    let best = if let Some(target_line) = line {
        candidates
            .into_iter()
            .min_by_key(|(is_decl, l, _)| {
                let dist = (*l as i64 - target_line as i64).abs();
                (!*is_decl, dist)
            })
            .map(|(_, _, decl)| decl)
    } else {
        candidates
            .into_iter()
            .min_by_key(|(is_decl, _, _)| !*is_decl)
            .map(|(_, _, decl)| decl)
    };

    best.with_context(|| format!("could not find declaration for predicate `{clean_name}`"))
}
