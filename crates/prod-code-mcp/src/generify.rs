/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Making a parameter generic: a concrete type becomes a type parameter with the bound it needs.
//!
//! `fn total(v: &Vec<u32>)` becomes `fn total<T: AsRef<[u32]>>(v: &T)`. The callers do not change —
//! the type argument is inferred from what they pass — but they are checked: a caller passing a type
//! that does not satisfy the bound, or a body that uses something the bound does not promise, is an
//! error in the overlay before anything is written. The bound is the caller's to choose; which trait
//! a function *should* ask for is a design decision, not something the text can say.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use crate::parameter_object::Language;

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Generified {
    pub function: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub was: String,
    pub now: String,
    /// Files that call the function, checked against the new signature.
    pub callers_checked: usize,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Generified {
    pub fn render(&self) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: `{}`\n- now: `{}`\n- {} file(s) that call it checked against the \
             new signature; their calls do not change, the type argument is inferred\n",
            self.function, self.file, self.was, self.now, self.callers_checked
        );
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str(
                "\nthe analyzer rejects the result — the body uses something the bound does not \
                 promise, or a caller passes a type that does not satisfy it or can no longer be \
                 inferred:\n",
            );
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        if self.applied {
            out.push_str("\n[applied]\n");
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make this edit\n");
        }
        out
    }
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn is_ident(c: char) -> bool {
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

/// Makes the parameter `param` of the function declared at `line`:`col` of `file` generic, as a
/// type parameter `type_param` bounded by `bound`.
#[allow(clippy::too_many_arguments)]
pub async fn generify(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    param: &str,
    bound: &str,
    type_param: &str,
    apply: bool,
    force: bool,
) -> Result<Generified> {
    generify_polyglot(
        remote,
        root,
        file,
        None,
        Some(line),
        Some(col),
        param,
        bound,
        type_param,
        apply,
        force,
    )
    .await
}

#[derive(Debug, Clone)]
struct PolyglotFuncDecl {
    name: String,
    decl_start: usize,
    _name_start: usize,
    name_end: usize,
    open_paren: usize,
    close_paren: usize,
    has_generics: bool,
    generics_span: Option<(usize, usize)>,
}

fn find_polyglot_func_decl(
    text: &str,
    lang: Language,
    symbol: Option<&str>,
    line: Option<u32>,
) -> Result<PolyglotFuncDecl> {
    let clean_name = symbol.map(|s| {
        s.rsplit_once("::")
            .map(|(_, m)| m)
            .or_else(|| s.rsplit_once('.').map(|(_, m)| m))
            .unwrap_or(s)
            .trim()
            .to_string()
    }).or_else(|| {
        let l = line?;
        let lines: Vec<&str> = text.lines().collect();
        let target_idx = (l.saturating_sub(1)) as usize;
        let start_idx = target_idx.saturating_sub(2);
        let end_idx = (target_idx + 2).min(lines.len().saturating_sub(1));
        for i in (start_idx..=end_idx).rev() {
            if let Some(name) = crate::inline_parameter::extract_decl_name_from_line(lines[i], lang) {
                return Some(name);
            }
        }
        None
    }).context("could not determine function name to generify parameter for")?;

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
            let candidate_line = text[..line_start].bytes().filter(|b| *b == b'\n').count() as u32 + 1;
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
                if let Some(bracket_close) = crate::parameter_object::matching_bracket(text, bracket_open) {
                    has_generics = true;
                    gen_span = Some((bracket_open + 1, bracket_close));
                    let rest = text[bracket_close + 1..].trim_start();
                    if rest.starts_with('(') {
                        open_paren = Some(bracket_close + 1 + (text[bracket_close + 1..].len() - rest.len()));
                    }
                }
            } else if trimmed_after.starts_with('(') {
                open_paren = Some(name_end + (after_name.len() - trimmed_after.len()));
            }
        } else if lang == Language::TypeScript || lang == Language::JavaScript || lang == Language::Swift {
            if trimmed_after.starts_with('<') {
                let angle_open = name_end + (after_name.len() - trimmed_after.len());
                if let Some(angle_close) = matching_angle_bracket(text, angle_open) {
                    has_generics = true;
                    gen_span = Some((angle_open + 1, angle_close));
                    let rest = text[angle_close + 1..].trim_start();
                    if rest.starts_with('(') {
                        open_paren = Some(angle_close + 1 + (text[angle_close + 1..].len() - rest.len()));
                    }
                }
            } else if trimmed_after.starts_with('(') {
                open_paren = Some(name_end + (after_name.len() - trimmed_after.len()));
            } else if (before_on_line.starts_with("const ") || before_on_line.starts_with("let ") || before_on_line.starts_with("var "))
                && after_name.contains('=')
            {
                let eq_pos = after_name.find('=').unwrap();
                let after_eq = after_name[eq_pos + 1..].trim_start();
                let after_async = after_eq.strip_prefix("async ").unwrap_or(after_eq).trim_start();
                if after_async.starts_with('<') {
                    let a_open = name_end + (after_name.len() - after_async.len());
                    if let Some(a_close) = matching_angle_bracket(text, a_open) {
                        has_generics = true;
                        gen_span = Some((a_open + 1, a_close));
                        let rest_a = text[a_close + 1..].trim_start();
                        if rest_a.starts_with('(') {
                            open_paren = Some(a_close + 1 + (text[a_close + 1..].len() - rest_a.len()));
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
        let Some(close_p) = crate::parameter_object::matching_bracket(text, open_p) else { continue };

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

fn rewrite_param_entry(
    entry_text: &str,
    target_param: &crate::parameter_object::Param,
    type_param: &str,
    lang: Language,
) -> String {
    match lang {
        Language::TypeScript | Language::JavaScript => {
            if let Some(colon) = entry_text.find(':') {
                let before_colon = &entry_text[..colon];
                let after_colon = &entry_text[colon + 1..];
                let (ty_str, default_str) = crate::parameter_object::split_default(after_colon.trim(), Language::TypeScript);
                let trimmed_ty = ty_str.trim();
                let new_ty = if trimmed_ty.ends_with("[]") {
                    format!("{type_param}[]")
                } else if let Some(angle_open) = trimmed_ty.find('<') {
                    if trimmed_ty.ends_with('>') {
                        let container = &trimmed_ty[..angle_open];
                        format!("{container}<{type_param}>")
                    } else {
                        type_param.to_string()
                    }
                } else {
                    type_param.to_string()
                };
                let mut out = format!("{before_colon}: {new_ty}");
                if let Some(def) = default_str {
                    out.push_str(&format!(" = {def}"));
                }
                out
            } else if let Some(eq) = entry_text.find('=') {
                let before_eq = entry_text[..eq].trim_end();
                let after_eq = &entry_text[eq..];
                format!("{before_eq}: {type_param} {after_eq}")
            } else {
                format!("{}: {type_param}", target_param.name)
            }
        }
        Language::Python => {
            if let Some(colon) = entry_text.find(':') {
                let before_colon = &entry_text[..colon];
                let after_colon = &entry_text[colon + 1..];
                let (ty_str, default_str) = crate::parameter_object::split_default(after_colon.trim(), Language::Python);
                let trimmed_ty = ty_str.trim();
                let new_ty = if let Some(bracket_open) = trimmed_ty.find('[') {
                    if trimmed_ty.ends_with(']') {
                        let container = &trimmed_ty[..bracket_open];
                        format!("{container}[{type_param}]")
                    } else {
                        type_param.to_string()
                    }
                } else {
                    type_param.to_string()
                };
                let mut out = format!("{before_colon}: {new_ty}");
                if let Some(def) = default_str {
                    out.push_str(&format!(" = {def}"));
                }
                out
            } else if let Some(eq) = entry_text.find('=') {
                let before_eq = entry_text[..eq].trim_end();
                let after_eq = &entry_text[eq..];
                format!("{before_eq}: {type_param} {after_eq}")
            } else {
                format!("{}: {type_param}", target_param.name)
            }
        }
        Language::Swift => {
            if let Some(colon) = entry_text.find(':') {
                let before_colon = &entry_text[..colon];
                let after_colon = &entry_text[colon + 1..];
                let (ty_str, default_str) = crate::parameter_object::split_default(after_colon.trim(), Language::Swift);
                let trimmed_ty = ty_str.trim();
                let new_ty = if trimmed_ty.ends_with('?') {
                    format!("{type_param}?")
                } else if trimmed_ty.starts_with('[') && trimmed_ty.ends_with(']') {
                    format!("[{type_param}]")
                } else if let Some(rest) = trimmed_ty.strip_prefix("inout ") {
                    let _ = rest;
                    format!("inout {type_param}")
                } else {
                    type_param.to_string()
                };
                let mut out = format!("{before_colon}: {new_ty}");
                if let Some(def) = default_str {
                    out.push_str(&format!(" = {def}"));
                }
                out
            } else {
                format!("{}: {type_param}", target_param.name)
            }
        }
        Language::Go => {
            if let Some(ref old_ty) = target_param.ty {
                let trimmed_ty = old_ty.trim();
                let new_ty = if trimmed_ty.starts_with('*') {
                    format!("*{type_param}")
                } else if trimmed_ty.starts_with("[]") {
                    format!("[]{type_param}")
                } else if trimmed_ty.starts_with("...") {
                    format!("...{type_param}")
                } else {
                    type_param.to_string()
                };
                entry_text.replace(trimmed_ty, &new_ty)
            } else {
                format!("{} {type_param}", target_param.name)
            }
        }
        Language::Cpp | Language::C | Language::Java => {
            let (decl_part, default_part) = if let Some(eq) = entry_text.find('=') {
                (&entry_text[..eq], Some(&entry_text[eq..]))
            } else {
                (entry_text, None)
            };
            let name_match = decl_part
                .match_indices(&target_param.name)
                .filter(|(idx, _)| {
                    let before_ok = *idx == 0 || !is_ident(decl_part[..*idx].chars().last().unwrap());
                    let after_ok = *idx + target_param.name.len() == decl_part.len()
                        || !is_ident(decl_part[*idx + target_param.name.len()..].chars().next().unwrap());
                    before_ok && after_ok
                })
                .last();
            if let Some((name_pos, _)) = name_match {
                let ty_part = decl_part[..name_pos].trim_end();
                let words: Vec<&str> = ty_part.split_whitespace().collect();
                let mut rewritten_words = Vec::new();
                let mut replaced = false;
                for w in words {
                    let clean = w.trim_matches(|c: char| !is_ident(c) && c != ':');
                    if !replaced && clean != "const" && clean != "volatile" && clean != "struct" && clean != "class" && !clean.is_empty() {
                        let replaced_w = w.replace(clean, type_param);
                        rewritten_words.push(replaced_w);
                        replaced = true;
                    } else {
                        rewritten_words.push(w.to_string());
                    }
                }
                let mut out = format!("{} {}", rewritten_words.join(" "), decl_part[name_pos..].trim());
                if let Some(def) = default_part {
                    out.push_str(def);
                }
                out
            } else {
                entry_text.to_string()
            }
        }
        Language::Rust => unreachable!(),
    }
}

/// Unified generify refactoring across Rust, TypeScript, JavaScript, Python, C++, Swift, and Go.
#[allow(clippy::too_many_arguments)]
pub async fn generify_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    param: &str,
    bound: &str,
    type_param: &str,
    apply: bool,
    force: bool,
) -> Result<Generified> {
    let lang = crate::parameter_object::Language::of(file)
        .with_context(|| format!("unsupported language for {}", file.display()))?;

    if lang == Language::Rust {
        let (l, c) = match (line, col) {
            (Some(l), Some(c)) => (l, c),
            (Some(l), None) => (l, 1),
            _ => {
                let text = std::fs::read_to_string(file)
                    .with_context(|| format!("cannot read {}", file.display()))?;
                let sym = symbol.context("missing `symbol` or `line`")?;
                let mut found_pos = None;
                for (name_idx, _) in text.match_indices(sym) {
                    let line_start = text[..name_idx].rfind('\n').map_or(0, |p| p + 1);
                    let before = text[line_start..name_idx].trim();
                    if before.ends_with("fn") || before.ends_with("pub fn") {
                        let (nl, nc) = crate::signature::position_at(&text, name_idx)?;
                        found_pos = Some((nl, nc));
                        break;
                    }
                }
                found_pos.with_context(|| format!("could not find declaration of `{sym}` in {}", file.display()))?
            }
        };
        return generify_rust(
            remote, root, file, l, c, param, bound, type_param, apply, force,
        )
        .await;
    }

    anyhow::ensure!(
        !type_param.is_empty() && type_param.chars().all(is_ident),
        "`{type_param}` is not a type parameter name"
    );
    let bound = bound.trim();

    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read {}", file.display()))?;

    let decl = find_polyglot_func_decl(&text, lang, symbol, line)?;

    // Check collision with existing generics
    if let Some((gs, ge)) = decl.generics_span {
        let existing = &text[gs..ge];
        anyhow::ensure!(
            !existing.split(|c: char| !is_ident(c)).any(|w| w == type_param),
            "`{}` already has a generic parameter `{type_param}`; pass another `type_param`",
            decl.name
        );
    }

    let list = &text[decl.open_paren + 1..decl.close_paren];
    let (_, params) = crate::parameter_object::parse_params(list, lang);
    let target_idx = params
        .iter()
        .position(|p| p.name == param)
        .with_context(|| format!("`{}` has no parameter `{param}`", decl.name))?;
    let target_param = &params[target_idx];

    let entries = crate::parameter_object::entries(list, lang);
    let entry_match = entries
        .iter()
        .find(|(at, entry_text)| *at <= target_param.name_at && target_param.name_at <= *at + entry_text.len());
    let (entry_at, entry_text) = entry_match
        .copied()
        .with_context(|| format!("could not locate parameter `{param}` in parameter list"))?;

    let mut new_list = list.to_string();
    let mut parameter_edits = Vec::new();
    if lang == Language::Go {
        let mut group_start = target_idx;
        while group_start > 0 && params[group_start - 1].shares_type {
            group_start -= 1;
        }
        let grouped = group_start < target_idx || target_param.shares_type;
        if grouped {
            let old_type = target_param
                .ty
                .as_deref()
                .context("cannot determine the shared Go parameter type")?;
            for index in group_start..=target_idx {
                let p = &params[index];
                let (at, raw) = entries
                    .iter()
                    .find(|(at, entry)| *at <= p.name_at && p.name_at <= *at + entry.len())
                    .copied()
                    .with_context(|| format!("could not locate Go parameter `{}`", p.name))?;
                let replacement = if index == target_idx {
                    let synthetic = if p.shares_type {
                        format!("{} {old_type}", p.name)
                    } else {
                        raw.to_string()
                    };
                    rewrite_param_entry(&synthetic, p, type_param, lang)
                } else {
                    format!("{} {old_type}", p.name)
                };
                parameter_edits.push((at, raw.len(), replacement));
            }
        } else {
            parameter_edits.push((
                entry_at,
                entry_text.len(),
                rewrite_param_entry(entry_text, target_param, type_param, lang),
            ));
        }
    } else {
        parameter_edits.push((
            entry_at,
            entry_text.len(),
            rewrite_param_entry(entry_text, target_param, type_param, lang),
        ));
    }
    parameter_edits.sort_by_key(|(at, _, _)| std::cmp::Reverse(*at));
    for (at, len, replacement) in parameter_edits {
        new_list.replace_range(at..at + len, &replacement);
    }

    let was = text[decl.decl_start..decl.close_paren + 1].trim().to_string();

    let mut new_text = text.clone();
    new_text.replace_range(decl.open_paren + 1..decl.close_paren, &new_list);

    // Now insert generic parameter declaration
    match lang {
        Language::TypeScript | Language::JavaScript => {
            let bound_spec = if bound.is_empty() || bound == "any" {
                String::new()
            } else {
                format!(" extends {bound}")
            };
            let gen_decl = format!("{type_param}{bound_spec}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                new_text.insert_str(decl.name_end, &format!("<{gen_decl}>"));
            }
        }
        Language::Python => {
            let bound_spec = if bound.is_empty() || bound == "Any" || bound == "object" {
                String::new()
            } else {
                format!(": {bound}")
            };
            let gen_decl = format!("{type_param}{bound_spec}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                new_text.insert_str(decl.name_end, &format!("[{gen_decl}]"));
            }
        }
        Language::Swift => {
            let bound_spec = if bound.is_empty() || bound == "Any" {
                String::new()
            } else {
                format!(": {bound}")
            };
            let gen_decl = format!("{type_param}{bound_spec}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                new_text.insert_str(decl.name_end, &format!("<{gen_decl}>"));
            }
        }
        Language::Go => {
            let bound_spec = if bound.is_empty() { "any" } else { bound };
            let gen_decl = format!("{type_param} {bound_spec}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                new_text.insert_str(decl.name_end, &format!("[{gen_decl}]"));
            }
        }
        Language::Cpp | Language::C => {
            let concept_spec = if bound.is_empty() || bound == "typename" || bound == "class" {
                "typename"
            } else {
                bound
            };
            let gen_decl = format!("{concept_spec} {type_param}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                let line_start = text[..decl.decl_start].rfind('\n').map_or(0, |p| p + 1);
                let indent_len = text[line_start..].len() - text[line_start..].trim_start().len();
                let indent = &text[line_start..line_start + indent_len];
                new_text.insert_str(decl.decl_start, &format!("{indent}template<{gen_decl}>\n"));
            }
        }
        Language::Java => {
            let bound_spec = if bound.is_empty() || bound == "Object" {
                String::new()
            } else {
                format!(" extends {bound}")
            };
            let gen_decl = format!("{type_param}{bound_spec}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                new_text.insert_str(decl.decl_start, &format!("<{gen_decl}> "));
            }
        }
        Language::Rust => unreachable!(),
    }

    let new_open_p = new_text[decl.decl_start..].find('(').map(|i| decl.decl_start + i).unwrap_or(decl.decl_start);
    let new_close_p = crate::parameter_object::matching_bracket(&new_text, new_open_p).unwrap_or(new_open_p);
    let now = new_text[decl.decl_start..new_close_p + 1].trim().to_string();

    let mut rewritten = vec![(file.to_string_lossy().into_owned(), new_text.clone())];

    let mut separate_cpp_header = false;
    if matches!(lang, Language::Cpp | Language::C) {
        let concept_spec = if bound.is_empty() || bound == "typename" || bound == "class" {
            "typename"
        } else {
            bound
        };
        let gen_decl = format!("{concept_spec} {type_param}");

        for entry in ignore::WalkBuilder::new(root).build().flatten() {
            let p = entry.path();
            if !p.is_file() || p == file {
                continue;
            }
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext, "h" | "hpp" | "hh" | "hxx") {
                continue;
            }
            let Ok(proto_content) = std::fs::read_to_string(p) else { continue };
            if !proto_content.contains(&decl.name) {
                continue;
            }

            if let Ok(proto_decl) = find_polyglot_func_decl(&proto_content, lang, Some(&decl.name), None) {
                let proto_list = &proto_content[proto_decl.open_paren + 1..proto_decl.close_paren];
                let (_, proto_params) = crate::parameter_object::parse_params(proto_list, lang);
                if let Some(target_proto_param) = proto_params.iter().find(|pr| pr.name == param) {
                    separate_cpp_header = true;
                    let proto_entries = crate::parameter_object::entries(proto_list, lang);
                    if let Some((pr_at, pr_text)) = proto_entries
                        .iter()
                        .find(|(at, entry_text)| *at <= target_proto_param.name_at && target_proto_param.name_at <= *at + entry_text.len())
                    {
                        let new_pr_entry = rewrite_param_entry(pr_text, target_proto_param, type_param, lang);
                        let mut new_pr_list = proto_list.to_string();
                        new_pr_list.replace_range(*pr_at..*pr_at + pr_text.len(), &new_pr_entry);

                        let mut new_proto_text = proto_content.clone();
                        new_proto_text.replace_range(proto_decl.open_paren + 1..proto_decl.close_paren, &new_pr_list);

                        if proto_decl.has_generics {
                            if let Some((_, ge)) = proto_decl.generics_span {
                                new_proto_text.insert_str(ge, &format!(", {gen_decl}"));
                            }
                        } else {
                            let line_start = proto_content[..proto_decl.decl_start].rfind('\n').map_or(0, |p| p + 1);
                            let indent_len = proto_content[line_start..].len() - proto_content[line_start..].trim_start().len();
                            let indent = &proto_content[line_start..line_start + indent_len];
                            new_proto_text.insert_str(proto_decl.decl_start, &format!("{indent}template<{gen_decl}>\n"));
                        }
                        rewritten.push((p.to_string_lossy().into_owned(), new_proto_text));
                    }
                }
            }
        }
    }
    anyhow::ensure!(
        !separate_cpp_header,
        "cannot safely synchronize this C/C++ signature with another header declaration; semantic declaration identity is not available"
    );

    let mut callers_checked = 0usize;
    let mut caller_files = Vec::new();
    let selected_is_header = matches!(
        file.extension().and_then(|ext| ext.to_str()),
        Some("h" | "hpp" | "hh" | "hxx")
    );
    let mut out_of_line_cpp_definition = false;
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let p = entry.path();
        if !p.is_file() || p == file || !crate::inline_parameter::language_matches(lang, p) {
            continue;
        }
        if let Ok(other_text) = std::fs::read_to_string(p)
            && other_text.contains(&decl.name)
        {
            callers_checked += 1;
            caller_files.push(p.to_path_buf());
            if selected_is_header
                && matches!(lang, Language::Cpp | Language::C)
                && p.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(ext, "cpp" | "cc" | "cxx" | "c"))
            {
                out_of_line_cpp_definition = true;
            }
        }
    }
    anyhow::ensure!(
        !out_of_line_cpp_definition,
        "cannot safely generify a C/C++ declaration whose definition is in another source file; move the definition into the header first"
    );

    let files_to_validate: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &files_to_validate, &caller_files).await?;
    let diagnostics: Vec<String> = reports
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

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. The body \
             needs more than the bound promises, or a caller no longer satisfies it or can no longer \
             infer its type; choose another bound, fix the caller, or pass `force: true`:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let files_map: std::collections::BTreeMap<PathBuf, String> = rewritten
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.clone()))
            .collect();
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files_map))?;
        applied = true;
    }

    Ok(Generified {
        function: decl.name,
        root: root.to_path_buf(),
        file: display(root, file),
        was,
        now,
        callers_checked,
        rewritten,
        diagnostics,
        applied,
    })
}

/// Makes the parameter `param` of the function declared at `line`:`col` of `file` generic in Rust.
#[allow(clippy::too_many_arguments)]
pub async fn generify_rust(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    param: &str,
    bound: &str,
    type_param: &str,
    apply: bool,
    force: bool,
) -> Result<Generified> {
    anyhow::ensure!(
        !type_param.is_empty() && type_param.chars().all(is_ident),
        "`{type_param}` is not a type parameter name"
    );
    let bound = bound.trim();
    anyhow::ensure!(
        !bound.is_empty(),
        "give the `bound` the parameter's type must satisfy"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = crate::signature::offset_of(&text, line, col)
        .context("the position is not inside the file")?;
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    anyhow::ensure!(
        text[..start].trim_end().ends_with("fn"),
        "the position is not the name of a function declaration"
    );
    let (name, open, close) =
        crate::signature::param_span(&text, start).context("the function has no parameter list")?;
    let name_end = start + name.len();

    // The parameter, and the span of its type.
    let list = &text[open..close];
    let mut offset = open;
    let mut found = None;
    for part in crate::signature::split_params(list) {
        let at_in = list[offset - open..]
            .find(part.trim())
            .map(|i| offset + i)
            .unwrap_or(offset);
        let (pattern, ty) = part.split_once(':').unwrap_or((part.as_str(), ""));
        let pattern = pattern.trim().trim_start_matches("mut ").trim();
        if pattern == param {
            let ty_start = at_in + part.trim().find(':').map_or(0, |i| i + 1);
            let lead = text[ty_start..].len() - text[ty_start..].trim_start().len();
            let ty_trim = ty.trim();
            found = Some((
                ty_start + lead,
                ty_start + lead + ty_trim.len(),
                ty_trim.to_string(),
            ));
            break;
        }
        offset = at_in + part.trim().len();
    }
    let (ty_start, ty_end, ty) =
        found.with_context(|| format!("`{name}` has no parameter `{param}`"))?;
    let (reference, concrete) = split_reference(&ty);
    anyhow::ensure!(
        !concrete.starts_with("impl ") && !concrete.starts_with("dyn "),
        "`{param}: {ty}` is already abstract"
    );
    let generics = generics_span(&text, name_end);
    if let Some((g_start, g_end)) = generics {
        let existing = &text[g_start..g_end];
        anyhow::ensure!(
            !existing
                .split(|c: char| !is_ident(c))
                .any(|word| word == type_param),
            "`{name}` already has a generic parameter `{type_param}`; pass another `type_param`"
        );
    }

    let was = text[text[..start].rfind("fn").unwrap_or(start)..close + 1].to_string();
    let mut new_text = text.clone();
    new_text.replace_range(ty_start..ty_end, &format!("{reference}{type_param}"));
    match generics {
        Some((_, g_end)) => {
            let existing = text[..g_end].trim_end();
            let sep = if existing.ends_with('<') || existing.ends_with(',') {
                ""
            } else {
                ", "
            };
            new_text.insert_str(g_end, &format!("{sep}{type_param}: {bound}"));
        }
        None => new_text.insert_str(name_end, &format!("<{type_param}: {bound}>")),
    }
    let fn_at = new_text[..start].rfind("fn").unwrap_or(start);
    let now_end = crate::signature::param_span(&new_text, start)
        .map(|(_, _, c)| c + 1)
        .unwrap_or(new_text.len());
    let now = new_text[fn_at..now_end].to_string();

    // Every file that calls it is checked against the new signature.
    let (nl, nc) = crate::signature::position_at(&text, start)?;
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    // Without them the callers go unchecked, and a clean report would mean nothing (#446).
    let callers: BTreeSet<PathBuf> = crate::signature::references(remote, root, file, nl, nc)
        .await
        .context("cannot find the callers to check against the new signature; nothing was planned")?
        .into_iter()
        .map(|(p, _, _)| p)
        .filter(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) != canonical)
        .collect();
    let also: Vec<PathBuf> = callers.iter().cloned().collect();
    let reports = crate::diagnostics::validate_texts(
        remote,
        root,
        &[(file.to_path_buf(), new_text.clone())],
        &also,
    )
    .await?;
    let diagnostics: Vec<String> = reports
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

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. The body \
             needs more than the bound promises, or a caller no longer satisfies it or can no longer \
             infer its type (an `.into()` that took its target from the old type); choose another \
             bound, fix the caller, or pass `force: true`:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let files: std::collections::BTreeMap<PathBuf, String> =
            std::iter::once((file.to_path_buf(), new_text.clone())).collect();
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }

    Ok(Generified {
        function: name,
        root: root.to_path_buf(),
        file: display(root, file),
        was,
        now,
        callers_checked: callers.len(),
        rewritten: vec![(file.to_string_lossy().into_owned(), new_text)],
        diagnostics,
        applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_selection_respects_the_resolved_declaration_line() {
        let text = "function convert(value: string): string { return value; }\n\
function convert(value: number): number { return value; }";
        let declaration = find_polyglot_func_decl(
            text,
            crate::parameter_object::Language::TypeScript,
            Some("convert"),
            Some(2),
        )
        .unwrap();
        assert_eq!(&text[declaration.open_paren + 1..declaration.close_paren], "value: number");
    }

    #[test]
    fn a_reference_is_kept_in_front_of_the_type_parameter() {
        assert_eq!(
            split_reference("Vec<u32>"),
            (String::new(), "Vec<u32>".to_string())
        );
        assert_eq!(
            split_reference("&Vec<u32>"),
            ("&".to_string(), "Vec<u32>".to_string())
        );
        assert_eq!(
            split_reference("&mut String"),
            ("&mut ".to_string(), "String".to_string())
        );
        assert_eq!(
            split_reference("&'a str"),
            ("&'a ".to_string(), "str".to_string())
        );
        assert_eq!(
            split_reference("&'a mut Buf"),
            ("&'a mut ".to_string(), "Buf".to_string())
        );
    }

    #[test]
    fn the_generic_list_is_found_after_the_name() {
        let t = "fn f<A: Clone, B>(a: A) {}";
        let (s, e) = generics_span(t, 4).unwrap();
        assert_eq!(&t[s..e], "A: Clone, B");
        let t = "fn g<M: Into<Vec<u8>>>(m: M) {}";
        let (s, e) = generics_span(t, 4).unwrap();
        assert_eq!(&t[s..e], "M: Into<Vec<u8>>");
        assert!(generics_span("fn h(x: u8) {}", 4).is_none());
    }

    #[test]
    fn the_report_says_what_the_signature_became_and_who_was_checked() {
        let done = Generified {
            function: "total".into(),
            root: "/root".into(),
            file: "src/lib.rs".into(),
            was: "fn total(v: &Vec<u32>)".into(),
            now: "fn total<T: AsRef<[u32]>>(v: &T)".into(),
            callers_checked: 2,
            rewritten: vec![],
            diagnostics: vec!["no method named `iter` found (src/lib.rs:2:7)".into()],
            applied: false,
        };
        let text = done.render();
        assert!(
            text.contains("now: `fn total<T: AsRef<[u32]>>(v: &T)`"),
            "{text}"
        );
        assert!(text.contains("2 file(s) that call it checked"), "{text}");
        assert!(text.contains("the bound does not"), "{text}");
        assert!(text.contains("nothing was written"), "{text}");
        let mut ok = done.clone();
        ok.diagnostics.clear();
        ok.applied = true;
        assert!(ok.render().contains("0 errors") && ok.render().contains("[applied]"));
    }
}
