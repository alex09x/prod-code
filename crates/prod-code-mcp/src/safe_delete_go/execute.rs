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
use std::path::Path;

use anyhow::{Context, Result, anyhow};

use super::directives::{
    current_text, go_module_root, is_generated, refuse_linked_source_path, refuse_linked_sources,
    refuse_source_directives, regular_unlinked_inside,
};
use super::packages::package_sources;
use super::parse::function_at;
use super::receiver::receiver_source_evidence;
use super::references::reference_evidence;
use super::types::{DeletedFunction, display, offset_at};

pub async fn delete_function(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<DeletedFunction> {
    anyhow::ensure!(
        file.extension().is_some_and(|extension| extension == "go"),
        "{} is not a Go source file; nothing was written",
        file.display()
    );
    let canonical_root = std::fs::canonicalize(root)
        .with_context(|| format!("cannot resolve checkout {}", root.display()))?;
    refuse_linked_source_path(root, &canonical_root, file)?;
    let canonical_file = regular_unlinked_inside(&canonical_root, file)?;
    let module = go_module_root(&canonical_root, &canonical_file)?;
    refuse_linked_sources(&module)?;

    let original = std::fs::read_to_string(&canonical_file).with_context(|| {
        format!(
            "cannot read {}; nothing was written",
            canonical_file.display()
        )
    })?;
    anyhow::ensure!(
        !is_generated(&original),
        "{} is generated Go source; nothing was written",
        display(&canonical_root, &canonical_file)
    );
    refuse_source_directives(&original)?;
    anyhow::ensure!(
        line > 0 && col > 0,
        "{}:{line}:{col} is not on a declaration name; nothing was written",
        display(&canonical_root, &canonical_file)
    );
    let requested = offset_at(&original, line - 1, col - 1).with_context(|| {
        format!(
            "{}:{line}:{col} is not a valid UTF-16 source position; nothing was written",
            display(&canonical_root, &canonical_file)
        )
    })?;

    let uri = url::Url::from_file_path(&canonical_file)
        .map_err(|_| anyhow!("invalid Go source path {}", canonical_file.display()))?
        .to_string();
    let symbols = crate::tools::execute_lsp_query(
        remote,
        &canonical_root,
        &canonical_file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await
    .context("gopls could not describe the Go declarations; nothing was written")?;
    let function = function_at(&canonical_file, &original, requested, &symbols)
        .context("Go safe delete refused; nothing was written")?;

    let receiver_sources = if let Some(receiver) = &function.receiver {
        let sources = receiver_source_evidence(&canonical_file, &original, receiver)
            .context("Go receiver-method safe delete refused; nothing was written")?;
        crate::signature_go::receiver_interface_evidence(
            remote,
            &canonical_root,
            &canonical_file,
            &original,
            &function.name,
            function.name_start,
        )
        .await
        .context(
            "Go receiver-method safe delete requires empty gopls implementation evidence; nothing was written",
        )?;
        if let Some(path) = crate::signature_go::package_interface_method_file(
            &canonical_file,
            &function.name,
        )
        .context(
            "Go receiver-method safe delete could not inspect local interface obligations; nothing was written",
        )? {
            anyhow::bail!(
                "Go receiver-method safe delete refused; {} has a local interface obligation in {}; nothing was written",
                function.name,
                display(&canonical_root, &path)
            );
        }
        Some(sources)
    } else {
        None
    };

    anyhow::ensure!(
        current_text(&canonical_file, &original)?,
        "{} changed while its declaration was inspected; nothing was written",
        display(&canonical_root, &canonical_file)
    );
    if let Some(expected) = &receiver_sources {
        anyhow::ensure!(
            package_sources(&canonical_file, &original)? == *expected,
            "the declaring Go package changed while receiver-method evidence was compiled; nothing was written"
        );
    }
    reference_evidence(
        remote,
        &canonical_root,
        &canonical_file,
        &original,
        &function,
    )
    .await
    .context("Go safe delete refused; nothing was written")?;

    let mut proposed = original.clone();
    proposed.replace_range(function.start..function.end, "");
    let verdict = crate::verify::compile_go_shadow(
        remote,
        &canonical_root,
        &canonical_file,
        &[(canonical_file.clone(), proposed.clone())],
    )
    .await
    .context("Go safe delete compiler evidence is unavailable; nothing was written")?;
    anyhow::ensure!(
        verdict.passed,
        "deleting {} does not compile under the active Go build flags; nothing was written:\n{}",
        function.name,
        verdict.output.trim()
    );
    anyhow::ensure!(
        current_text(&canonical_file, &original)?,
        "{} changed while the deletion was compiled; nothing was written",
        display(&canonical_root, &canonical_file)
    );
    if let Some(expected) = &receiver_sources {
        anyhow::ensure!(
            package_sources(&canonical_file, &original)? == *expected,
            "the declaring Go package changed while the receiver-method deletion was compiled; nothing was written"
        );
    }

    let mut files = BTreeMap::new();
    files.insert(canonical_file.clone(), proposed);
    let touched = crate::refactor::apply_workspace_edit(
        &canonical_root,
        &crate::signature::whole_file_edit(&files),
    )
    .context("the verified Go deletion could not be applied")?;
    anyhow::ensure!(
        touched.len() == 1,
        "the verified Go deletion updated {} paths instead of one",
        touched.len()
    );
    Ok(DeletedFunction {
        name: function.name,
        path: canonical_file,
    })
}
