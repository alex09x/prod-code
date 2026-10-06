/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, anyhow};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;

use super::compile::{
    compile_typescript_shadow, inspect_project, reference_evidence, refuse_incomplete_evidence,
};
use super::lsp::offset_at;
use super::parser::function_at;
use super::sources::{
    display, inspect_source, module_marker, project_snapshot, refuse_linked_path,
    regular_unlinked_inside, unchanged,
};
use super::types::DeletedFunction;

pub async fn delete_function(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<DeletedFunction> {
    anyhow::ensure!(
        file.extension().is_some_and(|extension| extension == "ts")
            && !file
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".d.ts")),
        "{} is not a supported TypeScript source file; nothing was written",
        file.display()
    );
    let checkout = std::fs::canonicalize(root)
        .with_context(|| format!("cannot resolve checkout {}", root.display()))?;
    refuse_linked_path(root, &checkout, file)?;
    let file = regular_unlinked_inside(&checkout, file)?;
    let project = inspect_project(&checkout, &file)?;
    let original = std::fs::read_to_string(&file).with_context(|| {
        format!(
            "cannot read {}; nothing was written",
            display(&checkout, &file)
        )
    })?;
    inspect_source(&original)?;
    anyhow::ensure!(
        module_marker(&original)?,
        "{} is not a contained TypeScript ES module; nothing was written",
        display(&checkout, &file)
    );
    anyhow::ensure!(
        line > 0 && col > 0,
        "the requested position is not one-based"
    );
    let requested = offset_at(&original, line - 1, col - 1).with_context(|| {
        format!(
            "{}:{line}:{col} is not a valid UTF-16 source position; nothing was written",
            display(&checkout, &file)
        )
    })?;

    let observed = project_snapshot(&project.root)?;
    let uri = url::Url::from_file_path(&file)
        .map_err(|_| anyhow!("invalid TypeScript source path {}", file.display()))?
        .to_string();
    let symbols = crate::tools::execute_lsp_query(
        remote,
        &checkout,
        &file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await
    .context(
        "the TypeScript language server could not describe declarations; nothing was written",
    )?;
    refuse_incomplete_evidence(&checkout, "declaration")?;
    unchanged(&project.root, &observed, "declarations")?;
    let function = function_at(&original, requested, &symbols)
        .context("TypeScript safe delete refused; nothing was written")?;

    reference_evidence(remote, &checkout, &project, &file, &original, &function)
        .await
        .context("TypeScript safe delete refused; nothing was written")?;
    refuse_incomplete_evidence(&checkout, "reference")?;
    unchanged(&project.root, &observed, "references")?;

    let mut proposed = original.clone();
    proposed.replace_range(function.start..function.end, "");
    let verdict = compile_typescript_shadow(remote, &checkout, &project.root, &file, &proposed)
        .await
        .context("TypeScript safe delete compiler evidence is unavailable; nothing was written")?;
    anyhow::ensure!(
        verdict.passed,
        "deleting {} does not compile under the active TypeScript configuration; nothing was written:\n{}",
        function.name,
        verdict.output.trim()
    );
    unchanged(&project.root, &observed, "the compiler check")?;

    let mut files = BTreeMap::new();
    files.insert(file.clone(), proposed);
    let touched = crate::refactor::apply_workspace_edit(
        &checkout,
        &crate::signature::whole_file_edit(&files),
    )
    .context("the verified TypeScript deletion could not be applied")?;
    anyhow::ensure!(
        touched.len() == 1,
        "the verified TypeScript deletion updated {} paths instead of one",
        touched.len()
    );
    Ok(DeletedFunction {
        name: function.name,
        path: file,
    })
}
