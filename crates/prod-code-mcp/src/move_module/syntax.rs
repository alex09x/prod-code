/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[cfg(test)]
use std::path::{Path, PathBuf};

use crate::move_item::ModulePath;
#[cfg(test)]
use crate::move_item::parent_module_file;

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The byte range of the top-level `mod name;` declaration in `text`, from its doc comment and
/// attributes to the end of its line, and the declaration line itself. `None` when the module is
/// not declared that way.
pub fn declaration(text: &str, name: &str) -> Option<(usize, usize, String)> {
    crate::move_item::rust_syntax::declaration(text, name)
}

/// `text` with `block` (a module declaration with its attributes) declared where
/// [`crate::move_item::declare_module`] would put a bare one.
pub fn declare_block(text: &str, name: &str, block: &str) -> String {
    declare_block_in(text, name, block, ra_ap_syntax::Edition::CURRENT)
}

pub(super) fn declare_block_in(
    text: &str,
    name: &str,
    block: &str,
    edition: ra_ap_syntax::Edition,
) -> String {
    let mut placed = crate::move_item::declare_module_in(text, name, false, edition);
    if let Some((start, end, _)) =
        crate::move_item::rust_syntax::declaration_in(&placed, name, edition)
    {
        placed.replace_range(start..end, block);
        if !block.ends_with('\n') {
            placed.insert(start + block.len(), '\n');
        }
    }
    placed
}

/// `text` with every `super::` (as a path's head, not the tail of a longer name) made `head::`.
pub fn resolve_super(text: &str, head: &str) -> (String, usize) {
    let mut old: Vec<_> = head.split("::").skip(1).map(str::to_string).collect();
    old.push("__moved".to_string());
    crate::move_item::rust_syntax::rebase_super(text, &old, 0, ra_ap_syntax::Edition::CURRENT)
        .unwrap_or_else(|_| (text.to_string(), 0))
}

/// How a reference to the module is spelled at `offset` of `text`: whether it stands in a `use`
/// group (`{b, x}`), and where its path begins when it is qualified.
#[derive(Debug, PartialEq)]
pub enum Spelling {
    Bare,
    Grouped,
    Qualified { path_start: usize },
}

pub fn spelling(text: &str, offset: usize) -> Spelling {
    let path_start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c) || *c == ':')
        .last()
        .map_or(offset, |(i, _)| i);
    if path_start < offset {
        return Spelling::Qualified { path_start };
    }
    // A group is `{b, c}` inside a `use`; a `{` that opens a block is not one.
    let statement = text[..offset]
        .rfind([';', '}'])
        .map_or(&text[..offset], |i| &text[i + 1..offset])
        .trim_start();
    let in_use = ["use ", "pub use ", "pub(crate) use "]
        .iter()
        .any(|head| statement.starts_with(head));
    match text[..offset].trim_end().chars().next_back() {
        Some('{') | Some(',') if in_use => Spelling::Grouped,
        _ => Spelling::Bare,
    }
}

/// The file that declares a module whose file is `file`: `src/a.rs` for `src/a/b.rs` and for
/// `src/a/b/mod.rs`.
#[cfg(test)]
pub(crate) fn declaring_file(file: &Path) -> Option<PathBuf> {
    let module_file = if file.file_name().is_some_and(|n| n == "mod.rs") {
        file.parent()?.with_extension("rs")
    } else {
        file.to_path_buf()
    };
    parent_module_file(&module_file)
}

/// Every file under `dir`, recursively.
#[cfg(test)]
pub(crate) fn files_under(dir: &Path) -> Vec<PathBuf> {
    crate::move_item::ownership::filesystem::files(dir, dir)
        .expect("test fixture must contain only regular files and directories")
}

pub(crate) fn parent_of(module: &ModulePath) -> ModulePath {
    let mut segments = module.segments.clone();
    segments.pop();
    ModulePath {
        krate: module.krate.clone(),
        segments,
    }
}
