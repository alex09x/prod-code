/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::extract_function::{Duplicate, Extracted};

use super::codegen::{generate_call_replacement, generate_function_code};
use super::duplicates::find_duplicates_in_text;
use super::lang::{Language, collect_workspace_sources, display, is_ident, mentions};
use super::outputs::analyze_outputs;
use super::scope::{extract_input_variables, find_enclosing_scope, is_balanced};
use super::tokenize::tokenize_polyglot;
use super::types::ExtractedParam;

#[allow(clippy::too_many_arguments)]
pub async fn extract_function_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    (line, col): (u32, u32),
    (end_line, end_col): (u32, u32),
    name: &str,
    duplicates: bool,
    parameterize: bool,
    other_files: bool,
) -> Result<Extracted> {
    anyhow::ensure!(
        !name.is_empty()
            && name.chars().all(is_ident)
            && !name.starts_with(|c: char| c.is_ascii_digit()),
        "`{name}` is not an identifier"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    anyhow::ensure!(
        !mentions(&text, name),
        "the file already has something called `{name}`; choose another name"
    );

    let start =
        crate::signature::offset_of(&text, line, col).context("the start is not in the file")?;
    let end = crate::signature::offset_of(&text, end_line, end_col)
        .context("the end is not in the file")?;
    anyhow::ensure!(start < end, "the selection is empty");
    let selection = text[start..end].trim().to_string();
    anyhow::ensure!(!selection.is_empty(), "the selection is empty");
    anyhow::ensure!(
        is_balanced(&selection),
        "the selection contains an unbalanced delimiter or string"
    );

    let lang = Language::of(file).unwrap_or(Language::TypeScript);

    let (scope_start, scope_end, _enclosing_fn, enclosing_ret, is_method, method_indent) =
        find_enclosing_scope(&text, lang, start, end);
    let scope_before = &text[scope_start..start];
    let scope_after = &text[end..scope_end];

    let mut inputs = extract_input_variables(&selection, scope_before, lang);
    let output = analyze_outputs(&selection, scope_after, scope_before, lang);

    let mut in_file_copies = Vec::new();
    if duplicates {
        in_file_copies =
            find_duplicates_in_text(&selection, &text, Some((start, end)), parameterize, lang);
    }

    let sel_tokens = tokenize_polyglot(&selection, lang);
    let mut param_descriptions = Vec::new();
    let mut param_indices: Vec<(usize, String, String)> = Vec::new();

    if parameterize {
        let mut differing_token_indices: Vec<usize> = in_file_copies
            .iter()
            .flat_map(|c| c.differs.iter().map(|(idx, _)| *idx))
            .collect();
        differing_token_indices.sort_unstable();
        differing_token_indices.dedup();

        for (n, &tok_idx) in differing_token_indices.iter().enumerate() {
            if tok_idx < sel_tokens.len() {
                let orig_lit = sel_tokens[tok_idx].text.clone();
                let param_name = if differing_token_indices.len() == 1 {
                    "value".to_string()
                } else {
                    format!("value{}", n + 1)
                };
                let ty = if orig_lit.starts_with('"') {
                    match lang {
                        Language::Zig => Some("[]const u8".into()),
                        _ => Some("string".into()),
                    }
                } else if orig_lit.starts_with('\'') {
                    match lang {
                        Language::Zig => Some("u8".into()),
                        _ => Some("char".into()),
                    }
                } else {
                    match lang {
                        Language::Go | Language::Cpp | Language::C | Language::Csharp => {
                            Some("int".into())
                        }
                        Language::Swift | Language::Kotlin => Some("Int".into()),
                        Language::Zig => Some("usize".into()),
                        _ => Some("number".into()),
                    }
                };
                inputs.push(ExtractedParam {
                    name: param_name.clone(),
                    ty: ty.clone(),
                });
                let ty_str = ty.unwrap_or_else(|| "any".into());
                param_descriptions.push(format!("{param_name}: {ty_str}"));
                param_indices.push((tok_idx, param_name, orig_lit));
            }
        }
    }

    let fn_selection_body = if param_indices.is_empty() {
        selection.clone()
    } else {
        let mut body_edits: Vec<(usize, usize, String)> = Vec::new();
        for (tok_idx, param_name, _) in &param_indices {
            if let Some(t) = sel_tokens.get(*tok_idx) {
                body_edits.push((t.start, t.end, param_name.clone()));
            }
        }
        apply_edits(&selection, &body_edits)
    };

    let param_names: Vec<String> = inputs.iter().map(|p| p.name.clone()).collect();
    let orig_args: Vec<String> = inputs
        .iter()
        .map(|p| {
            if let Some((_, _, orig_lit)) =
                param_indices.iter().find(|(_, name, _)| name == &p.name)
            {
                orig_lit.clone()
            } else {
                p.name.clone()
            }
        })
        .collect();

    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let mut sel_indent = text[line_start..start]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect::<String>();
    if sel_indent.is_empty() {
        sel_indent = text[start..end]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect::<String>();
    }
    let is_method = is_method || selection.contains("this.") || selection.contains("this ");
    let is_bol =
        text[..start].ends_with('\n') || start == 0 || text[line_start..start].trim().is_empty();

    let mut call_replacement = if is_bol {
        generate_call_replacement(
            name,
            &orig_args,
            &param_names,
            &output,
            lang,
            is_method,
            &sel_indent,
        )
    } else {
        generate_call_replacement(name, &orig_args, &param_names, &output, lang, is_method, "")
    };
    if text[start..end].ends_with('\n') && !call_replacement.ends_with('\n') {
        call_replacement.push('\n');
    }

    let is_exported = other_files || text.contains("export ");
    let fn_code = generate_function_code(
        name,
        &inputs,
        &output,
        &fn_selection_body,
        lang,
        is_method,
        is_exported,
        &method_indent,
        enclosing_ret.as_deref(),
    );

    let mut file_edits: Vec<(usize, usize, String)> = Vec::new();
    file_edits.push((start, end, call_replacement.clone()));

    let mut duplicate_records = Vec::new();
    for copy in &in_file_copies {
        let (dup_line, _) = crate::signature::position_at(&text, copy.start)?;
        let dup_line_start = text[..copy.start].rfind('\n').map_or(0, |i| i + 1);
        let dup_indent = text[dup_line_start..copy.start]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect::<String>();
        let dup_is_bol = text[..copy.start].ends_with('\n')
            || copy.start == 0
            || text[dup_line_start..copy.start].trim().is_empty();
        let copy_args: Vec<String> = inputs
            .iter()
            .map(|p| {
                if let Some((tok_idx, _, _)) =
                    param_indices.iter().find(|(_, name, _)| name == &p.name)
                {
                    if let Some((_, copy_lit)) = copy.differs.iter().find(|(idx, _)| idx == tok_idx)
                    {
                        copy_lit.clone()
                    } else {
                        p.name.clone()
                    }
                } else {
                    p.name.clone()
                }
            })
            .collect();
        let mut dup_call = if dup_is_bol {
            generate_call_replacement(
                name,
                &copy_args,
                &param_names,
                &output,
                lang,
                is_method,
                &dup_indent,
            )
        } else {
            generate_call_replacement(name, &copy_args, &param_names, &output, lang, is_method, "")
        };
        if text[copy.start..copy.end].ends_with('\n') && !dup_call.ends_with('\n') {
            dup_call.push('\n');
        }
        file_edits.push((copy.start, copy.end, dup_call));
        duplicate_records.push(Duplicate {
            file: display(root, file),
            line: dup_line,
            replaced: true,
            reason: None,
            passes: copy.differs.iter().map(|(_, v)| v.clone()).collect(),
        });
    }

    let insert_pos = if is_method { scope_end } else { scope_start };
    let sep = if lang == Language::Python && !is_method {
        "\n\n"
    } else {
        "\n"
    };
    let insert_text = if is_method {
        format!("\n{fn_code}")
    } else {
        format!("{fn_code}{sep}")
    };
    file_edits.push((insert_pos, insert_pos, insert_text));

    let new_text = apply_edits(&text, &file_edits);
    let mut rewritten = vec![(file.to_string_lossy().to_string(), new_text)];

    if other_files {
        let sources = collect_workspace_sources(root, lang);
        for other in sources {
            if other == *file {
                continue;
            }
            let Ok(other_text) = std::fs::read_to_string(&other) else {
                continue;
            };
            let copies = find_duplicates_in_text(&selection, &other_text, None, parameterize, lang);
            if !copies.is_empty() {
                let uncovered_literal = copies.iter().find_map(|copy| {
                    copy.differs.iter().find_map(|(token_idx, _)| {
                        (!param_indices
                            .iter()
                            .any(|(known_idx, _, _)| known_idx == token_idx))
                        .then_some(*token_idx)
                    })
                });
                anyhow::ensure!(
                    uncovered_literal.is_none(),
                    "{} has a differing literal at token {}, but no parameter was created for it; nothing was rewritten",
                    display(root, &other),
                    uncovered_literal.unwrap_or_default()
                );
                let mut other_edits: Vec<(usize, usize, String)> = Vec::new();
                for copy in &copies {
                    let (dup_line, _) = crate::signature::position_at(&other_text, copy.start)?;
                    let copy_args: Vec<String> = inputs
                        .iter()
                        .map(|p| {
                            if let Some((tok_idx, _, _)) =
                                param_indices.iter().find(|(_, name, _)| name == &p.name)
                            {
                                if let Some((_, copy_lit)) =
                                    copy.differs.iter().find(|(idx, _)| idx == tok_idx)
                                {
                                    copy_lit.clone()
                                } else {
                                    p.name.clone()
                                }
                            } else {
                                p.name.clone()
                            }
                        })
                        .collect();
                    let dup_call = generate_call_replacement(
                        name,
                        &copy_args,
                        &param_names,
                        &output,
                        lang,
                        false,
                        "",
                    );
                    other_edits.push((copy.start, copy.end, dup_call));
                    duplicate_records.push(Duplicate {
                        file: display(root, &other),
                        line: dup_line,
                        replaced: true,
                        reason: None,
                        passes: copy.differs.iter().map(|(_, v)| v.clone()).collect(),
                    });
                }
                let mut new_other = apply_edits(&other_text, &other_edits);
                let import_stmt = match lang {
                    Language::TypeScript | Language::JavaScript => {
                        let rel = crate::move_polyglot::relative_import_specifier(&other, file);
                        format!("import {{ {name} }} from \"{rel}\";\n")
                    }
                    Language::Python => {
                        let mod_name = file.file_stem().unwrap_or_default().to_string_lossy();
                        format!("from .{mod_name} import {name}\n")
                    }
                    _ => String::new(),
                };
                if !import_stmt.is_empty() {
                    new_other.insert_str(0, &import_stmt);
                }
                rewritten.push((other.to_string_lossy().to_string(), new_other));
            }
        }
    }

    let validate_files: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &validate_files, &[]).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| {
            r.items
                .iter()
                .filter(|d| d.severity == "error")
                .map(move |d| format!("{}:{}:{}: {}", r.file, d.line, d.col, d.message))
        })
        .collect();

    let extracted = Extracted {
        name: name.to_string(),
        root: root.to_path_buf(),
        file: display(root, file),
        call: call_replacement,
        parameters: param_descriptions,
        duplicates: duplicate_records,
        rewritten,
        diagnostics,
        applied: false,
    };

    Ok(extracted)
}

pub fn apply_edits(text: &str, edits: &[(usize, usize, String)]) -> String {
    let mut sorted = edits.to_vec();
    sorted.sort_by_key(|(from, _, _)| std::cmp::Reverse(*from));
    let mut out = text.to_string();
    for (from, to, replacement) in sorted {
        if from <= to && to <= out.len() {
            out.replace_range(from..to, &replacement);
        }
    }
    out
}
