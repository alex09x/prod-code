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

/// Whether `tree` in a `use` item binds `name` in the importing scope.
fn use_tree_binds(tree: &ra_ap_syntax::ast::UseTree, name: &str) -> bool {
    use ra_ap_syntax::ast::{self, HasName, PathSegmentKind};
    if let Some(list) = tree.use_tree_list() {
        return list.use_trees().any(|sub| use_tree_binds(&sub, name));
    }
    if tree.star_token().is_some() {
        return false;
    }
    if let Some(rename) = tree.rename() {
        return rename.name().is_some_and(|n| n.text() == name);
    }
    if tree.path().is_none() {
        return false;
    }
    let bound = std::iter::successors(Some(tree.clone()), |t| {
        t.parent_use_tree_list().map(|l| l.parent_use_tree())
    })
    .filter_map(|t| t.path())
    .flat_map(|p| std::iter::successors(Some(p), ast::Path::qualifier))
    .filter_map(|p| p.segment())
    .find(|s| s.self_token().is_none());
    matches!(
        bound.and_then(|s| s.kind()),
        Some(PathSegmentKind::Name(n)) if n.text() == name
    )
}

/// Whether a `use` in a scope enclosing `node` (an enclosing block or the current module or
/// file) imports `name` (#181, #1248). An import rust-analyzer cannot resolve is reported at
/// the `use`, so its uses are not reported again. Inside a function that an attribute macro
/// such as `#[tokio::test]` expands, rust-analyzer also does not resolve a type in a `let`
/// annotation through a `use` in the body, which rustc does.
pub(crate) fn imported_in_scope(node: &ra_ap_syntax::SyntaxNode, name: &str) -> bool {
    use ra_ap_syntax::AstNode;
    use ra_ap_syntax::ast::{self, HasModuleItem};
    let imports = |item: ast::Item| match item {
        ast::Item::Use(import) => import
            .use_tree()
            .is_some_and(|tree| use_tree_binds(&tree, name)),
        _ => false,
    };
    for scope in node.ancestors() {
        if let Some(block) = ast::StmtList::cast(scope.clone()) {
            if block.statements().any(|stmt| match stmt {
                ast::Stmt::Item(item) => imports(item),
                _ => false,
            }) {
                return true;
            }
        } else if let Some(module) = ast::ItemList::cast(scope.clone()) {
            return module.items().any(imports);
        } else if let Some(file) = ast::SourceFile::cast(scope) {
            return file.items().any(imports);
        }
    }
    false
}

/// Each `use` item in `file`, as the offset where its imported path starts and the range of
/// the item itself (from its visibility or `use` keyword through its semicolon).
pub(crate) fn use_items(
    file: &ra_ap_syntax::SourceFile,
) -> Vec<(ra_ap_ide::TextSize, ra_ap_ide::TextRange)> {
    use ra_ap_syntax::AstNode;
    use ra_ap_syntax::ast::{self, HasVisibility};
    file.syntax()
        .descendants()
        .filter_map(ast::Use::cast)
        .filter_map(|item| {
            let at = item.use_tree()?.syntax().text_range().start();
            let start = item
                .visibility()
                .map(|v| v.syntax().text_range().start())
                .or_else(|| item.use_token().map(|t| t.text_range().start()))
                .unwrap_or_else(|| item.syntax().text_range().start());
            let range = ra_ap_ide::TextRange::new(start, item.syntax().text_range().end());
            Some((at, range))
        })
        .collect()
}
