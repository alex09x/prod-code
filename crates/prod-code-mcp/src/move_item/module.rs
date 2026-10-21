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

use super::item::display;
use super::types::ModulePath;

pub(super) fn target_text(root: &Path, target: &Path) -> Result<(Option<PathBuf>, String)> {
    if target.exists() {
        let text = std::fs::read_to_string(target)
            .with_context(|| format!("cannot read {}", target.display()))?;
        return Ok((None, text));
    }
    anyhow::ensure!(
        target.extension().is_some_and(|e| e == "rs"),
        "{} is not a Rust source file",
        display(root, target)
    );
    let parent = parent_module_file(target).with_context(|| {
        format!(
            "{} does not exist, and neither does a module file to declare it in (for \
         `src/a/x.rs`, `src/a.rs` or `src/a/mod.rs`); create the parent module first",
            display(root, target)
        )
    })?;
    Ok((Some(parent), String::new()))
}

/// The file that must declare the module `target` would be: `src/lib.rs` or `src/main.rs` for
/// `src/util.rs`, and `src/a.rs` or `src/a/mod.rs` for `src/a/util.rs`. The first that exists.
pub fn parent_module_file(target: &Path) -> Option<PathBuf> {
    let dir = target.parent()?;
    let candidates = if dir.file_name().is_some_and(|n| n == "src") {
        vec![dir.join("lib.rs"), dir.join("main.rs")]
    } else {
        vec![dir.with_extension("rs"), dir.join("mod.rs")]
    };
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .or_else(|| super::test_crate::parent_module_file(target))
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
        None => text[..super::preamble::preamble_end(text)].lines().count(),
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
/// In the ordinary layout `src/lib.rs` and `src/main.rs` are the crate root,
/// `src/a.rs` and `src/a/mod.rs` are both the module `a`. A file reached through `#[path]` is
/// not generally understood. Integration-test targets and their declared path submodules
/// are resolved separately as independent crate roots.
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
    if let Some(module) = super::test_crate::module_of(&file, &crate_dir)? {
        return Ok((crate_dir, module));
    }
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

/// When the selected declaration at `start..=decl_end` is an inline `mod <name> { ... }` and the
/// target file is empty, extracts the module's inner body into `target` and replaces the inline
/// module in `source_text` with an external `mod <name>;` declaration (annotated with
/// `#[path = "..."]` when `target` is not at the default child module path) so `use super::*;`
/// inside the moved module keeps its original parent scope (#1117).
pub(crate) fn extract_inline_module(
    source_text: &str,
    start: u32,
    decl_end: u32,
    name: &str,
    file: &Path,
    target: &Path,
) -> Option<(String, String)> {
    let (_, raw_item) = super::item::cut(source_text, start, decl_end);
    let mod_kw = format!("mod {name}");
    let mod_pos = raw_item.find(&mod_kw)?;
    if mod_pos > 0
        && raw_item[..mod_pos]
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_whitespace())
    {
        return None;
    }
    let after_mod = &raw_item[mod_pos + mod_kw.len()..];
    let brace_rel = after_mod.find('{')?;
    if !after_mod[..brace_rel].trim().is_empty() {
        return None;
    }
    let open_brace = mod_pos + mod_kw.len() + brace_rel;
    let close_brace = raw_item.rfind('}')?;
    if close_brace <= open_brace {
        return None;
    }

    let header = raw_item[..open_brace].trim_end();
    let mut header_lines: Vec<String> = header.lines().map(str::to_string).collect();
    if header_lines.is_empty() {
        return None;
    }
    let last_line = header_lines.pop()?;
    let indent: String = last_line
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();

    let file_dir = file.parent()?;
    let file_stem = file.file_stem()?.to_str()?;
    let default_child = if matches!(file_stem, "lib" | "main" | "mod") {
        file_dir.join(format!("{name}.rs"))
    } else {
        file_dir.join(file_stem).join(format!("{name}.rs"))
    };
    let has_path_attr = header_lines.iter().any(|l| l.trim().starts_with("#[path"));
    if target != default_child && !has_path_attr {
        let rel = target
            .strip_prefix(file_dir)
            .ok()
            .and_then(|p| p.to_str())
            .map(|s| s.replace('\\', "/"))
            .or_else(|| {
                target
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
            })?;
        header_lines.push(format!("{indent}#[path = \"{rel}\"]"));
    }
    header_lines.push(format!("{};", last_line.trim_end()));
    let replacement_decl = header_lines.join("\n");

    let lines: Vec<&str> = source_text.lines().collect();
    let s = (start as usize).saturating_sub(1);
    let e = (decl_end as usize).min(lines.len());
    let mut kept: Vec<String> = lines[..s].iter().map(|l| (*l).to_string()).collect();
    kept.push(replacement_decl);
    kept.extend(lines[e..].iter().map(|l| (*l).to_string()));
    let mut source_new = kept.join("\n");
    if source_text.ends_with('\n') {
        source_new.push('\n');
    }

    let inner = &raw_item[open_brace + 1..close_brace];
    let mut body_lines: Vec<&str> = inner.lines().collect();
    while body_lines.first().is_some_and(|l| l.trim().is_empty()) {
        body_lines.remove(0);
    }
    while body_lines.last().is_some_and(|l| l.trim().is_empty()) {
        body_lines.pop();
    }
    let unindented: Vec<String> = body_lines
        .into_iter()
        .map(|l| {
            if let Some(stripped) = l.strip_prefix("    ") {
                stripped.to_string()
            } else if let Some(stripped) = l.strip_prefix('\t') {
                stripped.to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    let target_new = format!("{}\n", unindented.join("\n").trim_end());
    Some((source_new, target_new))
}
