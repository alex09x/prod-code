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
use std::path::Path;

use crate::parameter_object::Language;

use super::types::PolyglotFuncDecl;

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A parameter's type split into the reference in front of it (`&`, `&mut `, `&'a `, or nothing)
/// and the type itself.
pub fn split_reference(ty: &str) -> (String, String) {
    let ty = ty.trim();
    let Some(rest) = ty.strip_prefix('&') else {
        return (String::new(), ty.to_string());
    };
    let mut prefix = String::from("&");
    let mut rest = rest.trim_start();
    if rest.starts_with('\'') {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        prefix.push_str(&rest[..end]);
        prefix.push(' ');
        rest = rest[end..].trim_start();
    }
    if let Some(after) = rest.strip_prefix("mut ") {
        prefix.push_str("mut ");
        rest = after.trim_start();
    }
    (prefix, rest.to_string())
}

/// The generic parameter list of a function header: the span inside `<…>` after its name, or `None`
/// when it has none. `name_end` is where the name ends.
pub fn generics_span(text: &str, name_end: usize) -> Option<(usize, usize)> {
    let rest = &text[name_end..];
    let lead = rest.len() - rest.trim_start().len();
    if !rest.trim_start().starts_with('<') {
        return None;
    }
    let open = name_end + lead;
    let mut depth = 0i32;
    for (i, c) in text[open..].char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open + 1, open + i));
                }
            }
            _ => {}
        }
    }
    None
}

/// The offset of the matching `>` for the `<` at `open`.
pub fn matching_angle_bracket(text: &str, open: usize) -> Option<usize> {
    if text.as_bytes().get(open) != Some(&b'<') {
        return None;
    }
    let mut depth = 0i32;
    for (i, c) in text[open..].char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn find_polyglot_func_decl(
    text: &str,
    lang: Language,
    symbol: Option<&str>,
    line: Option<u32>,
) -> Result<PolyglotFuncDecl> {
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
            let target_idx = (l.saturating_sub(1)) as usize;
            let start_idx = target_idx.saturating_sub(2);
            let end_idx = (target_idx + 2).min(lines.len().saturating_sub(1));
            for i in (start_idx..=end_idx).rev() {
                if let Some(name) =
                    crate::inline_parameter::extract_decl_name_from_line(lines[i], lang)
                {
                    return Some(name);
                }
            }
            None
        })
        .context("could not determine function name to generify parameter for")?;

    for (name_idx, _) in text.match_indices(&clean_name) {
        if name_idx > 0 && text[..name_idx].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let after_name = &text[name_idx + clean_name.len()..];
        if after_name.chars().next().is_some_and(is_ident) {
            continue;
        }

        let line_start = text[..name_idx].rfind('\n').map_or(0, |p| p + 1);
        if let Some(target_line) = line {
            let candidate_line =
                text[..line_start].bytes().filter(|b| *b == b'\n').count() as u32 + 1;
            if candidate_line != target_line {
                continue;
            }
        }
        let before_on_line = &text[line_start..name_idx];
        if crate::inline_parameter::is_in_comment(text, name_idx, lang) {
            continue;
        }

        let line_trimmed = before_on_line.trim_start();
        if line_trimmed.starts_with("import ")
            || line_trimmed.starts_with("from ")
            || line_trimmed.starts_with("export {")
            || line_trimmed.starts_with("export *")
            || line_trimmed.starts_with("use ")
            || line_trimmed.starts_with("#include")
        {
            continue;
        }

        let before_trimmed = before_on_line.trim_end();
        if before_trimmed.ends_with('=')
            || before_trimmed.ends_with('+')
            || before_trimmed.ends_with('-')
            || before_trimmed.ends_with('*')
            || before_trimmed.ends_with('/')
            || before_trimmed.ends_with(',')
            || before_trimmed.ends_with('(')
            || before_trimmed.ends_with(':')
            || before_trimmed.ends_with("return")
            || before_trimmed.ends_with("throw")
            || before_trimmed.ends_with("await")
        {
            continue;
        }

        let trimmed_after = after_name.trim_start();
        let name_end = name_idx + clean_name.len();

        let mut has_generics = false;
        let mut gen_span = None;
        let mut template_header_span = None;
        let mut open_paren = None;

        if lang == Language::Go || lang == Language::Python {
            if trimmed_after.starts_with('[') {
                let bracket_open = name_end + (after_name.len() - trimmed_after.len());
                if let Some(bracket_close) =
                    crate::parameter_object::matching_bracket(text, bracket_open)
                {
                    has_generics = true;
                    gen_span = Some((bracket_open + 1, bracket_close));
                    let rest = text[bracket_close + 1..].trim_start();
                    if rest.starts_with('(') {
                        open_paren = Some(
                            bracket_close + 1 + (text[bracket_close + 1..].len() - rest.len()),
                        );
                    }
                }
            } else if trimmed_after.starts_with('(') {
                open_paren = Some(name_end + (after_name.len() - trimmed_after.len()));
            }
        } else if lang == Language::TypeScript
            || lang == Language::JavaScript
            || lang == Language::Swift
        {
            if trimmed_after.starts_with('<') {
                let angle_open = name_end + (after_name.len() - trimmed_after.len());
                if let Some(angle_close) = matching_angle_bracket(text, angle_open) {
                    has_generics = true;
                    gen_span = Some((angle_open + 1, angle_close));
                    let rest = text[angle_close + 1..].trim_start();
                    if rest.starts_with('(') {
                        open_paren =
                            Some(angle_close + 1 + (text[angle_close + 1..].len() - rest.len()));
                    }
                }
            } else if trimmed_after.starts_with('(') {
                open_paren = Some(name_end + (after_name.len() - trimmed_after.len()));
            } else if (before_on_line.starts_with("const ")
                || before_on_line.starts_with("let ")
                || before_on_line.starts_with("var "))
                && after_name.contains('=')
            {
                let eq_pos = after_name.find('=').unwrap();
                let after_eq = after_name[eq_pos + 1..].trim_start();
                let after_async = after_eq
                    .strip_prefix("async ")
                    .unwrap_or(after_eq)
                    .trim_start();
                if after_async.starts_with('<') {
                    let a_open = name_end + (after_name.len() - after_async.len());
                    if let Some(a_close) = matching_angle_bracket(text, a_open) {
                        has_generics = true;
                        gen_span = Some((a_open + 1, a_close));
                        let rest_a = text[a_close + 1..].trim_start();
                        if rest_a.starts_with('(') {
                            open_paren =
                                Some(a_close + 1 + (text[a_close + 1..].len() - rest_a.len()));
                        }
                    }
                } else if after_async.starts_with('(') {
                    open_paren = Some(name_end + (after_name.len() - after_async.len()));
                }
            }
        } else if (lang == Language::Cpp || lang == Language::C) && trimmed_after.starts_with('(') {
            open_paren = Some(name_end + (after_name.len() - trimmed_after.len()));
            let search_start = name_idx.saturating_sub(400);
            let before_decl = &text[search_start..name_idx];
            if let Some(tmpl_pos) = before_decl.rfind("template") {
                let tmpl_abs = search_start + tmpl_pos;
                let after_tmpl = text[tmpl_abs + 8..].trim_start();
                if after_tmpl.starts_with('<') {
                    let angle_open = tmpl_abs + 8 + (text[tmpl_abs + 8..].len() - after_tmpl.len());
                    if let Some(angle_close) = matching_angle_bracket(text, angle_open) {
                        let between_tmpl = &text[angle_close + 1..name_idx];
                        if !between_tmpl.contains(';') && !between_tmpl.contains('}') {
                            has_generics = true;
                            gen_span = Some((angle_open + 1, angle_close));
                            template_header_span = Some((tmpl_abs, angle_close + 1));
                        }
                    }
                }
            }
        }

        let Some(open_p) = open_paren else { continue };
        let Some(close_p) = crate::parameter_object::matching_bracket(text, open_p) else {
            continue;
        };

        let decl_start = template_header_span.map_or(line_start, |(s, _)| s);
        return Ok(PolyglotFuncDecl {
            name: clean_name,
            decl_start,
            _name_start: name_idx,
            name_end,
            open_paren: open_p,
            close_paren: close_p,
            has_generics,
            generics_span: gen_span,
        });
    }

    anyhow::bail!("could not find declaration of `{clean_name}` in file")
}
