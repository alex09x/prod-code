/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};

use crate::parameter_object::{Language, matching_bracket};

use crate::signature_polyglot::syntax::{
    extract_decl_name_from_line, find_python_body_close, is_ident, is_import_or_export_context,
    is_in_comment,
};
use crate::signature_polyglot::types::PolyglotDecl;

pub fn find_polyglot_declaration(
    text: &str,
    lang: Language,
    line: u32,
    _col: u32,
) -> Result<PolyglotDecl> {
    let lines: Vec<&str> = text.lines().collect();
    let l_idx = (line.saturating_sub(1) as usize).min(lines.len().saturating_sub(1));

    let mut candidate_name = None;
    if let Some(name) = extract_decl_name_from_line(lines[l_idx], lang) {
        candidate_name = Some(name);
    } else {
        let start_idx = l_idx.saturating_sub(10);
        for line in lines[start_idx..l_idx].iter().rev() {
            if let Some(name) = extract_decl_name_from_line(line, lang) {
                candidate_name = Some(name);
                break;
            }
        }
        if candidate_name.is_none() {
            let end_idx = (l_idx + 4).min(lines.len().saturating_sub(1));
            for line in &lines[l_idx + 1..=end_idx] {
                if let Some(name) = extract_decl_name_from_line(line, lang) {
                    candidate_name = Some(name);
                    break;
                }
            }
        }
    }

    let clean_name = candidate_name.context("could not determine function declaration name")?;

    let needle_paren = format!("{clean_name}(");
    let needle_space_paren = format!("{clean_name} (");
    let needle_generic = format!("{clean_name}<");

    let target_offset = text.lines().take(l_idx).map(|l| l.len() + 1).sum::<usize>();

    let mut candidates = Vec::new();
    for (pos, _) in text
        .match_indices(&needle_paren)
        .chain(text.match_indices(&needle_space_paren))
        .chain(text.match_indices(&needle_generic))
    {
        if pos > 0 {
            let prev = text[..pos].chars().next_back().unwrap();
            if is_ident(prev) {
                continue;
            }
        }
        let line_start = text[..pos].rfind('\n').map_or(0, |p| p + 1);
        let header_prefix = &text[line_start..pos];
        if is_in_comment(text, pos, lang) || is_import_or_export_context(text, pos, lang) {
            continue;
        }

        let is_decl = match lang {
            Language::Python => header_prefix.contains("def "),
            Language::Swift => header_prefix.contains("func "),
            Language::Go => header_prefix.contains("func "),
            Language::TypeScript | Language::JavaScript => {
                header_prefix.contains("function ")
                    || header_prefix.trim_start().starts_with("async ")
                    || (header_prefix.trim().chars().all(is_ident)
                        && !header_prefix.trim().is_empty())
            }
            Language::Cpp | Language::C | Language::Java => {
                let last = header_prefix.split_whitespace().last().unwrap_or("");
                !matches!(last, "return" | "throw" | "case" | "sizeof" | "")
                    && !header_prefix.trim_end().ends_with([
                        '=', '(', '[', ',', '?', ':', '!', '+', '-', '*', '/', '%', '&', '|', '^',
                    ])
            }
            _ => true,
        };
        if !is_decl {
            continue;
        }

        let after_name = pos + clean_name.len();
        let open_paren = match text[after_name..].find('(') {
            Some(p) => after_name + p,
            None => continue,
        };
        let close_paren = match matching_bracket(text, open_paren) {
            Some(p) => p,
            None => continue,
        };

        let (body_open, body_close, ret_span) = if lang == Language::Python {
            let colon = match text[close_paren..].find(':') {
                Some(c) => close_paren + c,
                None => continue,
            };
            let b_close = find_python_body_close(text, pos, colon);
            let ret = (close_paren < colon).then_some((close_paren + 1, colon));
            (colon, b_close, ret)
        } else {
            let b_open = match text[close_paren..].find('{') {
                Some(b) => close_paren + b,
                None => continue,
            };
            let b_close = match matching_bracket(text, b_open) {
                Some(b) => b,
                None => continue,
            };
            let ret = match lang {
                Language::TypeScript | Language::JavaScript | Language::Swift | Language::Go => {
                    Some((close_paren + 1, b_open))
                }
                Language::Cpp | Language::C | Language::Java => {
                    let line_start = text[..pos].rfind('\n').map_or(0, |p| p + 1);
                    Some((line_start, pos))
                }
                _ => None,
            };
            (b_open, b_close, ret)
        };

        let is_async =
            header_prefix.contains("async ") || text[close_paren..body_open].contains("async");

        let async_keyword_span = header_prefix
            .find("async ")
            .map(|idx| (line_start + idx, line_start + idx + 6));

        let visibility_span = if let Some(idx) = header_prefix.find("export ") {
            Some((line_start + idx, line_start + idx + 7))
        } else if let Some(idx) = header_prefix.find("public ") {
            Some((line_start + idx, line_start + idx + 7))
        } else {
            header_prefix
                .find("private ")
                .map(|idx| (line_start + idx, line_start + idx + 8))
        };

        let (receiver, params) =
            crate::parameter_object::parse_params(&text[open_paren + 1..close_paren], lang);

        let dist = pos.abs_diff(target_offset);
        candidates.push((
            dist,
            PolyglotDecl {
                fn_name: clean_name.clone(),
                open_paren,
                close_paren,
                body_open,
                body_close,
                receiver,
                params,
                ret_span,
                is_async,
                async_keyword_span,
                visibility_span,
            },
        ));
    }

    candidates.sort_by_key(|(d, _)| *d);
    let found_decl = candidates.into_iter().next().map(|(_, decl)| decl);

    found_decl.with_context(|| format!("could not locate declaration for function at line {line}"))
}
