/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! VFS path normalization, file safety checks, and text position conversions.

use ra_ap_ide::{FileId, TextSize};
use std::path::{Path, PathBuf};

/// 24-bit mask limit (0x007F_FFFF) to ensure EditionedFileId edition bits are never corrupted.
pub const MAX_SAFE_FILE_ID: u32 = 0x007F_FFFF;

pub fn is_safe_file_id(file_id: FileId) -> bool {
    file_id.index() <= MAX_SAFE_FILE_ID
}

/// Canonical path normalizer for VFS keys: removes `.` and `..` lexically, resolves absolute path.
pub fn normalize_vfs_path(path: &Path, workspace_root: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };

    let mut components = Vec::new();
    for comp in abs.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                components.pop();
            }
            c => components.push(c),
        }
    }
    components.into_iter().collect()
}

/// Whether `path` is a Rust source file by its extension.
pub(crate) fn is_rust_source(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "rs")
}

/// Convert 1-indexed (line, col) to 0-indexed byte offset. The column counts UTF-16 code units,
/// as the LSP positions the gateway answers with do (#456); one between the two halves of a
/// surrogate pair is on no character. A CRLF pair is one line break, so neither byte adds a
/// source column. A zero line or column, or a column past the end of its own line (rather than
/// clamped into whatever text follows), is on no position: `None`, never a nearby offset.
pub(crate) fn line_col_to_offset(
    text: &str,
    target_line: u32,
    target_col: u32,
) -> Option<TextSize> {
    if target_line == 0 || target_col == 0 {
        return None;
    }
    let mut current_line = 1;
    let mut line_start = 0;

    for (i, c) in text.char_indices() {
        if current_line == target_line {
            // Bound the slice to this line alone: scanning past its `\n` let an oversized
            // column silently resolve into a later line, or clamp to the end of the file.
            let line_end = text[line_start..]
                .find('\n')
                .map_or(text.len(), |rel| line_start + rel);
            // A CR immediately before LF is part of the line encoding, not a source column.
            let line_end = if line_end < text.len()
                && line_end > line_start
                && text.as_bytes()[line_end - 1] == b'\r'
            {
                line_end - 1
            } else {
                line_end
            };
            let line_slice = &text[line_start..line_end];
            let mut current_col = 1u32;
            for (col_offset, ch) in line_slice.char_indices() {
                if current_col >= target_col {
                    return (current_col == target_col)
                        .then(|| TextSize::from((line_start + col_offset) as u32));
                }
                current_col += ch.len_utf16() as u32;
            }
            return (current_col == target_col).then(|| TextSize::from(line_end as u32));
        }

        if c == '\n' {
            current_line += 1;
            line_start = i + 1;
        }
    }

    if current_line == target_line && target_col == 1 {
        return Some(TextSize::from(line_start as u32));
    }

    None
}

/// Convert 0-indexed byte offset to 1-indexed (line, col), the column in UTF-16 code units.
/// A CRLF pair is one logical line break, so an offset at either delimiter never produces a
/// position between them.
pub(crate) fn offset_to_line_col(text: &str, offset: TextSize) -> (u32, u32) {
    let target = usize::from(offset);
    let mut line = 1;
    let mut col = 1;

    for (i, c) in text.char_indices() {
        if i >= target {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else if c != '\r' || text.as_bytes().get(i + 1) != Some(&b'\n') {
            col += c.len_utf16() as u32;
        }
    }

    (line, col)
}

/// Whether a `use` in a scope enclosing `node` (a block, a module or the file) imports `name`
/// (#181). An import rust-analyzer cannot resolve is reported at the `use`, so its uses are
/// not reported again. Inside a function that an attribute macro such as `#[tokio::test]`
/// expands, rust-analyzer also does not resolve a type in a `let` annotation through a `use`
/// in the body, which rustc does.
pub(crate) fn imported_in_scope(node: &ra_ap_syntax::SyntaxNode, name: &str) -> bool {
    use ra_ap_syntax::AstNode;
    use ra_ap_syntax::ast::{self, HasModuleItem};
    let imports = |item: ast::Item| match item {
        ast::Item::Use(import) => import
            .syntax()
            .descendants()
            .filter_map(ast::NameRef::cast)
            .any(|n| n.text() == name),
        _ => false,
    };
    node.ancestors().any(|scope| {
        if let Some(block) = ast::StmtList::cast(scope.clone()) {
            block.statements().any(|stmt| match stmt {
                ast::Stmt::Item(item) => imports(item),
                _ => false,
            })
        } else if let Some(module) = ast::ItemList::cast(scope.clone()) {
            module.items().any(imports)
        } else if let Some(file) = ast::SourceFile::cast(scope) {
            file.items().any(imports)
        } else {
            false
        }
    })
}

/// Where the path of a `use` item starts, when `item` (the text from the item's first
/// non-blank character on) is one: `use a::b;`, `pub use`, `pub(crate) use`.
pub(crate) fn use_path_start(item: &str) -> Option<usize> {
    let mut rest = item;
    if let Some(after) = rest.strip_prefix("pub") {
        rest = match after.strip_prefix('(') {
            Some(scoped) => &scoped[scoped.find(')')? + 1..],
            None if after.starts_with(char::is_whitespace) => after,
            None => return None,
        };
        rest = rest.trim_start();
    }
    let after_use = rest.strip_prefix("use")?;
    let path = after_use.trim_start();
    (after_use.starts_with(char::is_whitespace) && !path.is_empty())
        .then(|| item.len() - path.len())
}
