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
use std::path::{Path, PathBuf};

/// The native TypeScript 7 compiler binary, which doubles as the language server
/// (`tsc --lsp --stdio`): `tsgo` on PATH, or the platform package under the global
/// `typescript` install (`@typescript/typescript-<os>-<arch>/lib/tsc`).
pub fn native_typescript_lsp() -> Option<PathBuf> {
    if let Ok(tsgo) = which_bin("tsgo") {
        return Some(tsgo);
    }
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let candidate = npm_global_root()?
        .join("typescript")
        .join("node_modules")
        .join("@typescript")
        .join(format!("typescript-{os}-{arch}"))
        .join("lib")
        .join("tsc");
    candidate.is_file().then_some(candidate)
}

/// Global npm module root (`npm root -g`), where `npm install -g` puts packages.
pub fn npm_global_root() -> Option<PathBuf> {
    let out = std::process::Command::new("npm")
        .args(["root", "-g"])
        .output()
        .ok()?;
    let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!root.is_empty()).then(|| PathBuf::from(root))
}

pub fn settings_for_section(root: &Path, section: &str) -> serde_json::Value {
    let analysis = serde_json::json!({
        "diagnosticMode": "workspace",
        "autoSearchPaths": true,
        "useLibraryCodeForTypes": true,
    });
    match section {
        "python.analysis" | "basedpyright.analysis" => analysis,
        s if s.starts_with("python") || s.starts_with("basedpyright") => {
            let mut settings = serde_json::json!({ "analysis": analysis });
            if let Some(python) = venv_python(root) {
                settings["pythonPath"] = serde_json::Value::String(python);
                settings["venvPath"] =
                    serde_json::Value::String(root.to_string_lossy().into_owned());
                settings["venv"] = serde_json::Value::String(".venv".to_string());
            }
            settings
        }
        _ => serde_json::json!({}),
    }
}

/// `<root>/.venv/bin/python` (or `venv/`, or in an ancestor checkout) when the project carries a virtual environment.
pub fn venv_python(root: &Path) -> Option<String> {
    for ancestor in root.ancestors() {
        for dir in [".venv", "venv"] {
            let candidate = ancestor.join(dir).join("bin").join("python");
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Helper to check if an executable binary is present in PATH.
pub fn which_bin(name: &str) -> Result<PathBuf> {
    let output = std::process::Command::new("which")
        .arg(name)
        .output()
        .context("which command failed")?;
    if output.status.success() {
        let path_str = String::from_utf8(output.stdout)?.trim().to_string();
        if !path_str.is_empty() {
            return Ok(PathBuf::from(path_str));
        }
    }
    anyhow::bail!("Binary {name} not found in PATH")
}
