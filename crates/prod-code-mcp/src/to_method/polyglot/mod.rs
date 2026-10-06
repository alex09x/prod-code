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
pub mod py;
pub mod rewrite;
pub mod swift;
pub mod ts;

pub use self::cpp::to_method_cpp;
pub use self::go::to_method_go;
pub use self::py::to_method_py;
pub use self::rewrite::rewrite_static_calls_in_code;
pub use self::swift::to_method_swift;
pub use self::ts::to_method_ts;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::make_static::Language;
use crate::to_method::helpers::display;
use crate::to_method::types::MadeMethod;

#[allow(clippy::too_many_arguments)]
pub async fn convert_to_method_polyglot(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    class_name: Option<&str>,
    method_name: &str,
    apply: bool,
    force: bool,
) -> Result<MadeMethod> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Cannot read {}", file_path.display()))?;
    let lang = Language::from_path(file_path)
        .with_context(|| format!("Unsupported language for file: {}", file_path.display()))?;

    let (owner, method, parameter, receiver, new_content, renamed_uses, file_rewritten) = match lang
    {
        Language::TypeScript => to_method_ts(&content, class_name, method_name)?,
        Language::Python => to_method_py(&content, class_name, method_name)?,
        Language::Cpp => to_method_cpp(&content, class_name, method_name)?,
        Language::Swift => to_method_swift(&content, class_name, method_name)?,
        Language::Go => to_method_go(&content, class_name, method_name)?,
    };

    let mut rewritten = vec![(file_path.to_string_lossy().to_string(), new_content)];
    let mut total_rewritten_calls = file_rewritten;

    for entry in ignore::WalkBuilder::new(workspace_root).build().flatten() {
        let path = entry.path();
        if path.is_file()
            && path != file_path
            && lang.matches_extension(path)
            && let Ok(other_content) = std::fs::read_to_string(path)
            && other_content.contains(method_name)
        {
            let (new_other, calls) =
                rewrite_static_calls_in_code(&other_content, method_name, &owner, lang);
            if new_other != other_content {
                rewritten.push((path.to_string_lossy().to_string(), new_other));
                total_rewritten_calls += calls;
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

    if apply {
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
    Ok(MadeMethod {
        owner,
        method,
        root: workspace_root.to_path_buf(),
        file: rel_file,
        parameter,
        receiver,
        renamed_uses,
        rewritten_calls: total_rewritten_calls,
        unchanged: vec![],
        unmatched: vec![],
        rewritten,
        diagnostics,
        applied: apply,
    })
}
