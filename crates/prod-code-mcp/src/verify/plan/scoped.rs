/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    has_xcode_project, is_xcodegen_spec, narrow_scope, plan_command_with, plan_xcode_command,
    plan_xcodegen_command, verify_project,
};
use crate::verify::{ProjectTools, VerifyKind, detect_tools};
use anyhow::{Context, Result, anyhow, ensure};
use std::path::Path;

pub(crate) struct ScopedVerification {
    pub subdir: Option<String>,
    pub language: &'static str,
    pub tools: ProjectTools,
    pub command: Vec<String>,
}

tokio::task_local! { static REQUIRE_NARROW: (); }

#[derive(Debug)]
pub(crate) struct WorkflowPlanRefusal(anyhow::Error);
impl std::fmt::Display for WorkflowPlanRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.0)
    }
}
impl std::error::Error for WorkflowPlanRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pub(crate) async fn with_narrow_scope<F: std::future::Future>(future: F) -> F::Output {
    REQUIRE_NARROW.scope((), future).await
}

/// Filesystem-only planning shared by execution and workflow preflight. This does
/// not sync, spawn a checker or run project configuration commands.
pub(crate) fn plan_scoped(
    root: &Path,
    project_hint: Option<&Path>,
    kind: VerifyKind,
    filter: Option<&str>,
) -> Result<ScopedVerification> {
    if REQUIRE_NARROW.try_with(|_| ()).is_ok() {
        return plan_narrow(root, project_hint, kind, filter)
            .map_err(|error| WorkflowPlanRefusal(error).into());
    }
    plan_inner(root, project_hint, kind, filter)
}

pub(crate) fn plan_narrow(
    root: &Path,
    hint: Option<&Path>,
    kind: VerifyKind,
    filter: Option<&str>,
) -> Result<ScopedVerification> {
    let target = hint.context("workflow verification requires a scope")?;
    let target = if target.is_absolute() || target.starts_with(root) {
        target.to_path_buf()
    } else {
        root.join(target)
    };
    let root = std::fs::canonicalize(root)?;
    let target = std::fs::canonicalize(target)?;
    ensure!(
        target != root && target.starts_with(&root),
        "verify scope must remain below the workspace root"
    );
    let metadata = std::fs::metadata(&target)?;
    ensure!(
        metadata.is_file() || metadata.is_dir(),
        "verify scope must be a regular file or directory"
    );
    let planned = plan_inner(&root, Some(&target), kind, filter)?;
    let scoped_go_cwd = planned
        .subdir
        .as_ref()
        .and_then(|subdir| std::fs::canonicalize(root.join(subdir)).ok())
        .is_some_and(|project| {
            project != root && project.starts_with(&root) && target.starts_with(project)
        });
    ensure!(
        !((planned.language == "rust" && planned.command.iter().any(|arg| arg == "--workspace"))
            || (planned.language == "go"
                && !scoped_go_cwd
                && planned.command.iter().any(|arg| arg == "./..."))),
        "verify scope would select the whole {} workspace at {}; choose a package path the verifier can narrow",
        planned.language,
        planned.subdir.as_deref().unwrap_or(".")
    );
    if kind == VerifyKind::Test {
        let unfiltered = plan_inner(&root, Some(&target), kind, None)?;
        ensure!(
            planned.command != unfiltered.command,
            "selected test runner cannot apply the requested filter; no test was started"
        );
    }
    Ok(planned)
}

fn plan_inner(
    root: &Path,
    project_hint: Option<&Path>,
    kind: VerifyKind,
    filter: Option<&str>,
) -> Result<ScopedVerification> {
    let normalized_hint = project_hint.map(|hint| {
        if hint.is_absolute() || hint.starts_with(root) {
            hint.to_path_buf()
        } else {
            root.join(hint)
        }
    });
    let project_hint = normalized_hint.as_deref();
    let (subdir, language) = verify_project(root, project_hint);
    let language = language.ok_or_else(|| anyhow!(
        "no project manifest (Cargo.toml, go.mod, package.json, pyproject.toml, CMakeLists.txt, Package.swift) at {}",
        root.display()
    ))?;
    let project_dir = subdir
        .as_ref()
        .map_or_else(|| root.to_path_buf(), |sub| root.join(sub));
    let tools = detect_tools(&project_dir);
    let mut command = if language == "swift" && has_xcode_project(&project_dir) {
        plan_xcode_command(kind, filter)?
    } else if language == "swift" && is_xcodegen_spec(&project_dir) {
        plan_xcodegen_command(kind, filter)?
    } else {
        plan_command_with(&tools, language, kind, filter)?
    };
    if let Some(hint) = project_hint {
        let missing_test_file = !hint.exists()
            && hint.extension().is_some_and(|extension| extension == "rs")
            && super::member_dir_named(&project_dir, hint).is_none();
        if language == "rust" && kind == VerifyKind::Test && (hint.is_file() || missing_test_file) {
            super::rust_test_file::narrow_rust_test_file(&mut command, &project_dir, hint)?;
        } else {
            narrow_scope(&mut command, language, &project_dir, hint);
            if language == "rust" && command.iter().any(|arg| arg == "--workspace") {
                let relative = hint.strip_prefix(&project_dir).ok();
                let root_source = relative.is_some_and(|path| {
                    path == Path::new("Cargo.toml")
                        || path.components().next().is_some_and(|part| {
                            matches!(
                                part.as_os_str().to_str(),
                                Some("src" | "tests" | "examples" | "benches")
                            )
                        })
                });
                if root_source
                    && let Some(name) = super::cargo_package_name(&project_dir.join("Cargo.toml"))
                {
                    let index = command
                        .iter()
                        .position(|arg| arg == "--workspace")
                        .expect("checked marker");
                    command.splice(index..=index, ["-p".into(), name]);
                }
            }
        }
    }
    Ok(ScopedVerification {
        subdir,
        language,
        tools,
        command,
    })
}
