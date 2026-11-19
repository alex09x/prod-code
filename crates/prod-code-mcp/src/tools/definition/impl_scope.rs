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
    // method, not a member of the `impl` around it. Its header may span lines
    // (`impl<T> Exec<T>` / `where T: Clone` / `{`): the lines at the opening line's indentation
    // above it are read up to the header, and an item ending there (`}` or `;`) ends the search.
    let code: Vec<(usize, &str)> = lines[..line]
        .iter()
        .map(|row| (indent(row), row.trim_start()))
        .filter(|(_, trimmed)| {
            !trimmed.is_empty() && !trimmed.starts_with("//") && !trimmed.starts_with('#')
        })
        .collect();
    let Some(open) = code.iter().rposition(|(level, _)| *level < depth) else {
        return false;
    };
    let level = code[open].0;
    for (at, (row_level, row)) in code[..=open].iter().enumerate().rev() {
        // A deeper line is a continuation of the header (`    T: Clone,`); a shallower one
        // is outside the block.
        if *row_level > level {
            continue;
        }
        if *row_level < level {
            return false;
        }
        if is_block_header(row) {
            return names_owner(row, owner);
        }
        if at != open && (row.ends_with('}') || row.ends_with(';')) {
            return false;
        }
    }
    false
}

/// Whether a line starts a type's block: `impl`, `struct`, `enum`, `union` or `trait`.
fn is_block_header(row: &str) -> bool {
    let row = row
        .trim_start_matches("pub(crate) ")
        .trim_start_matches("pub(super) ")
        .trim_start_matches("pub ")
        .trim_start_matches("unsafe ");
    row.starts_with("impl ")
        || row.starts_with("impl<")
        || ["struct ", "enum ", "union ", "trait "]
            .iter()
            .any(|keyword| row.starts_with(keyword))
}

/// Whether `text` has an `impl` block for `owner`, in any spelling: `impl Owner`,
/// `impl<T> Owner<T>`, `impl Trait for Owner`.
pub(crate) fn has_impl_of(text: &str, owner: &str) -> bool {
    text.lines().any(|row| {
        let row = row.trim_start();
        (row.starts_with("impl ") || row.starts_with("impl<")) && names_owner(row, owner)
    })
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

impl<T> fmt::Display for Wrapped<T>
where
    T: Clone,
{
    fn fmt(&self) {}
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

    /// A header over several lines, with a `where` clause, still owns its items.
    #[test]
    fn a_multi_line_header_owns_its_items() {
        let at = |needle: &str| SOURCE.lines().position(|l| l.contains(needle)).unwrap();
        assert!(inside_impl_of(SOURCE, at("fn fmt"), "Wrapped"));
        assert!(!inside_impl_of(SOURCE, at("fn fmt"), "Other"));
        assert!(has_impl_of(SOURCE, "Wrapped"));
        assert!(has_impl_of(SOURCE, "Exec"));
        assert!(!has_impl_of(SOURCE, "Missing"));
    }
}
