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

use super::types::ModulePath;

/// The file that must declare the module `target` would be: `src/lib.rs` or `src/main.rs` for
/// `src/util.rs`, and `src/a.rs` or `src/a/mod.rs` for `src/a/util.rs`. The first that exists.
pub fn parent_module_file(target: &Path) -> Option<PathBuf> {
    let dir = target.parent()?;
    let candidates = if dir.file_name().is_some_and(|n| n == "src") {
        vec![dir.join("lib.rs"), dir.join("main.rs")]
    } else {
        vec![dir.with_extension("rs"), dir.join("mod.rs")]
    };
    candidates.into_iter().find(|p| p.is_file())
}

/// `text` with `mod name;` (or `pub mod name;`) declared after its last top-level `mod` line, or
/// after its leading inner doc comments and attributes when it has none.
pub fn declare_module(text: &str, name: &str, public: bool) -> String {
    let line = format!("{}mod {name};", if public { "pub " } else { "" });
    let lines: Vec<&str> = text.lines().collect();
    let is_mod = |l: &str| {
        let t = l.trim_start();
        (t.starts_with("mod ") || t.starts_with("pub mod ") || t.starts_with("pub(crate) mod "))
            && t.trim_end().ends_with(';')
            && !l.starts_with(char::is_whitespace)
    };
    let at = match lines.iter().rposition(|l| is_mod(l)) {
        Some(i) => i + 1,
        None => lines
            .iter()
            .position(|l| {
                let t = l.trim_start();
                !(t.starts_with("//!") || t.starts_with("#![") || t.is_empty())
            })
            .unwrap_or(lines.len()),
    };
    // Blank lines above the declaration and nothing else (what an item cut from the top of the
    // file leaves behind) go.
    let leading_blank = if lines[..at].iter().all(|l| l.trim().is_empty()) {
        at
    } else {
        0
    };
    let at = at - leading_blank;
    let mut out: Vec<String> = lines[leading_blank..]
        .iter()
        .map(|l| l.to_string())
        .collect();
    out.insert(at, line);
    // One blank line between the declarations and what follows them.
    if lines.iter().rposition(|l| is_mod(l)).is_none()
        && out.get(at + 1).is_some_and(|l| !l.trim().is_empty())
    {
        out.insert(at + 1, String::new());
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

/// The crate directory above `file`, and the module `file` is inside it.
///
/// Only the ordinary layout is understood: `src/lib.rs` and `src/main.rs` are the crate root,
/// `src/a.rs` and `src/a/mod.rs` are both the module `a`. A file reached through `#[path]` is
/// not, and the caller is told so rather than moved into the wrong module.
pub fn module_of(file: &Path) -> Result<(PathBuf, ModulePath)> {
    let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let crate_dir = file
        .ancestors()
        .skip(1)
        .find(|dir| dir.join("Cargo.toml").is_file())
        .with_context(|| format!("no Cargo.toml above {}", file.display()))?
        .to_path_buf();
    let manifest = std::fs::read_to_string(crate_dir.join("Cargo.toml"))
        .with_context(|| format!("cannot read {}", crate_dir.join("Cargo.toml").display()))?;
    let krate = crate_name(&manifest).with_context(|| {
        format!(
            "{} names no package",
            crate_dir.join("Cargo.toml").display()
        )
    })?;

    let rel = file.strip_prefix(crate_dir.join("src")).map_err(|_| {
        anyhow::anyhow!(
            "{} is not under {}/src; only the ordinary crate layout is understood",
            file.display(),
            crate_dir.display()
        )
    })?;
    let mut segments: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let last = segments.pop().unwrap_or_default();
    match last.strip_suffix(".rs") {
        Some("lib") | Some("main") | Some("mod") => {}
        Some(stem) => segments.push(stem.to_string()),
        None => anyhow::bail!("{} is not a Rust source file", file.display()),
    }
    Ok((crate_dir, ModulePath { krate, segments }))
}

/// The crate's Rust-spelled name: `[lib] name` when it has one, else `[package] name`.
pub(crate) fn crate_name(manifest: &str) -> Option<String> {
    let mut package = None;
    let mut lib = None;
    let mut section = "";
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.trim_matches(['[', ']'].as_slice());
            continue;
        }
        let Some(rest) = line.strip_prefix("name") else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match section {
            "package" => package = Some(value),
            "lib" => lib = Some(value),
            _ => {}
        }
    }
    lib.or(package).map(|n| n.replace('-', "_"))
}
