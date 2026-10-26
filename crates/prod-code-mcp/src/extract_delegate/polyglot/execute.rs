/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    cpp::restructure_cpp_semantic, go::restructure_go_semantic, py::restructure_py_semantic,
    semantic::Source, swift::restructure_swift_semantic, ts::restructure_ts_semantic,
};
use crate::extract_delegate::{
    Extracted,
    common::{display, is_ident},
    rust::extract_delegate_rust,
};
use crate::parameter_object::Language;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_arguments)]
fn restructure(
    source: &mut Source<'_>,
    symbol: Option<&str>,
    line: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
) -> Result<(String, String)> {
    let text = source.text;
    let field = source.field;
    match source.lang {
        Language::TypeScript | Language::JavaScript => restructure_ts_semantic(
            text,
            symbol,
            line,
            fields,
            methods,
            helper,
            field,
            source.lang == Language::JavaScript,
            source,
        ),
        Language::Python => {
            restructure_py_semantic(text, symbol, line, fields, methods, helper, field, source)
        }
        Language::Go => {
            restructure_go_semantic(text, symbol, line, fields, methods, helper, field, source)
        }
        Language::Cpp | Language::C => {
            restructure_cpp_semantic(text, symbol, line, fields, methods, helper, field, source)
        }
        Language::Swift => {
            restructure_swift_semantic(text, symbol, line, fields, methods, helper, field, source)
        }
        _ => unreachable!(),
    }
}

/// Extract fields and methods using identities from the analyzer's original positions.
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
    anyhow::ensure!(
        lang != Language::Java,
        "extract_delegate does not support Java yet"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let (_, owner) = restructure(
        &mut Source::new(&text, lang, field),
        symbol,
        line,
        fields,
        methods,
        helper,
    )?;
    let canonical_root = std::fs::canonicalize(root)?;
    let canonical_file = std::fs::canonicalize(file)?;
    let mut references: BTreeMap<PathBuf, Vec<(u32, u32, String)>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    for moved in fields {
        let position =
            crate::encapsulate_field::field_position_in_lsp(remote, root, file, &owner, moved)
                .await;
        let (l, c) = match position {
            Ok(Some(p)) => p,
            Ok(None) => {
                unmatched.push(format!(
                    "{}: field `{moved}` has no analyzer identity",
                    display(root, file)
                ));
                continue;
            }
            Err(e) => {
                unmatched.push(format!("field `{moved}` identity: {e:#}"));
                continue;
            }
        };
        match crate::signature::references(remote, root, file, l, c).await {
            Ok(refs) => {
                for (path, l, c) in refs {
                    let path = std::fs::canonicalize(&path).unwrap_or(path);
                    if !path.starts_with(&canonical_root) || Language::of(&path) != Some(lang) {
                        unmatched.push(format!(
                            "{}: moved-field reference is outside the supported workspace language",
                            display(root, &path)
                        ));
                    } else {
                        references
                            .entry(path)
                            .or_default()
                            .push((l, c, moved.clone()));
                    }
                }
            }
            Err(e) => unmatched.push(format!("field `{moved}` references: {e:#}")),
        }
    }
    references.entry(canonical_file.clone()).or_default();
    let mut files = BTreeMap::new();
    let mut accesses = 0;
    for (path, refs) in references {
        let content = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                unmatched.push(format!("{}: cannot read source: {e}", display(root, &path)));
                continue;
            }
        };
        let mut source = Source::new(&content, lang, field);
        let planned = (|| -> Result<String> {
            for (l, c, name) in refs {
                source.add(l, c, &name)?;
            }
            let result = if path == canonical_file {
                restructure(&mut source, Some(&owner), line, fields, methods, helper)?.0
            } else if lang == Language::Go {
                if path.parent() != canonical_file.parent() {
                    for name in [helper, field] {
                        anyhow::ensure!(
                            name.chars().next().is_some_and(char::is_uppercase),
                            "Go cross-package extraction requires exported requested name `{name}`"
                        );
                    }
                }
                let mut edits = super::go_literals::literal_edits(&mut source, &owner, helper)?;
                edits.sort_by_key(|e| e.0);
                let mut out = String::new();
                let mut cursor = 0;
                for (s, e, replacement) in edits {
                    anyhow::ensure!(
                        s >= cursor,
                        "overlapping Go literals have unproved evaluation semantics"
                    );
                    out.push_str(&source.rewrite(&content[cursor..s])?);
                    out.push_str(&replacement);
                    cursor = e;
                }
                out.push_str(&source.rewrite(&content[cursor..])?);
                out
            } else {
                source.rewrite(&content)?
            };
            source.finish()?;
            Ok(result)
        })();
        match planned {
            Ok(rewritten) => {
                accesses += source.accesses;
                if rewritten != content {
                    files.insert(path, rewritten);
                }
            }
            Err(e) => unmatched.push(format!("{}: {e:#}", display(root, &path))),
        }
    }
    let edits: Vec<_> = files.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
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
        helper: helper.into(),
        field: field.into(),
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
