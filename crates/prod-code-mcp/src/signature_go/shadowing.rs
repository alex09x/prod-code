/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature_go::text::{closing, display, is_ident_byte, skip_opaque, skip_space};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::Path;

/// Primitive-looking names are safe only while the declaring package has not shadowed them.
/// Go's package block spans files and declarations are order-independent, so checking only the
/// signature token or accepting a successful compile would mistake a user-defined type or alias
/// for a predeclared primitive.
pub(crate) fn ensure_predeclared_types_unshadowed(
    root: &Path,
    file: &Path,
    types: &[&str],
) -> Result<()> {
    let directory = file
        .parent()
        .with_context(|| format!("{} has no containing package directory", file.display()))?;
    let declaring_text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let package = package_name(&declaring_text)
        .with_context(|| format!("cannot identify the Go package in {}", file.display()))?;
    let wanted: BTreeSet<&str> = types.iter().copied().collect();

    for entry in std::fs::read_dir(directory).with_context(|| {
        format!(
            "cannot inspect the Go package directory {}",
            directory.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "cannot inspect an entry in the Go package directory {}",
                directory.display()
            )
        })?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "go") {
            continue;
        }
        let file_type = entry
            .file_type()
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        // `DirEntry::file_type` deliberately does not follow links. Go and the checkout sync
        // do, so silently skipping one could make a package-level alias look predeclared. Do
        // not follow it here: the link might lead outside the checkout we are allowed to read.
        if file_type.is_symlink() {
            anyhow::bail!(
                "cannot prove primitive type identity: linked Go source {} is not inspected",
                display(root, &path)
            );
        }
        if !file_type.is_file() {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        if package_name(&source) != Some(package) {
            continue;
        }
        for name in package_type_names(&source) {
            if wanted.contains(name.as_str()) {
                anyhow::bail!(
                    "`{name}` is declared as a package type in {}; its spelling does not name the predeclared primitive",
                    display(root, &path)
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn package_name(text: &str) -> Option<&str> {
    let at = skip_space(text, 0);
    if !text[at..].starts_with("package")
        || at > 0 && is_ident_byte(text.as_bytes()[at - 1])
        || text
            .as_bytes()
            .get(at + "package".len())
            .is_some_and(|byte| is_ident_byte(*byte))
    {
        return None;
    }
    let start = skip_space(text, at + "package".len());
    let end = identifier_end(text, start);
    (end > start).then_some(&text[start..end])
}

pub(crate) fn identifier_end(text: &str, start: usize) -> usize {
    let mut end = start;
    while end < text.len() && is_ident_byte(text.as_bytes()[end]) {
        end += 1;
    }
    end
}

/// Names introduced by package-level `type` declarations, including parenthesized declaration
/// groups. Comments and literal text are skipped, and declarations inside function bodies are not
/// package declarations.
pub(crate) fn package_type_names(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut names = Vec::new();
    let (mut braces, mut brackets, mut parens, mut at) = (0usize, 0usize, 0usize, 0usize);
    while at < bytes.len() {
        if let Some(end) = skip_opaque(bytes, at) {
            at = end;
            continue;
        }
        match bytes[at] {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b't' if braces == 0
                && brackets == 0
                && parens == 0
                && text[at..].starts_with("type")
                && (at == 0 || !is_ident_byte(bytes[at - 1]))
                && !bytes
                    .get(at + "type".len())
                    .is_some_and(|byte| is_ident_byte(*byte)) =>
            {
                let start = skip_space(text, at + "type".len());
                if bytes.get(start) == Some(&b'(') {
                    let Some(close) = closing(text, start) else {
                        return names;
                    };
                    grouped_type_names(text, start, close, &mut names);
                    at = close + 1;
                    continue;
                }
                let end = identifier_end(text, start);
                if end > start {
                    names.push(text[start..end].to_string());
                }
                at = end;
                continue;
            }
            _ => {}
        }
        at += 1;
    }
    names
}

pub(crate) fn grouped_type_names(text: &str, open: usize, close: usize, names: &mut Vec<String>) {
    let bytes = text.as_bytes();
    let (mut parens, mut brackets, mut braces) = (0usize, 0usize, 0usize);
    let (mut at, mut at_spec_start) = (open + 1, true);
    while at < close {
        if let Some(end) = skip_opaque(bytes, at) {
            if at_spec_start || text[at..end].contains('\n') {
                at_spec_start = true;
            }
            at = end;
            continue;
        }
        if at_spec_start && bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if at_spec_start {
            let end = identifier_end(text, at);
            if end > at {
                names.push(text[at..end].to_string());
                at_spec_start = false;
                at = end;
                continue;
            }
        }
        match bytes[at] {
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b';' if parens == 0 && brackets == 0 && braces == 0 => at_spec_start = true,
            b'\n' if parens == 0 && brackets == 0 && braces == 0 => at_spec_start = true,
            _ => {}
        }
        at += 1;
    }
}
