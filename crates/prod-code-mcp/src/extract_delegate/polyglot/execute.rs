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

use super::cpp::restructure_cpp;
use super::go::restructure_go;
use super::py::restructure_py;
use super::rewrite::{owner_region, rewrite_external_file};
use super::swift::restructure_swift;
use super::ts::restructure_ts;
use crate::extract_delegate::Extracted;
use crate::extract_delegate::common::{display, is_ident};
use crate::extract_delegate::rust::extract_delegate_rust;
use crate::parameter_object::Language;

/// Extracts `fields` and `methods` of a struct or class across languages into `helper`, held
/// in the new field `field`.
#[allow(clippy::too_many_arguments)]
pub async fn extract_delegate_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
    apply: bool,
    force: bool,
    _verify: Option<&str>,
) -> Result<Extracted> {
    for n in [helper, field] {
        anyhow::ensure!(
            !n.is_empty() && n.chars().all(is_ident),
            "`{n}` is not an identifier"
        );
    }
    anyhow::ensure!(!fields.is_empty(), "name at least one field");

    let lang = Language::of(file)
        .with_context(|| format!("unsupported language for {}", file.display()))?;
    if lang == Language::Rust {
        return extract_delegate_rust(
            remote, root, file, symbol, line, col, fields, methods, helper, field, apply, force,
        )
        .await;
    }
    if lang == Language::Java {
        anyhow::bail!("extract_delegate does not support Java yet");
    }

    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;

    let (restructured, owner) = match lang {
        Language::TypeScript => {
            restructure_ts(&text, symbol, line, fields, methods, helper, field, false)?
        }
        Language::JavaScript => {
            restructure_ts(&text, symbol, line, fields, methods, helper, field, true)?
        }
        Language::Python => restructure_py(&text, symbol, line, fields, methods, helper, field)?,
        Language::Cpp | Language::C => {
            restructure_cpp(&text, symbol, line, fields, methods, helper, field)?
        }
        Language::Swift => restructure_swift(&text, symbol, line, fields, methods, helper, field)?,
        Language::Go => restructure_go(&text, symbol, line, fields, methods, helper, field)?,
        Language::Rust | Language::Java => unreachable!(),
    };

    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut accesses = 0;
    let mut unmatched = Vec::new();
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let canonical_owner_file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let mut references_by_file_line: BTreeMap<PathBuf, BTreeMap<u32, usize>> = BTreeMap::new();
    for moved_field in fields {
        let position = match crate::encapsulate_field::field_position_in_lsp(
            remote,
            root,
            file,
            &owner,
            moved_field,
        )
        .await
        {
            Ok(position) => position,
            Err(err) => {
                unmatched.push(format!(
                    "{}: references for moved field `{moved_field}` could not be resolved: {err:#}",
                    display(root, file)
                ));
                continue;
            }
        };
        let Some((line, col)) = position else {
            continue;
        };
        match crate::signature::references(remote, root, file, line, col).await {
            Ok(references) => {
                for (path, line, _) in references {
                    let path = std::fs::canonicalize(&path).unwrap_or(path);
                    if !path.starts_with(&canonical_root) || Language::of(&path) != Some(lang) {
                        unmatched.push(format!(
                            "{}: moved-field reference is outside the supported workspace language",
                            display(root, &path)
                        ));
                        continue;
                    }
                    if path != canonical_owner_file {
                        *references_by_file_line
                            .entry(path)
                            .or_default()
                            .entry(line)
                            .or_default() += 1;
                    }
                }
            }
            Err(err) => unmatched.push(format!(
                "{}: references for moved field `{moved_field}` could not be resolved: {err:#}",
                display(root, file)
            )),
        }
    }

    let owner_body = owner_region(&restructured, lang, &owner).unwrap_or(&restructured);
    let (owner_probe, _) = rewrite_external_file(owner_body, lang, &owner, field, fields, helper);
    if owner_probe != owner_body {
        unmatched.push(format!(
            "{}: remaining owner methods contain moved-field accesses that need semantic resolution",
            display(root, file)
        ));
    }
    if matches!(lang, Language::Cpp | Language::C)
        && fields.iter().any(|moved| owner_body.contains(moved))
    {
        unmatched.push(format!(
            "{}: unqualified moved-field uses need semantic resolution",
            display(root, file)
        ));
    }
    files.insert(file.to_path_buf(), restructured);

    let mut processed = std::collections::BTreeSet::new();
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if path.is_file()
            && path != file
            && Language::of(path) == Some(lang)
            && let Ok(content) = std::fs::read_to_string(path)
            && fields.iter().any(|f| content.contains(f))
        {
            let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            processed.insert(canonical.clone());
            if lang == Language::Go {
                let (rewritten, _) =
                    rewrite_external_file(&content, lang, &owner, field, fields, helper);
                if rewritten != content {
                    unmatched.push(format!(
                            "{}: Go literals need package-aware semantic resolution before they can be rewritten",
                            display(root, path)
                        ));
                }
                continue;
            }
            let refs = references_by_file_line.get(&canonical);
            let mut rewritten = String::with_capacity(content.len());
            let mut rewrite_failed = false;
            let mut file_accesses = 0usize;
            for (index, raw_line) in content.split_inclusive('\n').enumerate() {
                let line_number = index as u32 + 1;
                let expected = refs
                    .and_then(|by_line| by_line.get(&line_number))
                    .copied()
                    .unwrap_or(0);
                let (body, ending) = if let Some(body) = raw_line.strip_suffix("\r\n") {
                    (body, "\r\n")
                } else if let Some(body) = raw_line.strip_suffix('\n') {
                    (body, "\n")
                } else {
                    (raw_line, "")
                };
                let (line, actual) =
                    rewrite_external_file(body, lang, &owner, field, fields, helper);
                if actual != expected {
                    if actual > 0 || expected > 0 {
                        unmatched.push(format!(
                                "{}:{}: {actual} textual moved-field access(es) do not match {expected} analyzer reference(s)",
                                display(root, path),
                                line_number
                            ));
                        rewrite_failed = true;
                        rewritten.push_str(raw_line);
                        continue;
                    }
                }
                file_accesses += actual;
                rewritten.push_str(&line);
                rewritten.push_str(ending);
            }
            if refs.is_some_and(|by_line| {
                by_line
                    .keys()
                    .any(|line| *line as usize > content.lines().count())
            }) {
                unmatched.push(format!(
                    "{}: analyzer reference points beyond end of file",
                    display(root, path)
                ));
                rewrite_failed = true;
            }
            if rewrite_failed {
                continue;
            }
            if rewritten != content {
                accesses += file_accesses;
                files.insert(path.to_path_buf(), rewritten);
            } else if file_accesses > 0 {
                accesses += file_accesses;
            }
        }
    }
    for path in references_by_file_line.keys() {
        if !processed.contains(path) {
            unmatched.push(format!(
                "{}: analyzer reference did not resolve to a readable workspace source file",
                display(root, path)
            ));
        }
    }

    files.retain(|p, t| std::fs::read_to_string(p).map(|o| o != *t).unwrap_or(true));

    let edits: Vec<(PathBuf, String)> = files.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &[]).await?;
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
            "{} reference(s) cannot be safely matched to `{owner}`; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }

    Ok(Extracted {
        helper: helper.to_string(),
        field: field.to_string(),
        fields: fields.to_vec(),
        methods: methods.to_vec(),
        root: root.to_path_buf(),
        rewritten: files
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        accesses,
        unmatched,
        diagnostics,
        applied,
    })
}
