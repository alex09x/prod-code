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
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::lexer::{advance, ascii_identifier_end, keyword_at, opaque_end, pair, skip_trivia};

pub fn collect_sources(
    project: &Path,
    directory: &Path,
    sources: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in std::fs::read_dir(directory)
        .with_context(|| format!("cannot inspect TypeScript project {}", directory.display()))?
    {
        let entry = entry.context("cannot inspect a TypeScript project entry")?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "the TypeScript project contains linked path {}; deletion is refused",
            display(project, &path)
        );
        if metadata.is_dir() {
            collect_sources(project, &path, sources)?;
            continue;
        }
        let extension = path.extension().and_then(|value| value.to_str());
        if matches!(extension, Some("js" | "jsx" | "mjs" | "cjs" | "tsx")) {
            anyhow::bail!(
                "{} is JavaScript or TSX; only contained .ts projects are supported",
                display(project, &path)
            );
        }
        if extension == Some("ts") {
            anyhow::ensure!(
                !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".d.ts")),
                "ambient declaration source {} is not supported",
                display(project, &path)
            );
            sources.insert(std::fs::canonicalize(&path)?);
        }
    }
    Ok(())
}

pub fn inspect_source(text: &str) -> Result<()> {
    let header = text
        .get(..text.len().min(4096))
        .unwrap_or(text)
        .to_ascii_lowercase();
    anyhow::ensure!(
        !header.contains("@generated")
            && !header.contains("generated file")
            && !header.contains("do not edit")
            && !text.contains("sourceMappingURL="),
        "generated TypeScript source is not supported"
    );
    let mut cursor = 0usize;
    while cursor < text.len() {
        if let Some(end) = opaque_end(text, cursor)? {
            cursor = end;
            continue;
        }
        if text.as_bytes()[cursor] == b'/' {
            anyhow::bail!(
                "regular-expression and division syntax is outside the safe-delete subset"
            );
        }
        if let Some(end) = ascii_identifier_end(text, cursor) {
            let word = &text[cursor..end];
            let next = skip_trivia(text, end)?;
            if word == "eval"
                || (matches!(word, "Function" | "require" | "import")
                    && text.as_bytes().get(next) == Some(&b'('))
            {
                anyhow::bail!("dynamic evaluation or name resolution is not supported");
            }
            cursor = end;
        } else {
            anyhow::ensure!(
                text.as_bytes()[cursor] != b'\\',
                "escaped identifiers are outside the safe-delete subset"
            );
            cursor = advance(text, cursor);
        }
    }
    Ok(())
}

pub fn module_marker(text: &str) -> Result<bool> {
    let mut cursor = 0usize;
    let mut stack = Vec::new();
    while cursor < text.len() {
        if let Some(end) = opaque_end(text, cursor)? {
            cursor = end;
            continue;
        }
        let byte = text.as_bytes()[cursor];
        match byte {
            b'(' | b'[' | b'{' => stack.push(byte),
            b')' | b']' | b'}' => {
                let open = stack
                    .pop()
                    .context("unmatched delimiter in TypeScript source")?;
                anyhow::ensure!(
                    pair(open, byte),
                    "mismatched delimiter in TypeScript source"
                );
            }
            _ if stack.is_empty()
                && (keyword_at(text, cursor, "import") || keyword_at(text, cursor, "export")) =>
            {
                return Ok(true);
            }
            _ => {}
        }
        cursor = advance(text, cursor);
    }
    anyhow::ensure!(stack.is_empty(), "unclosed delimiter in TypeScript source");
    Ok(false)
}

pub fn project_snapshot(project: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
        for entry in std::fs::read_dir(directory)
            .with_context(|| format!("cannot snapshot {}", directory.display()))?
        {
            let entry = entry.context("cannot inspect a TypeScript project entry")?;
            let path = entry.path();
            let name = path.file_name().and_then(|name| name.to_str());
            if matches!(name, Some(".git" | "node_modules" | "target")) {
                continue;
            }
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("cannot inspect {}", path.display()))?;
            anyhow::ensure!(
                !metadata.file_type().is_symlink(),
                "the TypeScript project contains linked path {}; deletion is refused",
                display(root, &path)
            );
            if metadata.is_dir() {
                visit(root, &path, files)?;
            } else if metadata.is_file() {
                files.insert(
                    path.strip_prefix(root)
                        .expect("entry below root")
                        .to_path_buf(),
                    std::fs::read(&path)
                        .with_context(|| format!("cannot snapshot {}", path.display()))?,
                );
            } else {
                anyhow::bail!("the TypeScript project contains a special file");
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(project, project, &mut files)?;
    Ok(files)
}

pub fn unchanged(project: &Path, observed: &BTreeMap<PathBuf, Vec<u8>>, stage: &str) -> Result<()> {
    anyhow::ensure!(
        project_snapshot(project)? == *observed,
        "the TypeScript project changed while {stage} was inspected; nothing was written"
    );
    Ok(())
}

pub fn refuse_linked_path(root: &Path, canonical_root: &Path, source: &Path) -> Result<()> {
    let relative = source
        .strip_prefix(root)
        .or_else(|_| source.strip_prefix(canonical_root))
        .with_context(|| {
            format!(
                "{} is outside the checkout; nothing was written",
                source.display()
            )
        })?;
    let mut current = canonical_root.to_path_buf();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => continue,
            std::path::Component::Normal(name) => current.push(name),
            _ => anyhow::bail!(
                "{} is outside the checkout; nothing was written",
                source.display()
            ),
        }
        let metadata = std::fs::symlink_metadata(&current).with_context(|| {
            format!("cannot inspect {}; nothing was written", current.display())
        })?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "the requested TypeScript path contains linked path {}; deletion is refused",
            current.display()
        );
    }
    Ok(())
}

pub fn regular_unlinked_inside(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("cannot inspect {}; nothing was written", path.display()))?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "{} is not an unlinked regular source file; nothing was written",
        path.display()
    );
    let canonical = std::fs::canonicalize(&path)
        .with_context(|| format!("cannot resolve {}; nothing was written", path.display()))?;
    anyhow::ensure!(
        canonical.starts_with(root),
        "{} is outside the TypeScript project; nothing was written",
        path.display()
    );
    Ok(canonical)
}

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
