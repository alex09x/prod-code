/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::extract_field::helpers::{display, is_ident, is_in_literal_or_comment, mentions};
use crate::extract_field::polyglot::constructors::find_constructors;
use crate::extract_field::polyglot::expression_boundaries::{
    has_complete_expression_boundaries, requires_complete_expression_boundaries,
};
use crate::extract_field::polyglot::parsers::{has_member_named, parse_locals};
use crate::extract_field::polyglot::target::discover_target;
use crate::extract_field::rust::extract;
use crate::extract_field::types::ExtractedField;
use crate::parameter_object::Language;

/// Promotes the expression selected in `file` into a field of the type its method belongs to
/// across TypeScript/JavaScript, Python, Go, Swift, and C++.
#[allow(clippy::too_many_arguments)]
pub async fn extract_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    start: (u32, u32),
    end: (u32, u32),
    name: &str,
    ty: Option<&str>,
    init: Option<&str>,
    replace_all: bool,
    apply: bool,
    force: bool,
) -> Result<ExtractedField> {
    anyhow::ensure!(
        !name.is_empty() && name.chars().all(is_ident),
        "`{name}` is not an identifier"
    );
    let lang = Language::of(file)
        .with_context(|| format!("unsupported language for {}", file.display()))?;
    if lang == Language::Rust {
        return extract(
            remote,
            root,
            file,
            start,
            end,
            name,
            ty,
            init,
            replace_all,
            apply,
            force,
        )
        .await;
    }
    if lang == Language::Java {
        anyhow::bail!("extract_field does not support Java yet; use LSP or IDE assists");
    }

    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let from = crate::signature::offset_of(&text, start.0, start.1)
        .context("the selection does not start inside the file")?;
    let to = crate::signature::offset_of(&text, end.0, end.1)
        .context("the selection does not end inside the file")?;
    anyhow::ensure!(to > from, "the selection is empty");
    let expression = text[from..to].trim().to_string();
    anyhow::ensure!(!expression.is_empty(), "the selection is only whitespace");
    let selected = from + (text[from..to].len() - text[from..to].trim_start().len());
    let selected_end = selected + expression.len();
    let requires_boundaries = requires_complete_expression_boundaries(&expression, lang);
    if requires_boundaries {
        anyhow::ensure!(
            has_complete_expression_boundaries(&text, selected, selected_end, &expression, lang),
            "the selected expression does not cover a complete expression"
        );
    }

    let target = discover_target(&text, lang, from, to)?;
    let owner = target.owner;
    let method = target.method;
    let receiver_name = target.receiver_name;
    let params = target.params;
    let body_open = target.body_open;
    let body_close = target.body_close;
    let class_body_open = target.class_body_open;
    let class_close_line_start = target.class_close_line_start;
    let init_body = target.init_body;
    let is_static_method = target.is_static_method;

    let recv_kw = match lang {
        Language::TypeScript
        | Language::JavaScript
        | Language::Cpp
        | Language::C
        | Language::Java => "this",
        Language::Go => &receiver_name,
        _ => "self",
    };
    let init = match init {
        Some(init) => init.trim().to_string(),
        None => {
            anyhow::ensure!(
                !mentions(&expression, recv_kw),
                "`{expression}` reads `{recv_kw}`, which does not exist yet where `{owner}` is built; \
                 pass `init` with what a new value should start as"
            );
            expression.clone()
        }
    };

    for param in &params {
        anyhow::ensure!(
            !mentions(&init, param),
            "`{expression}` mentions parameter `{param}`, which does not exist where `{owner}` is built; \
             pass `init` with what a new value should start as"
        );
    }

    let locals = parse_locals(&text[body_open..from], lang);
    for local in &locals {
        anyhow::ensure!(
            !mentions(&init, local),
            "`{expression}` mentions local `{local}`, which does not exist where `{owner}` is built; \
             pass `init` with what a new value should start as"
        );
    }

    anyhow::ensure!(
        !has_member_named(&text, lang, &owner, name),
        "`{owner}` already has a field `{name}`"
    );

    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());

    let (insert_offset, insert_text) = match lang {
        Language::TypeScript | Language::JavaScript => {
            let line_after_open = text[class_body_open + 1..]
                .find('\n')
                .map(|i| class_body_open + 1 + i + 1)
                .unwrap_or(class_body_open + 1);
            let decl = match lang {
                Language::TypeScript => {
                    if let Some(t) = ty {
                        if is_static_method {
                            format!("    static {name}: {t} = {init};\n")
                        } else {
                            format!("    {name}: {t} = {init};\n")
                        }
                    } else if is_static_method {
                        format!("    static {name} = {init};\n")
                    } else {
                        format!("    {name} = {init};\n")
                    }
                }
                _ if is_static_method => format!("    static {name} = {init};\n"),
                _ => format!("    {name} = {init};\n"),
            };
            (line_after_open, decl)
        }
        Language::Python => {
            if let Some((_, init_end)) = init_body {
                (init_end, format!("\n        self.{name} = {init}\n"))
            } else {
                let decl = if let Some(t) = ty {
                    format!("    {name}: {t} = {init}\n")
                } else {
                    format!("    {name} = {init}\n")
                };
                (class_body_open, format!("\n{decl}"))
            }
        }
        Language::Go => {
            let ty_str = ty.context(
                "pass the field's `type`: Go requires a type for struct field declarations",
            )?;
            (class_close_line_start, format!("    {name} {ty_str}\n"))
        }
        Language::Swift => {
            let line_after_open = text[class_body_open + 1..]
                .find('\n')
                .map(|i| class_body_open + 1 + i + 1)
                .unwrap_or(class_body_open + 1);
            let decl = if let Some(t) = ty {
                format!("    var {name}: {t} = {init}\n")
            } else {
                format!("    var {name} = {init}\n")
            };
            (line_after_open, decl)
        }
        Language::Cpp | Language::C => {
            let ty_str =
                ty.context("pass the field's `type`: C++ requires a type for member declarations")?;
            (
                class_close_line_start,
                format!("    {ty_str} {name} = {init};\n"),
            )
        }
        Language::Rust | Language::Java => unreachable!(),
    };

    let own_edits = edits.entry(file.to_path_buf()).or_default();
    own_edits.push((insert_offset, 0, insert_text));

    let recv_expr = match lang {
        Language::TypeScript | Language::JavaScript | Language::Java => format!("this.{name}"),
        Language::Python | Language::Swift => format!("self.{name}"),
        Language::Cpp | Language::C => format!("this->{name}"),
        Language::Go => format!("{receiver_name}.{name}"),
        Language::Rust => format!("self.{name}"),
    };

    let replaced = if replace_all {
        own_edits.push((selected, expression.len(), recv_expr.clone()));
        let mut count = 1;
        let mut at = body_open;
        while let Some(i) = text[at..body_close].find(&expression) {
            let hit = at + i;
            let after = hit + expression.len();
            if hit == selected {
                at = after;
                continue;
            }
            if requires_boundaries
                && !has_complete_expression_boundaries(&text, hit, after, &expression, lang)
            {
                at = after;
                continue;
            }
            let left_boundary = expression.chars().next().is_none_or(|c| {
                !is_ident(c) || hit == 0 || !text[..hit].chars().next_back().is_some_and(is_ident)
            });
            let right_boundary = expression.chars().next_back().is_none_or(|c| {
                !is_ident(c)
                    || after >= text.len()
                    || !text[after..].chars().next().is_some_and(is_ident)
            });
            if is_in_literal_or_comment(&text, hit, lang) || !left_boundary || !right_boundary {
                at = hit + expression.len();
                continue;
            }
            own_edits.push((hit, expression.len(), recv_expr.clone()));
            count += 1;
            at = after;
        }
        count
    } else {
        own_edits.push((selected, expression.len(), recv_expr));
        1
    };

    let mut unmatched = Vec::new();
    let constructors = find_constructors(
        root,
        file,
        &text,
        lang,
        &owner,
        name,
        &init,
        &mut edits,
        &mut texts,
        &mut unmatched,
    );

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, _, _)| *at);
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }

    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, &[]).await?;
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
            unmatched.is_empty(),
            "{} constructor reference(s) could not be matched to `{owner}`; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `init`, or \
             `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(ExtractedField {
        owner,
        method,
        root: root.to_path_buf(),
        file: display(root, file),
        name: name.to_string(),
        ty: ty.unwrap_or("").to_string(),
        init,
        replaced,
        constructors,
        blocked: Vec::new(),
        unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
