/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::lex::{opaque_end, skip_trivia};

pub(crate) fn refuse_attached_directives(text: &str, start: usize) -> Result<()> {
    let line_start = text[..start].rfind('\n').map_or(0, |newline| newline + 1);
    anyhow::ensure!(
        skip_trivia(text, line_start)? == start,
        "the declaration range begins after non-comment source on the same line"
    );
    let mut lines = text[..line_start].lines().rev();
    for line in &mut lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if !trimmed.starts_with("//") {
            break;
        }
        anyhow::ensure!(
            !trimmed.starts_with("//go:") && !trimmed.starts_with("//export "),
            "a Go compiler or cgo directive is attached to the function"
        );
    }
    Ok(())
}

pub(crate) fn is_generated(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut cursor = usize::from(text.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
    while cursor < bytes.len() {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let rest = &bytes[cursor..];
        if rest.starts_with(b"//") {
            let end = rest
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |offset| cursor + offset);
            let line = text[cursor..end].trim_end_matches('\r');
            if let Some(marker) = line.strip_prefix("// Code generated ")
                && marker.ends_with(" DO NOT EDIT.")
            {
                return true;
            }
            cursor = end;
            continue;
        }
        if rest.starts_with(b"/*") {
            let Some(end) = rest[2..].windows(2).position(|window| window == b"*/") else {
                return false;
            };
            cursor += end + 4;
            continue;
        }
        return false;
    }
    false
}

pub(crate) fn refuse_source_directives(text: &str) -> Result<()> {
    let mut cursor = 0usize;
    while cursor < text.len() {
        let rest = &text.as_bytes()[cursor..];
        anyhow::ensure!(
            !rest.starts_with(b"//line ") && !rest.starts_with(b"/*line "),
            "Go source line directives are not supported; nothing was written"
        );
        cursor = opaque_end(text, cursor)?.unwrap_or(cursor + 1);
    }
    Ok(())
}

pub(crate) fn current_text(path: &Path, expected: &str) -> Result<bool> {
    Ok(std::fs::read_to_string(path)
        .with_context(|| format!("cannot reread {}", path.display()))?
        == expected)
}

pub(crate) fn refuse_linked_source_path(
    root: &Path,
    canonical_root: &Path,
    source: &Path,
) -> Result<()> {
    let relative = source
        .strip_prefix(root)
        .or_else(|_| source.strip_prefix(canonical_root))
        .with_context(|| {
            format!(
                "{} is outside the checkout {}; nothing was written",
                source.display(),
                root.display()
            )
        })?;
    let mut current = canonical_root.to_path_buf();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => continue,
            std::path::Component::Normal(name) => current.push(name),
            _ => anyhow::bail!(
                "{} is outside the checkout {}; nothing was written",
                source.display(),
                root.display()
            ),
        }
        let metadata = std::fs::symlink_metadata(&current).with_context(|| {
            format!("cannot inspect {}; nothing was written", current.display())
        })?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "the requested Go source contains linked source path {}; deletion is refused",
            current.display()
        );
    }
    Ok(())
}

pub(crate) fn regular_unlinked_inside(root: &Path, path: &Path) -> Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect {}; nothing was written", path.display()))?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "{} is not an unlinked regular source file; nothing was written",
        path.display()
    );
    let canonical = std::fs::canonicalize(path)
        .with_context(|| format!("cannot resolve {}; nothing was written", path.display()))?;
    anyhow::ensure!(
        canonical.starts_with(root),
        "{} is outside the checkout {}; nothing was written",
        path.display(),
        root.display()
    );
    Ok(canonical)
}

pub(crate) fn go_module_root(root: &Path, source: &Path) -> Result<PathBuf> {
    let mut directory = source.parent();
    while let Some(candidate) = directory {
        anyhow::ensure!(
            candidate.starts_with(root),
            "the source is outside the checkout"
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
        "no Go module contains {}; nothing was written",
        source.display()
    )
}

pub(crate) fn refuse_linked_sources(module: &Path) -> Result<()> {
    fn is_ignored_dir_name(name: &str) -> bool {
        name.starts_with('.')
            || name.starts_with('_')
            || matches!(name, "testdata" | "vendor" | "node_modules" | "target")
            || name.ends_with(".framework")
            || name.ends_with(".xcframework")
    }

    fn dir_contains_go(dir: &Path, depth: usize) -> bool {
        if depth > 16 {
            return true;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return true;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "go") {
                return true;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && is_ignored_dir_name(name)
            {
                continue;
            }
            if path.is_dir() && dir_contains_go(&path, depth + 1) {
                return true;
            }
        }
        false
    }

    fn visit(directory: &Path) -> Result<Option<PathBuf>> {
        for entry in std::fs::read_dir(directory)
            .with_context(|| format!("cannot inspect Go module {}", directory.display()))?
        {
            let entry = entry.context("cannot inspect a Go module entry")?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .with_context(|| format!("cannot inspect {}", path.display()))?;
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && !metadata.file_type().is_symlink()
                && is_ignored_dir_name(name)
            {
                continue;
            }
            if metadata.file_type().is_symlink() {
                let hides_go = path.extension().is_some_and(|extension| extension == "go")
                    || std::fs::metadata(&path)
                        .is_ok_and(|target| target.is_dir() && dir_contains_go(&path, 0));
                if hides_go {
                    return Ok(Some(path));
                }
            } else if metadata.is_dir()
                && let Some(link) = visit(&path)?
            {
                return Ok(Some(link));
            }
        }
        Ok(None)
    }
    if let Some(link) = visit(module)? {
        anyhow::bail!(
            "the Go module contains linked source path {}; deletion is refused",
            link.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::refuse_linked_sources;

    #[cfg(unix)]
    #[test]
    fn non_go_framework_and_dotbuild_symlinks_do_not_block_safe_delete() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("go.mod"), "module example.com/m\n").unwrap();
        std::fs::write(root.join("main.go"), "package main\n").unwrap();

        let build_dir = root.join("macos/.build/artifacts/WebRTC.xcframework/Versions/A/Resources");
        std::fs::create_dir_all(&build_dir).unwrap();
        std::os::unix::fs::symlink(
            "Versions/A/Resources",
            root.join("macos/.build/artifacts/WebRTC.xcframework/Resources"),
        )
        .unwrap();

        let ext_non_go = dir.path().join("external_assets");
        std::fs::create_dir_all(&ext_non_go).unwrap();
        std::fs::write(ext_non_go.join("info.plist"), "<plist/>").unwrap();
        std::os::unix::fs::symlink(&ext_non_go, root.join("assets_link")).unwrap();

        assert!(
            refuse_linked_sources(root).is_ok(),
            "non-Go symlinks and .build/.xcframework artifacts should be ignored"
        );

        let ext_go = dir.path().join("external_go");
        std::fs::create_dir_all(&ext_go).unwrap();
        std::fs::write(ext_go.join("hidden.go"), "package hidden\n").unwrap();
        std::os::unix::fs::symlink(&ext_go, root.join("pkg_link")).unwrap();

        assert!(
            refuse_linked_sources(root).is_err(),
            "directory symlink containing .go sources must still be refused"
        );
    }
}
