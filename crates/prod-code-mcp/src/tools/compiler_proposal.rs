/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::validation_proposal::ValidationProposal;
use anyhow::{Context, Result};
use std::{net::SocketAddr, path::Path};

/// These names influence the existing engine/tool/command selection. Changing them
/// requires route selection against the proposed definitions, which is not established
/// by inspecting the current checkout. No private build-language parser is required.
fn build_definition(path: &str) -> bool {
    let path = Path::new(path);
    matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some(
            "Cargo.toml"
                | "go.mod"
                | "go.work"
                | "package.json"
                | "tsconfig.json"
                | "pyproject.toml"
                | "setup.cfg"
                | "Package.swift"
                | "CMakeLists.txt"
                | "CMakePresets.json"
                | "meson.build"
                | "meson.options"
                | "meson_options.txt"
                | "Makefile"
                | "makefile"
                | "GNUmakefile"
                | "build.zig"
                | "build.gradle"
                | "build.gradle.kts"
                | "pom.xml"
                | "prod-code.toml"
        )
    ) || path.extension().is_some_and(|s| s == "cmake" || s == "mk")
}

pub(super) async fn compile_proposal(
    remote: SocketAddr,
    root: &Path,
    proposal: &ValidationProposal,
) -> Result<(usize, String)> {
    let definitions: Vec<_> = proposal
        .spec
        .edits
        .iter()
        .filter(|e| build_definition(&e.relative_path))
        .map(|e| e.relative_path.as_str())
        .collect();
    if !definitions.is_empty() {
        return Ok((
            1,
            format!(
                "compiler: incomplete/unverified: changed build definitions {}; configured command route selection is unestablished; no compiler request was made",
                definitions.join(", ")
            ),
        ));
    }
    let mut routes = Vec::new();
    let mut unresolved = Vec::new();
    // The public text-only entry point also supports checking an unchanged baseline.
    if proposal.spec.edits.is_empty() {
        let (subdir, language) = crate::sync::engine_project(root, root);
        routes.push((
            subdir,
            language.context("compile needs a project manifest")?,
        ));
    }
    for edit in &proposal.spec.edits {
        let hint = root.join(&edit.relative_path);
        let hint = if hint.is_symlink() {
            hint.parent().unwrap_or(root).to_path_buf()
        } else {
            hint
        };
        let (subdir, engine) = crate::sync::engine_project(root, &hint);
        let language = engine.or_else(|| crate::sync::expected_engine(root));
        if let Some(language) = language {
            let route = (subdir, language);
            if !routes.contains(&route) {
                routes.push(route);
            }
        } else {
            unresolved.push(format!(
                "{}: no configured compiler route",
                edit.relative_path
            ));
        }
    }
    // engine_project intentionally preserves legacy selection, including same-language
    // enclosing projects. Expose other recognizable command roots rather than assuming
    // the first route compiles them. This is bounded discovery, not graph certification.
    for edit in &proposal.spec.edits {
        let hint = root.join(&edit.relative_path);
        for dir in hint
            .parent()
            .into_iter()
            .flat_map(Path::ancestors)
            .take_while(|p| p.starts_with(root))
        {
            let relative = dir.strip_prefix(root).unwrap();
            let subdir = (!relative.as_os_str().is_empty())
                .then(|| relative.to_string_lossy().replace('\\', "/"));
            // Cargo's selected workspace command already includes its owned members.
            // Reuse route ownership rather than treating all same-language crates as
            // covered. Other manifests in a member directory remain independent.
            let covered_cargo_member = dir.join("Cargo.toml").is_file()
                && routes.iter().any(|(cwd, language)| {
                    let project = root.join(cwd.as_deref().unwrap_or(""));
                    *language == "rust"
                        && project.join("Cargo.toml").is_file()
                        && dir != project
                        && dir.starts_with(&project)
                        && !crate::sync::engine::workspace::excluded_from_root_workspace(
                            &project, dir,
                        )
                });
            let manifest = [
                "Cargo.toml",
                "go.mod",
                "package.json",
                "Package.swift",
                "CMakeLists.txt",
                "meson.build",
                "Makefile",
                "makefile",
                "GNUmakefile",
                "pyproject.toml",
                "build.zig",
                "pom.xml",
                "build.gradle",
                "build.gradle.kts",
            ]
            .iter()
            .any(|name| {
                dir.join(name).is_file() && (*name != "Cargo.toml" || !covered_cargo_member)
            });
            if manifest && !routes.iter().any(|(cwd, _)| cwd == &subdir) {
                let message = format!(
                    "additional command root {} is unverified/incomplete",
                    subdir.as_deref().unwrap_or(".")
                );
                if !unresolved.contains(&message) {
                    unresolved.push(message);
                }
            }
        }
    }
    let mut count = usize::from(!unresolved.is_empty());
    let mut reports = unresolved;
    for (subdir, language) in routes {
        let project = root.join(subdir.as_deref().unwrap_or(""));
        let command = match crate::verify::plan_command_with(
            &crate::verify::detect_tools(&project),
            language,
            crate::verify::VerifyKind::Check,
            None,
        ) {
            Ok(command) => command,
            Err(error) => {
                count += 1;
                reports.push(format!(
                    "compiler: cwd={:?} route unverified/incomplete: {error:#}",
                    subdir.as_deref().unwrap_or(".")
                ));
                continue;
            }
        };
        let run = format!(
            "argv={} cwd={} timeout_secs=900",
            serde_json::to_string(&command)?,
            serde_json::to_string(subdir.as_deref().unwrap_or("."))?
        );
        let outcome = crate::shadow::run_shadow(
            remote,
            root,
            subdir.as_deref(),
            std::slice::from_ref(&proposal.spec),
            command,
            Vec::new(),
            900,
            1,
            1 << 20,
            false,
        )
        .await
        .with_context(|| {
            format!("compiler: {run} exit=unknown error=shadow request failed timed_out=unknown")
        })?;
        let result = outcome
            .results
            .iter()
            .find(|r| r.name == proposal.spec.name)
            .with_context(|| {
                format!("compiler: {run} exit=unknown error=missing-result timed_out=unknown")
            })?;
        let status = format!(
            "compiler: {run} exit={} error={} timed_out={}",
            result.exit_code.map_or("unknown".into(), |c| c.to_string()),
            serde_json::to_string(&result.error)?,
            result.timed_out
        );
        let mut seen = std::collections::HashSet::new();
        let errors: Vec<_> =
            super::diagnostics_handler::compile_diagnostics(language, &result.output)
                .into_iter()
                .filter(|d| d.level == "error" && seen.insert(d.render()))
                .collect();
        let failed = result.error.is_some()
            || result.timed_out
            || result.exit_code != Some(0)
            || !errors.is_empty();
        count += if failed { errors.len().max(1) } else { 0 };
        let mut report = format!("{status}: {} error(s)", errors.len());
        if failed {
            for error in &errors {
                report.push_str(&format!("\n  {}", error.render()));
            }
            if errors.iter().any(|d| {
                d.code
                    .as_deref()
                    .is_some_and(crate::diagnostics::is_borrow_checker_error_code)
            }) {
                report.push_str(
                    "\n  [INTERCEPT: BORROW CHECKER VIOLATION detected in proposed edits]",
                );
            }
            // Preserve failures even if the output parser recognizes no diagnostic.
            if errors.is_empty() {
                report.push_str(&format!("\n{}", result.output));
            }
        } else {
            report.push_str("\nconfigured compiler check passed on the complete supported proposal; coverage is limited to the displayed command");
        }
        report.push_str(
            "\nTarget, feature, platform and companion build coverage is not established.",
        );
        reports.push(report);
    }
    Ok((count, reports.join("\n")))
}
