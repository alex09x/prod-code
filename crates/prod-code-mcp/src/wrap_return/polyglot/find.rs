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
use crate::wrap_return::types::PolyglotFuncDecl;
use crate::wrap_return::utils::{cpp_return_type_span, is_ident};
use anyhow::{Context, Result};

/// Finds the declaration info for a function in non-Rust languages.
pub fn find_polyglot_decl(
    text: &str,
    lang: Language,
    symbol: Option<&str>,
    line: Option<u32>,
) -> Result<PolyglotFuncDecl> {
    let clean_name = symbol
        .map(|f| {
            f.rsplit_once("::")
                .map(|(_, m)| m)
                .or_else(|| f.rsplit_once('.').map(|(_, m)| m))
                .unwrap_or(f)
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
        .context("could not determine function name to wrap return value for")?;

    let target_line_start = line.and_then(|wanted| {
        if wanted == 0 {
            return None;
        }
        let mut offset = 0usize;
        for (index, source_line) in text.lines().enumerate() {
            if index + 1 == wanted as usize {
                return Some(offset);
            }
            offset += source_line.len() + 1;
        }
        None
    });

    for (name_idx, _) in text.match_indices(&clean_name) {
        if name_idx > 0 && text[..name_idx].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let after_name = &text[name_idx + clean_name.len()..];
        if after_name.chars().next().is_some_and(is_ident) {
            continue;
        }

        let line_start = text[..name_idx].rfind('\n').map_or(0, |p| p + 1);
        let before_on_line = text[line_start..name_idx].trim_start();
        if crate::inline_parameter::is_in_comment(text, name_idx, lang)
            || crate::inline_parameter::is_in_string(text, name_idx, lang)
        {
            continue;
        }

        let line_trimmed = before_on_line.trim_start();
        if line_trimmed.starts_with("import ")
            || line_trimmed.starts_with("import{")
            || line_trimmed.starts_with("from ")
            || line_trimmed.starts_with("export {")
            || line_trimmed.starts_with("export{")
            || line_trimmed.starts_with("export *")
            || line_trimmed.starts_with("use ")
            || line_trimmed.starts_with("#include")
        {
            continue;
        }

        // Multi-line import or export statement context
        let search_start = name_idx.saturating_sub(500);
        let before_sub = &text[search_start..name_idx];
        if let Some(imp_pos) = before_sub
            .rfind("import ")
            .or_else(|| before_sub.rfind("export {"))
        {
            let between = &before_sub[imp_pos..];
            if between.contains('{') && !between.contains('}') {
                continue;
            }
        }

        // Skip call/expression contexts
        let before_trimmed = before_on_line.trim_end();
        if before_trimmed.ends_with('=')
            || before_trimmed.ends_with('+')
            || before_trimmed.ends_with('-')
            || before_trimmed.ends_with('*')
            || before_trimmed.ends_with('/')
            || before_trimmed.ends_with(',')
            || before_trimmed.ends_with('(')
            || (before_trimmed.ends_with(':') && !before_trimmed.ends_with("::"))
            || before_trimmed.ends_with("return")
            || before_trimmed.ends_with("throw")
            || before_trimmed.ends_with("await")
        {
            continue;
        }

        // Check if followed by `(` (or generic `<...>(`)
        let trimmed_after = after_name.trim_start();
        let open_paren = if trimmed_after.starts_with('(') {
            name_idx + clean_name.len() + (after_name.len() - trimmed_after.len())
        } else if trimmed_after.starts_with('<') {
            if let Some(end_gen) = trimmed_after.find('>') {
                let rest_after_gen = trimmed_after[end_gen + 1..].trim_start();
                if rest_after_gen.starts_with('(') {
                    name_idx + clean_name.len() + (after_name.len() - rest_after_gen.len())
                } else {
                    continue;
                }
            } else {
                continue;
            }
        } else if (lang == Language::TypeScript || lang == Language::JavaScript)
            && (before_on_line.starts_with("const ")
                || before_on_line.starts_with("let ")
                || before_on_line.starts_with("var "))
        {
            // Arrow function e.g. `const fn = (...) =>`
            let Some(eq_pos) = after_name.find('=') else {
                continue;
            };
            let after_eq = after_name[eq_pos + 1..].trim_start();
            let after_async = after_eq
                .strip_prefix("async ")
                .unwrap_or(after_eq)
                .trim_start();
            if after_async.starts_with('(') {
                name_idx + clean_name.len() + (after_name.len() - after_async.len())
            } else {
                continue;
            }
        } else {
            continue;
        };

        let Some(close_paren) = crate::parameter_object::matching_bracket(text, open_paren) else {
            continue;
        };

        let decl_start = text[line_start..name_idx]
            .rfind(['{', ';'])
            .map_or(line_start, |i| line_start + i + 1);
        let is_async = text[decl_start..open_paren].contains("async");

        if lang == Language::Python {
            let Some(colon_rel) = text[close_paren..].find(':') else {
                continue;
            };
            let colon = close_paren + colon_rel;
            let between = text[close_paren + 1..colon].trim();
            let (was, ret_span, has_return_type) = if let Some(arr_pos) = between.find("->") {
                let r = between[arr_pos + 2..].trim();
                let abs_s =
                    close_paren + 1 + (text[close_paren + 1..colon].find("->").unwrap() + 2);
                let abs_start =
                    abs_s + (text[abs_s..colon].len() - text[abs_s..colon].trim_start().len());
                let abs_end =
                    colon - (text[abs_s..colon].len() - text[abs_s..colon].trim_end().len());
                (r.to_string(), Some((abs_start, abs_end)), true)
            } else {
                ("None".to_string(), None, false)
            };
            let body_open = colon;
            let body_close =
                crate::inline_parameter::find_python_body_close(text, decl_start, colon);
            if target_line_start
                .is_some_and(|selected| selected < decl_start || selected > body_close)
            {
                continue;
            }
            return Ok(PolyglotFuncDecl {
                name: clean_name,
                decl_start,
                name_start: name_idx,
                close_paren,
                body_open,
                body_close,
                was,
                ret_span,
                is_async,
                is_arrow: false,
                has_return_type,
            });
        }

        // C-style braces
        let Some(open_brace_rel) = text[close_paren..].find('{') else {
            continue;
        };
        let open_brace = close_paren + open_brace_rel;
        let Some(body_close) = crate::parameter_object::matching_bracket(text, open_brace) else {
            continue;
        };
        if target_line_start.is_some_and(|selected| selected < decl_start || selected > body_close)
        {
            continue;
        }

        let header_slice = &text[close_paren + 1..open_brace];
        if header_slice.contains(';') {
            continue;
        }
        let is_arrow = header_slice.contains("=>");

        let (was, ret_span, has_return_type) = match lang {
            Language::TypeScript | Language::JavaScript => {
                let end_header = if is_arrow {
                    close_paren + 1 + header_slice.find("=>").unwrap()
                } else {
                    open_brace
                };
                if let Some(c_rel) = text[close_paren + 1..end_header].find(':') {
                    let c_pos = close_paren + 1 + c_rel;
                    let ret_raw = text[c_pos + 1..end_header].trim();
                    let s_start = c_pos
                        + 1
                        + (text[c_pos + 1..end_header].len()
                            - text[c_pos + 1..end_header].trim_start().len());
                    let s_end = end_header
                        - (text[c_pos + 1..end_header].len()
                            - text[c_pos + 1..end_header].trim_end().len());
                    (ret_raw.to_string(), Some((s_start, s_end)), true)
                } else {
                    let def_ret = if lang == Language::TypeScript {
                        "void"
                    } else {
                        ""
                    };
                    (def_ret.to_string(), None, false)
                }
            }
            Language::Swift => {
                if let Some(arr_pos) = header_slice.find("->") {
                    let ret_raw = header_slice[arr_pos + 2..].trim();
                    let s_pos = close_paren + 1 + arr_pos + 2;
                    let s_start = s_pos
                        + (text[s_pos..open_brace].len()
                            - text[s_pos..open_brace].trim_start().len());
                    let s_end = open_brace
                        - (text[s_pos..open_brace].len()
                            - text[s_pos..open_brace].trim_end().len());
                    (ret_raw.to_string(), Some((s_start, s_end)), true)
                } else {
                    ("Void".to_string(), None, false)
                }
            }
            Language::Go => {
                let ret_raw = header_slice.trim();
                if !ret_raw.is_empty() {
                    let s_pos = close_paren + 1;
                    let s_start = s_pos + (header_slice.len() - header_slice.trim_start().len());
                    let s_end = open_brace - (header_slice.len() - header_slice.trim_end().len());
                    (ret_raw.to_string(), Some((s_start, s_end)), true)
                } else {
                    (String::new(), None, false)
                }
            }
            Language::Cpp | Language::C | Language::Java => {
                let (ret_raw, ret_span) = cpp_return_type_span(text, decl_start, name_idx);
                (ret_raw, ret_span, ret_span.is_some())
            }
            Language::Python | Language::Rust => unreachable!(),
        };

        return Ok(PolyglotFuncDecl {
            name: clean_name,
            decl_start,
            name_start: name_idx,
            close_paren,
            body_open: open_brace,
            body_close,
            was,
            ret_span,
            is_async,
            is_arrow,
            has_return_type,
        });
    }

    anyhow::bail!("function declaration `{clean_name}` not found")
}
