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
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;

use super::lsp::{line_col_utf16, lsp_span};
use super::sources::{
    collect_sources, display, inspect_source, project_snapshot, refuse_linked_path,
    regular_unlinked_inside, unchanged,
};
use super::types::{CompileVerdict, FunctionRange, Project};

pub async fn reference_evidence(
    remote: SocketAddr,
    checkout: &Path,
    project: &Project,
    file: &Path,
    text: &str,
    function: &FunctionRange,
) -> Result<()> {
    let (line, character) = line_col_utf16(text, function.name_start)?;
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow!("invalid TypeScript source path {}", file.display()))?
        .to_string();
    let answer = crate::tools::execute_lsp_query(
        remote,
        checkout,
        file,
        "textDocument/references",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": true }
        }),
    )
    .await
    .context("the TypeScript language server could not list every reference")?;
    let locations = answer
        .as_array()
        .filter(|locations| !locations.is_empty())
        .with_context(|| {
            format!(
                "the TypeScript language server listed no location, not even the declaration of {}: {answer}",
                function.name
            )
        })?;
    let mut declarations = 0usize;
    let mut uses = Vec::new();
    let mut seen = BTreeSet::new();
    for location in locations {
        let uri = location
            .get("uri")
            .and_then(serde_json::Value::as_str)
            .context("a reference has no file URI")?;
        let uri = url::Url::parse(uri).context("a reference has an invalid URI")?;
        anyhow::ensure!(
            uri.scheme() == "file" && uri.query().is_none() && uri.fragment().is_none(),
            "a reference is not a plain local file URI"
        );
        let reported = uri
            .to_file_path()
            .map_err(|_| anyhow!("a reference is not a local file URI"))?;
        let path = regular_unlinked_inside(&project.root, &reported)?;
        anyhow::ensure!(
            project.sources.contains(&path),
            "{} is outside the complete configured TypeScript source set",
            display(checkout, &path)
        );
        let source = if path == file {
            text.to_string()
        } else {
            std::fs::read_to_string(&path).with_context(|| {
                format!("cannot read referenced source {}", display(checkout, &path))
            })?
        };
        let span = lsp_span(
            &source,
            location.get("range").context("a reference has no range")?,
        )
        .context("a reference has a malformed range")?;
        anyhow::ensure!(
            source.get(span.start..span.end) == Some(function.name.as_str())
                && span.end - span.start == function.name.len(),
            "{} names stale or malformed reference evidence",
            display(checkout, &path)
        );
        anyhow::ensure!(
            seen.insert((path.clone(), span.start, span.end)),
            "{} contains duplicate reference evidence",
            display(checkout, &path)
        );
        if path == file && span.start == function.name_start && span.end == function.name_end {
            declarations += 1;
        } else {
            let (line, col) = line_col_utf16(&source, span.start)?;
            uses.push(format!(
                "{}:{}:{}",
                display(checkout, &path),
                line + 1,
                col + 1
            ));
        }
    }
    anyhow::ensure!(
        declarations == 1,
        "the TypeScript language server listed the declaration {} times instead of exactly once",
        declarations
    );
    anyhow::ensure!(
        uses.is_empty(),
        "{} is still referenced at {}; no function was deleted",
        function.name,
        uses.join(", ")
    );
    Ok(())
}

pub async fn compile_typescript_shadow(
    remote: SocketAddr,
    checkout: &Path,
    project: &Path,
    source: &Path,
    proposed: &str,
) -> Result<CompileVerdict> {
    let observed = project_snapshot(project)?;
    let relative_project = project
        .strip_prefix(checkout)
        .expect("project is inside checkout");
    let subdir = if relative_project.as_os_str().is_empty() {
        None
    } else {
        Some(
            relative_project
                .to_str()
                .context("the TypeScript project path is not UTF-8")?,
        )
    };
    let outcome = crate::shadow::run_shadow(
        remote,
        checkout,
        subdir,
        &[crate::shadow::HypothesisSpec {
            name: "typescript-compiler-verification".to_string(),
            edits: vec![crate::shadow::HypothesisEdit {
                relative_path: crate::shadow::relative_edit_path(checkout, source)?,
                text: Some(proposed.to_string()),
            }],
        }],
        [
            "tsc",
            "--noEmit",
            "--pretty",
            "false",
            "--incremental",
            "false",
            "--project",
            "tsconfig.json",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        Vec::new(),
        120,
        1,
        16 * 1024,
        false,
    )
    .await
    .context("the remote gateway could not run TypeScript compiler verification")?;
    anyhow::ensure!(
        matches!(
            outcome.mode.as_str(),
            "overlay" | "overlay-ram" | "in-place"
        ),
        "the remote gateway returned an unrecognized shadow mode {:?}",
        outcome.mode
    );
    anyhow::ensure!(
        outcome.results.len() == 1,
        "the remote gateway returned {} compiler outcomes instead of one",
        outcome.results.len()
    );
    let result = &outcome.results[0];
    anyhow::ensure!(
        result.name == "typescript-compiler-verification",
        "the remote gateway returned compiler evidence for {:?}",
        result.name
    );
    anyhow::ensure!(
        result.error.is_none(),
        "the remote TypeScript compiler could not start: {}",
        result.error.as_deref().unwrap_or_default()
    );
    anyhow::ensure!(
        !result.timed_out,
        "the remote TypeScript compiler timed out"
    );
    let exit_code = result
        .exit_code
        .context("the remote TypeScript compiler returned no exit status")?;
    unchanged(project, &observed, "the remote compiler")?;
    let mut output = result.output.clone();
    if !outcome.server_workspace_root.is_empty() {
        output = output.replace(&outcome.server_workspace_root, ".");
    }
    output = output.replace(checkout.to_string_lossy().as_ref(), ".");
    if exit_code != 0 && output.trim().is_empty() {
        output = format!("the TypeScript compiler exited with {exit_code} and no diagnostic");
    }
    Ok(CompileVerdict {
        passed: exit_code == 0,
        output,
    })
}

pub fn inspect_project(checkout: &Path, source: &Path) -> Result<Project> {
    let mut directory = source.parent();
    let config = loop {
        let candidate = directory.context("the TypeScript source has no parent directory")?;
        anyhow::ensure!(
            candidate.starts_with(checkout),
            "the TypeScript source is outside the checkout"
        );
        let config = candidate.join("tsconfig.json");
        if config.exists() {
            break config;
        }
        if candidate == checkout {
            anyhow::bail!(
                "no tsconfig.json contains {}; nothing was written",
                display(checkout, source)
            );
        }
        directory = candidate.parent();
    };
    refuse_linked_path(checkout, checkout, &config)?;
    let config = regular_unlinked_inside(checkout, &config)?;
    let project_root = config.parent().expect("config has parent").to_path_buf();
    let raw = std::fs::read_to_string(&config).with_context(|| {
        format!(
            "cannot read {}; nothing was written",
            display(checkout, &config)
        )
    })?;
    let json: serde_json::Value = serde_json::from_str(&raw)
        .context("tsconfig.json must be strict JSON in the supported safe-delete subset")?;
    let object = json
        .as_object()
        .context("tsconfig.json must contain an object")?;
    for unsafe_key in ["extends", "references", "files", "exclude"] {
        anyhow::ensure!(
            !object.contains_key(unsafe_key),
            "tsconfig.json field {unsafe_key:?} is outside the supported contained configuration"
        );
    }
    anyhow::ensure!(
        object
            .keys()
            .all(|key| matches!(key.as_str(), "compilerOptions" | "include")),
        "tsconfig.json contains fields outside the supported simple configuration"
    );
    let options = object
        .get("compilerOptions")
        .and_then(serde_json::Value::as_object)
        .context("tsconfig.json needs compilerOptions")?;
    for unsafe_key in [
        "allowJs",
        "checkJs",
        "composite",
        "incremental",
        "declaration",
        "emitDeclarationOnly",
        "outDir",
        "rootDir",
        "rootDirs",
        "paths",
        "baseUrl",
        "typeRoots",
        "plugins",
    ] {
        anyhow::ensure!(
            !options.contains_key(unsafe_key),
            "compiler option {unsafe_key:?} is outside the supported contained configuration"
        );
    }
    let module = options
        .get("module")
        .and_then(serde_json::Value::as_str)
        .context("compilerOptions.module must explicitly select an ES module mode")?;
    anyhow::ensure!(
        matches!(
            module.to_ascii_lowercase().as_str(),
            "es2020" | "es2022" | "esnext" | "node16" | "nodenext"
        ),
        "compilerOptions.module is not a supported ES module mode"
    );
    let includes = object
        .get("include")
        .and_then(serde_json::Value::as_array)
        .filter(|items| !items.is_empty())
        .context("tsconfig.json needs a non-empty include list")?;
    let mut roots = BTreeSet::new();
    for include in includes {
        let include = include
            .as_str()
            .context("every tsconfig include entry must be a string")?;
        let prefix = include
            .strip_suffix("/**/*.ts")
            .or_else(|| include.strip_suffix("/**/*"))
            .unwrap_or(include)
            .trim_end_matches('/');
        anyhow::ensure!(
            !prefix.is_empty()
                && !prefix.contains('*')
                && !prefix.contains('?')
                && !Path::new(prefix).is_absolute()
                && Path::new(prefix).components().all(|component| {
                    matches!(
                        component,
                        std::path::Component::Normal(_) | std::path::Component::CurDir
                    )
                }),
            "tsconfig include {include:?} is outside the supported directory subset"
        );
        let requested = project_root.join(prefix);
        refuse_linked_path(&project_root, &project_root, &requested)?;
        let canonical = std::fs::canonicalize(&requested)
            .with_context(|| format!("cannot resolve included TypeScript directory {include:?}"))?;
        anyhow::ensure!(
            canonical.starts_with(&project_root) && canonical.is_dir(),
            "included TypeScript path {include:?} is not a directory inside the project"
        );
        roots.insert(canonical);
    }

    let mut sources = BTreeSet::new();
    for root in roots {
        collect_sources(&project_root, &root, &mut sources)?;
    }
    anyhow::ensure!(
        sources.contains(source),
        "{} is not in the tsconfig include set",
        display(checkout, source)
    );
    for path in &sources {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!("cannot read configured source {}", display(checkout, path))
        })?;
        inspect_source(&text).with_context(|| {
            format!(
                "configured source {} is unsupported",
                display(checkout, path)
            )
        })?;
    }
    Ok(Project {
        root: project_root,
        sources,
    })
}

pub fn refuse_incomplete_evidence(root: &Path, stage: &str) -> Result<()> {
    let notes = crate::session::take_indexing_notes(root);
    anyhow::ensure!(
        notes.is_empty(),
        "the TypeScript language server reported incomplete {stage} evidence: {}",
        notes.join("; ")
    );
    Ok(())
}
