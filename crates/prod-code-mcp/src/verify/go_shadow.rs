/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

/// The Go compiler's verdict for complete proposed source in an isolated checkout copy.
#[derive(Debug, Clone)]
pub struct GoCompileVerdict {
    pub passed: bool,
    pub output: String,
}

/// Compiles every package and its external test package with the proposed files in place.
///
/// `go test -exec=true -run=^$` builds every package and test binary without writing colliding
/// artifacts to disk or executing test bodies. [`crate::shadow::run_shadow`] stages the complete proposed texts in a
/// private workspace on the supplied gateway; no Go process or checkout copy is created on the
/// client. `-mod=readonly` prevents the compiler from changing module metadata. A source file
/// changing while the gateway checks the proposal is an error, never a verdict about a mixture
/// of revisions.
pub async fn compile_go_shadow(
    remote: SocketAddr,
    root: &Path,
    source: &Path,
    files: &[(PathBuf, String)],
) -> Result<GoCompileVerdict> {
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|e| anyhow!("cannot resolve the checkout {}: {e}", root.display()))?;
    let source = canonical_inside(&canonical_root, source)?;
    let module = go_module_root(&canonical_root, &source)?;
    let module_relative = module
        .strip_prefix(&canonical_root)
        .expect("module is inside root");
    let subdir = if module_relative.as_os_str().is_empty() {
        None
    } else {
        Some(
            module_relative
                .to_str()
                .context("the Go module path is not UTF-8")?,
        )
    };

    let observed = crate::sync::scan_workspace_files(&canonical_root, Some(&module))?;
    let edits = files
        .iter()
        .map(|(path, text)| {
            let path = canonical_inside(&canonical_root, path)?;
            anyhow::ensure!(
                path.starts_with(&module),
                "{} is outside the selected Go module {}; verification refused",
                path.display(),
                module.display()
            );
            Ok(crate::shadow::HypothesisEdit {
                relative_path: crate::shadow::relative_edit_path(&canonical_root, &path)?,
                text: Some(text.clone()),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let outcome = crate::shadow::run_shadow(
        remote,
        &canonical_root,
        subdir,
        &[crate::shadow::HypothesisSpec {
            name: "go-compiler-verification".to_string(),
            edits,
        }],
        vec![
            "go".to_string(),
            "test".to_string(),
            "-exec=true".to_string(),
            "-run=^$".to_string(),
            "-mod=readonly".to_string(),
            "./...".to_string(),
        ],
        vec![("GOTOOLCHAIN".to_string(), "local".to_string())],
        120,
        1,
        16 * 1024,
        false,
    )
    .await
    .context("the remote gateway could not run Go compiler verification")?;
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
        result.name == "go-compiler-verification",
        "the remote gateway returned compiler evidence for {:?}",
        result.name
    );
    anyhow::ensure!(
        result.error.is_none(),
        "the remote Go compiler could not start: {}",
        result.error.as_deref().unwrap_or_default()
    );
    anyhow::ensure!(!result.timed_out, "the remote Go compiler timed out");
    let exit_code = result
        .exit_code
        .context("the remote Go compiler returned no exit status")?;

    let current = crate::sync::scan_workspace_files(&canonical_root, Some(&module))?;
    anyhow::ensure!(
        observed == current,
        "the Go module changed while the remote compiler checked the proposal; nothing was written"
    );
    let output = if exit_code != 0 && result.output.trim().is_empty() {
        format!("the Go compiler exited with {exit_code} and no diagnostic")
    } else {
        result.output.clone()
    };
    Ok(GoCompileVerdict {
        passed: exit_code == 0,
        output,
    })
}

fn canonical_inside(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let path = std::fs::canonicalize(&path)
        .with_context(|| format!("cannot resolve {} for Go verification", path.display()))?;
    anyhow::ensure!(
        path.starts_with(root) && path.is_file(),
        "{} is not a regular file inside the checkout; Go verification refused",
        path.display()
    );
    Ok(path)
}

fn go_module_root(root: &Path, source: &Path) -> Result<PathBuf> {
    let mut directory = source.parent();
    while let Some(candidate) = directory {
        anyhow::ensure!(
            candidate.starts_with(root),
            "{} is outside the checkout; Go verification refused",
            source.display()
        );
        if candidate.join("go.mod").is_file() {
            return Ok(candidate.to_path_buf());
        }
        if candidate == root {
            break;
        }
        directory = candidate.parent();
    }
    anyhow::bail!(
        "cannot find a Go module containing {}; compiler verification refused",
        source.display()
    )
}
