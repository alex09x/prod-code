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
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use super::cpp::make_static_cpp;
use super::go::make_static_go;
use super::py::make_static_py;
use super::swift::make_static_swift;
use super::ts::make_static_ts;
use crate::make_static::calls::{method_declaration_position, rewrite_calls_in_code};
use crate::make_static::helpers::display;
use crate::make_static::types::{Language, MadeStatic};

#[allow(clippy::too_many_arguments)]
pub async fn make_static_polyglot(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    class_name: Option<&str>,
    method_name: &str,
    apply: bool,
    force: bool,
) -> Result<MadeStatic> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Cannot read {}", file_path.display()))?;
    let lang = Language::from_path(file_path)
        .with_context(|| format!("Unsupported language for file: {}", file_path.display()))?;

    let (owner, method, _, _, mut blocked) = match lang {
        Language::TypeScript => make_static_ts(&content, class_name, method_name, None)?,
        Language::Python => make_static_py(&content, class_name, method_name, None)?,
        Language::Cpp => make_static_cpp(&content, class_name, method_name, None)?,
        Language::Swift => make_static_swift(&content, class_name, method_name, None)?,
        Language::Go => make_static_go(&content, class_name, method_name, None)?,
    };

    let (declaration_line, declaration_col) =
        method_declaration_position(&content, lang, &owner, &method)?;
    let references = crate::signature::references(
        remote,
        workspace_root,
        file_path,
        declaration_line,
        declaration_col,
    )
    .await
    .with_context(|| format!("cannot find calls to `{owner}.{method}`; nothing was planned"))?;
    let mut references_by_file: BTreeMap<PathBuf, std::collections::HashSet<(u32, u32)>> =
        BTreeMap::new();
    for (path, line, col) in references {
        let canonical = std::fs::canonicalize(&path).unwrap_or(path);
        references_by_file
            .entry(canonical)
            .or_default()
            .insert((line, col));
    }
    let canonical_target =
        std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    let mut target_references = references_by_file
        .remove(&canonical_target)
        .unwrap_or_default();
    let (_, _, new_content, file_rewritten, parser_blocked) = match lang {
        Language::TypeScript => make_static_ts(
            &content,
            Some(&owner),
            &method,
            Some(&mut target_references),
        )?,
        Language::Python => make_static_py(
            &content,
            Some(&owner),
            &method,
            Some(&mut target_references),
        )?,
        Language::Cpp => make_static_cpp(
            &content,
            Some(&owner),
            &method,
            Some(&mut target_references),
        )?,
        Language::Swift => make_static_swift(
            &content,
            Some(&owner),
            &method,
            Some(&mut target_references),
        )?,
        Language::Go => make_static_go(
            &content,
            Some(&owner),
            &method,
            Some(&mut target_references),
        )?,
    };
    blocked.extend(parser_blocked);

    let receiver = match lang {
        Language::TypeScript => "this".to_string(),
        Language::Python => "self".to_string(),
        Language::Cpp => "*this".to_string(),
        Language::Swift => "self".to_string(),
        Language::Go => format!("(*{owner})"),
    };

    let mut rewritten = vec![(file_path.to_string_lossy().to_string(), new_content)];
    let mut total_rewritten_calls = file_rewritten;
    let mut unmatched = Vec::new();
    for (line, col) in target_references {
        unmatched.push(format!(
            "{}:{line}:{col}: analyzer reference was not a supported call",
            display(workspace_root, file_path)
        ));
    }

    for entry in ignore::WalkBuilder::new(workspace_root).build().flatten() {
        let path = entry.path();
        if path.is_file()
            && path != file_path
            && lang.matches_extension(path)
            && let Ok(other_content) = std::fs::read_to_string(path)
            && other_content.contains(method_name)
        {
            let rel = display(workspace_root, path);
            let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            let mut file_references = references_by_file.remove(&canonical).unwrap_or_default();
            let (new_other, calls) = rewrite_calls_in_code(
                &other_content,
                method_name,
                &owner,
                lang,
                &rel,
                &mut blocked,
                Some(&mut file_references),
            );
            for (line, col) in file_references {
                unmatched.push(format!(
                    "{rel}:{line}:{col}: analyzer reference was not a supported call"
                ));
            }
            if new_other != other_content {
                rewritten.push((path.to_string_lossy().to_string(), new_other));
                total_rewritten_calls += calls;
            }
        }
    }

    for (path, references) in references_by_file {
        for (line, col) in references {
            unmatched.push(format!(
                "{}:{line}:{col}: analyzer reference was not a supported call",
                display(workspace_root, &path)
            ));
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

    if apply {
        anyhow::ensure!(
            blocked.is_empty(),
            "{} call site(s) would drop a receiver that does something; nothing was written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
        );
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) were not rewritten; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let rewritten_map: BTreeMap<PathBuf, String> = rewritten
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.clone()))
            .collect();
        let edit = crate::signature::whole_file_edit(&rewritten_map);
        crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
    }

    let rel_file = display(workspace_root, file_path);
    Ok(MadeStatic {
        owner,
        method: method.to_string(),
        root: workspace_root.to_path_buf(),
        file: rel_file,
        receiver,
        rewritten_calls: total_rewritten_calls,
        blocked,
        unmatched,
        rewritten,
        diagnostics,
        applied: apply,
    })
}
