/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;

use crate::diagnostics::filter::refresh_hallucinations;
use crate::diagnostics::types::{DiagnosticsReport, DocDiagnostic};

/// Returns a reason string if the error indicates the file was excluded by platform build tags or constraints.
pub fn platform_exclusion_reason(file: &Path, err: &anyhow::Error) -> Option<String> {
    let err_str = err.to_string();
    let lower_err = err_str.to_lowercase();
    let file_str = file.to_string_lossy();

    let is_metadata_err = lower_err.contains("no package metadata")
        || lower_err.contains("no packages found")
        || lower_err.contains("build constraints exclude all go files")
        || lower_err.contains("cannot find package")
        || lower_err.contains("no such package");

    if is_metadata_err {
        if file_str.ends_with("_darwin.go") {
            return Some("Darwin / macOS build tags excluded on current host".to_string());
        }
        if file_str.ends_with("_linux.go") {
            return Some("Linux build tags excluded on current host".to_string());
        }
        if file_str.ends_with("_windows.go") {
            return Some("Windows build tags excluded on current host".to_string());
        }
        if file_str.ends_with("_freebsd.go") || file_str.ends_with("_openbsd.go") {
            return Some("BSD build tags excluded on current host".to_string());
        }
        return Some(format!(
            "excluded by platform build constraints: {}",
            err_str.lines().next().unwrap_or(&err_str)
        ));
    }
    None
}

pub fn platform_excluded_report(file: &str, reason: &str) -> DiagnosticsReport {
    DiagnosticsReport {
        file: file.to_string(),
        errors: 0,
        warnings: 0,
        items: vec![DocDiagnostic {
            severity: "info".to_string(),
            code: Some("platform-excluded".to_string()),
            message: reason.to_string(),
            line: 1,
            col: 1,
            source: Some("prod-code".to_string()),
            note: None,
            end: None,
        }],
        preexisting: Vec::new(),
        in_derive: Vec::new(),
        auto_trait: Vec::new(),
        hallucinations: Vec::new(),
    }
}

/// Whether an error message matches a Swift compiler symbol or type resolution error across targets.
pub fn is_swift_cross_target_candidate(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("has no member")
        || lower.contains("cannot find type")
        || lower.contains("cannot find '")
        || lower.contains("no member named")
        || lower.contains("is not a member type of")
        || lower.contains("extra argument")
        || lower.contains("incorrect argument label")
        || lower.contains("missing argument for parameter")
        || lower.contains("do not match any available overload")
}

/// The target name of a Swift file based on SwiftPM standard layout (Sources/<Target> or Tests/<Target>).
pub fn swift_target_name(path: &Path) -> Option<String> {
    let parts: Vec<&str> = path
        .components()
        .map(|c| c.as_os_str().to_str().unwrap_or(""))
        .collect();
    for (i, &p) in parts.iter().enumerate() {
        if (p == "Sources" || p == "Tests") && i + 1 < parts.len() {
            return Some(parts[i + 1].to_string());
        }
    }
    None
}

/// Swift sourcekit-lsp cannot compile dependent targets in-memory across module boundaries;
/// when target B imports target A with @testable or import, sourcekit-lsp evaluates target B
/// against the on-disk .swiftmodule in .build, which may be stale or lack proposed in-memory
/// members. When errors in a Swift validation batch match cross-target resolution failures or
/// span multiple targets, verify them with a shadow compiler check (`swift build --build-tests`).
/// If the compiler accepts the overlay with zero errors, suppress the false positive diagnostics (#759, #762).
pub async fn reconcile_swift_cross_target_diagnostics(
    remote: SocketAddr,
    root: &Path,
    edits: &[(std::path::PathBuf, String)],
    reports: &mut [DiagnosticsReport],
) -> Result<()> {
    let has_swift = edits
        .iter()
        .any(|(p, _)| p.extension().and_then(|e| e.to_str()) == Some("swift"));
    if !has_swift {
        return Ok(());
    }

    let has_errors = reports.iter().any(|r| r.errors > 0);
    if !has_errors {
        return Ok(());
    }

    let targets: HashSet<String> = edits
        .iter()
        .filter_map(|(p, _)| swift_target_name(p))
        .collect();
    let is_cross_target = targets.len() > 1;
    let has_candidate_error = reports.iter().any(|r| {
        r.items
            .iter()
            .any(|i| i.severity == "error" && is_swift_cross_target_candidate(&i.message))
    });

    if !is_cross_target && !has_candidate_error {
        return Ok(());
    }

    if let Ok((0, _output)) = crate::tools::compile_check(remote, root, edits).await {
        tracing::info!(
            "Swift cross-target overlay verified cleanly by shadow compiler; suppressing stale sourcekit-lsp diagnostics"
        );
        for report in reports.iter_mut() {
            report.items.retain(|item| {
                item.severity != "error" || !is_swift_cross_target_candidate(&item.message)
            });
            report.errors = report
                .items
                .iter()
                .filter(|item| item.severity == "error")
                .count();
            report.warnings = report
                .items
                .iter()
                .filter(|item| item.severity == "warning")
                .count();
            refresh_hallucinations(report);
        }
    }

    Ok(())
}
