/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{ManifestOverlay, join_checked};
use crate::extract_trait::lex::lexical_code;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Follow literal module declarations from a proven target root. Unlinked files,
/// macro-generated modules and computed includes cannot establish ownership.
pub(super) fn sources(
    overlay: &ManifestOverlay<'_>,
    target: &Path,
    proven: bool,
) -> BTreeMap<PathBuf, bool> {
    let mut sources = BTreeMap::new();
    visit(overlay, target, true, proven, &mut sources);
    sources
}

fn visit(
    overlay: &ManifestOverlay<'_>,
    file: &Path,
    root: bool,
    proven: bool,
    seen: &mut BTreeMap<PathBuf, bool>,
) {
    if !overlay.files.contains(file) {
        return;
    }
    let Some(text) = overlay.read(file) else {
        return;
    };
    let code = lexical_code(&text);
    let conditional = text.match_indices("#![").any(|(i, _)| {
        code[i]
            && text[i + 3..].split(']').next().is_some_and(|attr| {
                let attr = attr.split_whitespace().collect::<String>();
                attr.starts_with("cfg") && attr != "cfg(test)"
            })
    });
    let proven = proven && !conditional;
    if seen.get(file).is_some_and(|old| *old || !proven) {
        return;
    }
    seen.insert(file.to_path_buf(), proven);
    let parent = file.parent().unwrap_or(Path::new(""));
    let module_dir = if root || file.file_name().is_some_and(|n| n == "mod.rs") {
        parent.to_path_buf()
    } else {
        file.with_extension("")
    };
    scope(overlay, &text, &module_dir, parent, proven, seen);
}

fn scope(
    overlay: &ManifestOverlay<'_>,
    text: &str,
    module_dir: &Path,
    path_dir: &Path,
    proven: bool,
    seen: &mut BTreeMap<PathBuf, bool>,
) {
    let code = lexical_code(text);
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut path = None;
    let mut unsupported_path = false;
    let mut conditional = false;
    while i < bytes.len() {
        if !code[i] || bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i] == b'#' && bytes.get(i + 1) == Some(&b'[') {
            let Some(end) = close(bytes, &code, i + 1, b'[', b']') else {
                return;
            };
            let attr = text[i + 2..end].trim();
            if attr.starts_with("path") {
                path = literal_path(attr);
                unsupported_path = path.is_none();
            } else if attr.starts_with("cfg_attr") && attr.contains("path") {
                unsupported_path = true;
            }
            if attr.starts_with("cfg") && attr.split_whitespace().collect::<String>() != "cfg(test)"
            {
                conditional = true;
            }
            i = end + 1;
            continue;
        }
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if &text[start..i] != "mod" {
                continue;
            }
            let name_start = skip_space(bytes, i);
            let mut name_end = name_start;
            while name_end < bytes.len()
                && (bytes[name_end].is_ascii_alphanumeric() || bytes[name_end] == b'_')
            {
                name_end += 1;
            }
            if name_end == name_start {
                continue;
            }
            let name = &text[name_start..name_end];
            let delimiter = skip_space(bytes, name_end);
            if unsupported_path {
                path = None;
                unsupported_path = false;
                i = name_end;
                continue;
            }
            match bytes.get(delimiter) {
                Some(b';') => {
                    if let Some(raw) = path.take() {
                        if let Some(file) = join_checked(path_dir, Path::new(&raw)) {
                            visit(overlay, &file, false, proven && !conditional, seen);
                        }
                    } else {
                        visit(
                            overlay,
                            &module_dir.join(format!("{name}.rs")),
                            false,
                            proven && !conditional,
                            seen,
                        );
                        visit(
                            overlay,
                            &module_dir.join(name).join("mod.rs"),
                            false,
                            proven && !conditional,
                            seen,
                        );
                    }
                    i = delimiter + 1;
                    conditional = false;
                }
                Some(b'{') => {
                    let Some(end) = close(bytes, &code, delimiter, b'{', b'}') else {
                        return;
                    };
                    let dir = path
                        .take()
                        .and_then(|raw| join_checked(path_dir, Path::new(&raw)))
                        .unwrap_or_else(|| module_dir.join(name));
                    scope(
                        overlay,
                        &text[delimiter + 1..end],
                        &dir,
                        &dir,
                        proven && !conditional,
                        seen,
                    );
                    i = end + 1;
                    conditional = false;
                }
                _ => {
                    path = None;
                }
            }
        } else if matches!(bytes[i], b'{' | b'(') {
            let right = if bytes[i] == b'{' { b'}' } else { b')' };
            let Some(end) = close(bytes, &code, i, bytes[i], right) else {
                return;
            };
            i = end + 1;
            if bytes[end] == b'}' {
                path = None;
                unsupported_path = false;
                conditional = false;
            }
        } else {
            i += 1;
        }
    }
}

fn skip_space(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

fn close(bytes: &[u8], code: &[bool], start: usize, left: u8, right: u8) -> Option<usize> {
    let mut depth = 0usize;
    for i in start..bytes.len() {
        if !code[i] {
            continue;
        }
        if bytes[i] == left {
            depth += 1;
        }
        if bytes[i] == right {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn literal_path(attr: &str) -> Option<String> {
    let doc: toml::Value = toml::from_str(attr).ok()?;
    doc.get("path")?.as_str().map(str::to_string)
}
