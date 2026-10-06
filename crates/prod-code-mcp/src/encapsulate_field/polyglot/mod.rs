/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod cpp;
pub mod go;
pub mod lsp;
pub mod py;
pub mod replace;
pub mod swift;
pub mod ts_js;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use cpp::{encapsulate_field_cpp, rewrite_external_cpp};
pub use go::{encapsulate_field_go, rewrite_external_go};
pub(crate) use lsp::field_position_in_lsp;
pub use lsp::{field_position_in_symbols, lsp_symbol_position};
pub use py::{encapsulate_field_py, rewrite_external_py};
pub use replace::{
    replace_cpp_unqualified, replace_line_self, replace_line_this, replace_line_this_private,
};
pub use swift::{encapsulate_field_swift, rewrite_external_swift};
pub use ts_js::{encapsulate_field_js, encapsulate_field_ts, rewrite_external_ts};

use super::case::{display, field_at_line_col, is_ident};
use super::types::{EncapsulatedField, Language};

fn has_ambiguous_property_use(text: &str, field: &str) -> bool {
    let needles = [format!(".{field}"), format!("->{field}")];
    text.lines().any(|line| {
        needles.iter().any(|needle| {
            let mut rest = line;
            while let Some(pos) = rest.find(needle) {
                let before = rest[..pos].trim_end();
                let receiver = before
                    .rsplit(|c: char| !is_ident(c))
                    .next()
                    .unwrap_or_default();
                if receiver != "this" && receiver != "self" {
                    return true;
                }
                rest = &rest[pos + needle.len()..];
            }
            false
        })
    })
}

fn rewrite_external_line(lang: Language, line: &str, field: &str) -> (String, usize, usize) {
    match lang {
        Language::TypeScript | Language::JavaScript => rewrite_external_ts(line, field),
        Language::Python => rewrite_external_py(line, field),
        Language::Cpp => rewrite_external_cpp(line, field),
        Language::Swift => rewrite_external_swift(line, field),
        Language::Go => rewrite_external_go(line, field),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn encapsulate_polyglot(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    class_name: Option<&str>,
    field_name: &str,
    by_value: Option<bool>,
    apply: bool,
    force: bool,
) -> Result<EncapsulatedField> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Cannot read {}", file_path.display()))?;
    let lang = Language::from_path(file_path)
        .with_context(|| format!("Unsupported language for file: {}", file_path.display()))?;

    let (owner, ty, new_content, file_reads, file_writes, left_in_file) = match lang {
        Language::TypeScript => encapsulate_field_ts(&content, class_name, field_name)?,
        Language::JavaScript => encapsulate_field_js(&content, class_name, field_name)?,
        Language::Python => encapsulate_field_py(&content, class_name, field_name)?,
        Language::Cpp => encapsulate_field_cpp(&content, class_name, field_name, by_value)?,
        Language::Swift => encapsulate_field_swift(&content, class_name, field_name)?,
        Language::Go => encapsulate_field_go(&content, class_name, field_name)?,
    };

    let mut rewritten = vec![(file_path.to_string_lossy().to_string(), new_content)];
    let mut total_reads = file_reads;
    let mut total_writes = file_writes;
    let mut unmatched = Vec::new();
    let dot_access = format!(".{field_name}");
    let quoted_or_commented_access = content.lines().any(|line| {
        line.contains(&dot_access)
            && (line.contains('\"')
                || line.contains('\'')
                || line.contains("//")
                || line.contains("/*"))
    });
    let canonical_root =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let canonical_file =
        std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    let mut references_by_file: BTreeMap<PathBuf, BTreeMap<u32, Vec<u32>>> = BTreeMap::new();
    let references_available =
        match field_position_in_lsp(remote, workspace_root, file_path, &owner, field_name).await {
            Ok(Some((line, col))) => {
                match crate::signature::references(remote, workspace_root, file_path, line, col)
                    .await
                {
                    Ok(references) => {
                        for (path, line, col) in references {
                            let path = std::fs::canonicalize(&path).unwrap_or(path);
                            references_by_file
                                .entry(path)
                                .or_default()
                                .entry(line)
                                .or_default()
                                .push(col);
                        }
                        true
                    }
                    Err(err) => {
                        unmatched.push(format!(
                            "{}: analyzer references could not be verified: {err:#}",
                            display(workspace_root, file_path)
                        ));
                        false
                    }
                }
            }
            Ok(None) => false,
            Err(err) => {
                unmatched.push(format!(
                    "{}: field symbols could not be resolved: {err:#}",
                    display(workspace_root, file_path)
                ));
                false
            }
        };

    if !references_available && quoted_or_commented_access {
        unmatched.push(format!(
            "{}: possible `{field_name}` references occur in string or comment text and need semantic resolution",
            display(workspace_root, file_path)
        ));
    }

    let target_reference_count: usize = references_by_file
        .get(&canonical_file)
        .into_iter()
        .flat_map(|lines| lines.values())
        .map(Vec::len)
        .sum();
    if references_available && target_reference_count != file_reads + file_writes + left_in_file {
        unmatched.push(format!(
            "{}: analyzer found {target_reference_count} field reference(s) in the declaring file, but only {} could be rewritten",
            display(workspace_root, file_path),
            file_reads + file_writes + left_in_file
        ));
    }

    if !references_available && has_ambiguous_property_use(&content, field_name) {
        unmatched.push(format!(
            "{}: property accesses cannot be proven to belong to `{owner}`",
            display(workspace_root, file_path)
        ));
    }

    for (path, ref_lines) in &references_by_file {
        if path == &canonical_file {
            continue;
        }
        if !path.starts_with(&canonical_root) || !lang.matches_extension(path) {
            unmatched.push(format!(
                "{}: analyzer reference is outside the supported workspace language",
                display(workspace_root, path)
            ));
            continue;
        }
        let other_content = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                unmatched.push(format!(
                    "{}: referenced source could not be read: {err}",
                    display(workspace_root, path)
                ));
                continue;
            }
        };
        let mut output = String::with_capacity(other_content.len());
        let mut file_reads = 0;
        let mut file_writes = 0;
        let mut rewrite_failed = false;
        for (index, source_line) in other_content.split_inclusive('\n').enumerate() {
            let line_number = index as u32 + 1;
            let Some(columns) = ref_lines.get(&line_number) else {
                if source_line.contains(&format!(".{field_name}"))
                    || (lang == Language::Cpp && source_line.contains(&format!("->{field_name}")))
                {
                    unmatched.push(format!(
                        "{}:{}: possible `{field_name}` access is absent from analyzer references",
                        display(workspace_root, path),
                        line_number
                    ));
                    rewrite_failed = true;
                }
                output.push_str(source_line);
                continue;
            };
            let (body, ending) = if let Some(body) = source_line.strip_suffix("\r\n") {
                (body, "\r\n")
            } else if let Some(body) = source_line.strip_suffix('\n') {
                (body, "\n")
            } else {
                (source_line, "")
            };
            if columns
                .iter()
                .any(|col| field_at_line_col(body, 1, *col).as_deref() != Some(field_name))
            {
                unmatched.push(format!(
                    "{}:{}: analyzer reference does not point at `{field_name}`",
                    display(workspace_root, path),
                    line_number
                ));
                rewrite_failed = true;
                output.push_str(source_line);
                continue;
            }
            let (changed, reads, writes) = rewrite_external_line(lang, body, field_name);
            if reads + writes != columns.len() {
                unmatched.push(format!(
                    "{}:{}: only {} of {} analyzer reference(s) could be rewritten",
                    display(workspace_root, path),
                    line_number,
                    reads + writes,
                    columns.len()
                ));
                rewrite_failed = true;
                output.push_str(source_line);
                continue;
            }
            file_reads += reads;
            file_writes += writes;
            output.push_str(&changed);
            output.push_str(ending);
        }
        if ref_lines
            .keys()
            .any(|line| *line as usize > other_content.lines().count())
        {
            unmatched.push(format!(
                "{}: analyzer reference points beyond end of file",
                display(workspace_root, path)
            ));
            rewrite_failed = true;
        }
        if !rewrite_failed {
            total_reads += file_reads;
            total_writes += file_writes;
            rewritten.push((path.to_string_lossy().into_owned(), output));
        }
    }

    for entry in ignore::WalkBuilder::new(workspace_root).build().flatten() {
        let path = entry.path();
        if !path.is_file() || !lang.matches_extension(path) {
            continue;
        }
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if canonical == canonical_file {
            continue;
        }
        let Ok(other_content) = std::fs::read_to_string(path) else {
            continue;
        };
        let ref_lines = references_by_file.get(&canonical);
        for (index, line) in other_content.lines().enumerate() {
            if line.contains(&format!(".{field_name}"))
                || (lang == Language::Cpp && line.contains(&format!("->{field_name}")))
            {
                if ref_lines.is_none_or(|lines| !lines.contains_key(&(index as u32 + 1))) {
                    unmatched.push(format!(
                        "{}:{}: possible `{field_name}` access could not be matched to `{owner}`",
                        display(workspace_root, path),
                        index + 1
                    ));
                }
            }
        }
    }

    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.clone()))
        .collect();
    let reports =
        crate::diagnostics::validate_texts(remote, workspace_root, &to_check, &[]).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.source
                    .as_deref()
                    .map(|s| format!("[{s}] "))
                    .unwrap_or_default(),
                d.message,
                d.line,
                d.col
            )
        })
        .collect();

    let mut applied = false;
    if apply && unmatched.is_empty() && (diagnostics.is_empty() || force) {
        let rewritten_map: BTreeMap<PathBuf, String> = rewritten
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.clone()))
            .collect();
        let edit = crate::signature::whole_file_edit(&rewritten_map);
        crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
        applied = true;
    }

    let rel_file = display(workspace_root, file_path);
    Ok(EncapsulatedField {
        owner,
        root: workspace_root.to_path_buf(),
        file: rel_file,
        field: field_name.to_string(),
        ty,
        by_value: by_value.unwrap_or(false),
        reads: total_reads,
        writes: total_writes,
        chained_reads: 0,
        left_in_file,
        blocked: vec![],
        unmatched,
        rewritten,
        diagnostics,
        applied,
    })
}
