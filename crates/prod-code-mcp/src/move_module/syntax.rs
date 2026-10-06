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

use crate::move_item::{ModulePath, parent_module_file};

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The byte range of the top-level `mod name;` declaration in `text`, from its doc comment and
/// attributes to the end of its line, and the declaration line itself. `None` when the module is
/// not declared that way.
pub fn declaration(text: &str, name: &str) -> Option<(usize, usize, String)> {
    let mut offset = 0;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_end();
        let decl = t
            .strip_suffix(&format!("mod {name};"))
            .is_some_and(|vis| vis.is_empty() || (vis.starts_with("pub") && vis.ends_with(' ')));
        if decl && !line.starts_with(char::is_whitespace) {
            let mut start = offset;
            for above in lines[..i].iter().rev() {
                let a = above.trim_start();
                if a.starts_with("///") || a.starts_with("#[") {
                    start -= above.len();
                } else {
                    break;
                }
            }
            return Some((start, offset + line.len(), t.to_string()));
        }
        offset += line.len();
    }
    None
}

/// `text` with `block` (a module declaration with its attributes) declared where
/// [`crate::move_item::declare_module`] would put a bare one.
pub fn declare_block(text: &str, name: &str, block: &str) -> String {
    let placed = crate::move_item::declare_module(text, name, false);
    let bare = format!("mod {name};");
    match placed.lines().position(|l| l == bare) {
        Some(at) => {
            let mut lines: Vec<String> = placed.lines().map(str::to_string).collect();
            lines[at] = block.trim_end().to_string();
            let mut out = lines.join("\n");
            out.push('\n');
            out
        }
        None => placed,
    }
}

/// `text` with every `super::` (as a path's head, not the tail of a longer name) made `head::`.
pub fn resolve_super(text: &str, head: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    let mut at = 0;
    for (i, _) in text.match_indices("super::") {
        // Not the tail of a longer name, and not the second `super` of `super::super::`.
        if i < at
            || text[..i].chars().next_back().is_some_and(is_ident)
            || text[..i].ends_with("::")
        {
            continue;
        }
        // `super::super::` reaches past the old parent; it is left for the type check to name.
        if text[i + 7..].starts_with("super::") {
            continue;
        }
        out.push_str(&text[at..i]);
        out.push_str(head);
        out.push_str("::");
        at = i + "super::".len();
        count += 1;
    }
    out.push_str(&text[at..]);
    (out, count)
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
pub(crate) fn declaring_file(file: &Path) -> Option<PathBuf> {
    let module_file = if file.file_name().is_some_and(|n| n == "mod.rs") {
        file.parent()?.with_extension("rs")
    } else {
        file.to_path_buf()
    };
    parent_module_file(&module_file)
}

/// Every file under `dir`, recursively.
pub(crate) fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}

pub(crate) fn parent_of(module: &ModulePath) -> ModulePath {
    let mut segments = module.segments.clone();
    segments.pop();
    ModulePath {
        krate: module.krate.clone(),
        segments,
    }
}
