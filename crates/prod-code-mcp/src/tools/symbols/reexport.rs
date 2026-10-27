/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A qualified Rust name written by its `pub use` path (#1582): `tools::render_outline` for an
//! item defined in `tools/outline/render.rs` that `tools.rs` re-exports. The index answers only
//! the defining path, so an ancestor module that re-exports the name is looked for in its file.

use std::path::{Path, PathBuf};

use super::matching::module_path_ends_with;
use super::types::SymbolHit;

/// Whether the Rust item `hit`, named `name`, is re-exported by a module that encloses it and
/// whose path ends with `qualifiers`: that module's file has a `pub use` naming `name` (not
/// renamed with `as`). A glob re-export is not followed, since it does not say which items.
pub(crate) fn reexported_as(root: &Path, hit: &SymbolHit, qualifiers: &[&str], name: &str) -> bool {
    if qualifiers.is_empty() || hit.path.extension().is_none_or(|ext| ext != "rs") {
        return false;
    }
    enclosing_module_files(root, &hit.path)
        .into_iter()
        .filter(|file| module_path_ends_with(root, file, qualifiers))
        .any(|file| std::fs::read_to_string(&file).is_ok_and(|text| names_in_pub_use(&text, name)))
}

/// The files of the modules that enclose the item in `file`, innermost first, up to its crate's
/// root (`lib.rs` or `main.rs`) and never above `root`.
fn enclosing_module_files(root: &Path, file: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dir = file.parent();
    while let Some(d) = dir {
        if !d.starts_with(root) || d == root {
            break;
        }
        if d.file_name().is_some_and(|n| n == "src") {
            out.extend(
                ["lib.rs", "main.rs"]
                    .iter()
                    .map(|f| d.join(f))
                    .filter(|p| p.as_path() != file && p.is_file()),
            );
            break;
        }
        out.extend(
            [d.with_extension("rs"), d.join("mod.rs")]
                .into_iter()
                .filter(|p| p.as_path() != file && p.is_file()),
        );
        dir = d.parent();
    }
    out
}

/// Whether a `pub use` (or `pub(…) use`) statement in `text` names `name` as what it exports.
fn names_in_pub_use(text: &str, name: &str) -> bool {
    let mut rest = text;
    while let Some(at) = rest.find("pub") {
        let after = &rest[at + 3..];
        rest = after;
        let after = match after.trim_start().strip_prefix('(') {
            Some(scoped) => match scoped.find(')') {
                Some(close) => &scoped[close + 1..],
                None => continue,
            },
            None => after,
        };
        let Some(body) = after.trim_start().strip_prefix("use ") else {
            continue;
        };
        let statement = &body[..body.find(';').unwrap_or(body.len())];
        if exports_word(statement, name) {
            return true;
        }
    }
    false
}

/// Whether `name` is a whole identifier of `statement` that the statement exports under that
/// name: the last segment of a path, not followed by `::` or renamed with `as`.
fn exports_word(statement: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(found) = statement[from..].find(name) {
        let at = from + found;
        let end = at + name.len();
        from = end;
        let before = statement[..at].chars().next_back();
        let after_char = statement[end..].chars().next();
        if before.is_some_and(is_ident) || after_char.is_some_and(is_ident) {
            continue;
        }
        let after = statement[end..].trim_start();
        if after.starts_with("::") || after.starts_with("as ") || after.starts_with("as\n") {
            continue;
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pub_use_names_what_it_exports() {
        assert!(names_in_pub_use(
            "mod outline;\npub use outline::{OutlineOptions, render_outline, render_outline_with};\n",
            "render_outline"
        ));
        assert!(names_in_pub_use(
            "pub(crate) use change::change_with;",
            "change_with"
        ));
        assert!(!names_in_pub_use(
            "use outline::render_outline;",
            "render_outline"
        ));
        assert!(!names_in_pub_use(
            "pub use outline::render_outline as outline;",
            "render_outline"
        ));
        assert!(!names_in_pub_use(
            "pub use render_outline::Thing;",
            "render_outline"
        ));
        assert!(!names_in_pub_use("pub use outline::*;", "render_outline"));
        assert!(!names_in_pub_use(
            "pub use outline::render_outline_with;",
            "render_outline"
        ));
    }
}
