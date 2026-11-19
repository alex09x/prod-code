/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Whether a Rust source line is inside a block of a named type: its `impl`, or its `struct`,
//! `enum`, `union` or `trait` body (#2045).

/// Whether 0-based `line` of `text` is a direct member of a block whose header names `owner`:
/// `impl Owner`, `impl Trait for Owner`, `impl<T> Owner<T>`, `struct Owner {`, and so on. The
/// header is the nearest line above with less indentation; a line at the top level is in no
/// block.
pub(crate) fn inside_impl_of(text: &str, line: usize, owner: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    let Some(target) = lines.get(line) else {
        return false;
    };
    let indent = |row: &str| row.len() - row.trim_start().len();
    let depth = indent(target);
    if depth == 0 {
        return false;
    }
    // The block directly around the line: a local of a method (`let cancel`) is inside the
    // method, not a member of the `impl` around it.
    lines[..line]
        .iter()
        .rev()
        .map(|row| (indent(row), row.trim_start()))
        .filter(|(_, trimmed)| {
            !trimmed.is_empty() && !trimmed.starts_with("//") && !trimmed.starts_with('#')
        })
        .find(|(level, _)| *level < depth)
        .is_some_and(|(_, header)| names_owner(header, owner))
}

/// Whether a block header names `owner` as its type.
fn names_owner(header: &str, owner: &str) -> bool {
    let header = header
        .trim_start_matches("pub(crate) ")
        .trim_start_matches("pub(super) ")
        .trim_start_matches("pub ")
        .trim_start_matches("unsafe ");
    let typed = if let Some(rest) = header.strip_prefix("impl") {
        if !rest.starts_with([' ', '<']) {
            return false;
        }
        // The type of `impl Trait for Type`, or of `impl Type`.
        let rest = rest.split('{').next().unwrap_or(rest);
        let rest = rest.split(" where ").next().unwrap_or(rest);
        rest.rsplit(" for ").next().unwrap_or(rest).to_string()
    } else if let Some(rest) = ["struct ", "enum ", "union ", "trait "]
        .iter()
        .find_map(|keyword| header.strip_prefix(keyword))
    {
        rest.to_string()
    } else {
        return false;
    };
    // Generic parameters of an `impl<T>` come before the type; skip them.
    let typed = typed.trim();
    let typed = if typed.starts_with('<') {
        skip_generics(typed)
    } else {
        typed
    };
    // `Owner<T>` and `crate::Owner` name `Owner` too.
    let path: String = typed
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
        .collect();
    path.rsplit("::").next() == Some(owner)
}

/// `text` after a leading balanced `<…>`.
fn skip_generics(text: &str) -> &str {
    let mut depth = 0usize;
    for (index, c) in text.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return &text[index + 1..];
                }
            }
            _ => {}
        }
    }
    ""
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "use super::*;

#[path = \"liq_cancel.rs\"]
mod cancel;

impl Exec {
    pub fn sweep(&self) {
        let cancel = 1;
    }
}

impl<T: Clone> Element for Exec<T> {
    fn paint(&self) {}
}

pub struct Exec {
    pub cancel: bool,
}

impl Other {
    fn cancel(&self) {}
}
";

    /// #2045: the `mod cancel;` beside `impl Exec` is not inside it, nor is a local of its
    /// method; its methods and the struct's fields are, and another type's method is not.
    #[test]
    fn a_line_is_inside_the_blocks_of_its_type_only() {
        let at = |needle: &str| SOURCE.lines().position(|l| l.contains(needle)).unwrap();
        assert!(!inside_impl_of(SOURCE, at("mod cancel"), "Exec"));
        assert!(!inside_impl_of(SOURCE, at("let cancel"), "Exec"));
        assert!(inside_impl_of(SOURCE, at("fn sweep"), "Exec"));
        assert!(inside_impl_of(SOURCE, at("fn paint"), "Exec"));
        assert!(inside_impl_of(SOURCE, at("pub cancel"), "Exec"));
        assert!(!inside_impl_of(SOURCE, at("fn cancel"), "Exec"));
        assert!(inside_impl_of(SOURCE, at("fn cancel"), "Other"));
    }
}
