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
        // An item that ended above (`impl Exec {}`, `mod cancel;`) is not this block's header.
        if at != open && (row.ends_with('}') || row.ends_with(';')) {
            return false;
        }
        if is_block_header(row) {
            // The header may continue on the lines below it (`impl Trait for` / `    Exec {`).
            let header: Vec<&str> = code[at..].iter().map(|(_, row)| *row).collect();
            return names_owner(&header.join(" "), owner);
        }
        // Only a `where` clause or the lone `{` of a type's header is read past; any other line
        // starts a block of its own (`mod nested {`, `pub fn helper<T>(x: T)` / `where …` /
        // `{`), which holds the line instead.
        if !(row.starts_with('{') || row.starts_with("where")) {
            return false;
        }
    }
    false
}

/// `row` without the visibility and `unsafe` before its keyword.
fn without_modifiers(row: &str) -> &str {
    row.trim_start_matches("pub(crate) ")
        .trim_start_matches("pub(super) ")
        .trim_start_matches("pub ")
        .trim_start_matches("unsafe ")
}

/// Whether a line starts a type's block: `impl`, `struct`, `enum`, `union` or `trait`.
fn is_block_header(row: &str) -> bool {
    let row = without_modifiers(row);
    row.starts_with("impl ")
        || row.starts_with("impl<")
        || ["struct ", "enum ", "union ", "trait "]
            .iter()
            .any(|keyword| row.starts_with(keyword))
}

/// Whether `text` has an `impl` block for `owner`, in any spelling: `impl Owner`,
/// `impl<T> Owner<T>`, `impl Trait for Owner`, `unsafe impl`, a header over several lines.
pub(crate) fn has_impl_of(text: &str, owner: &str) -> bool {
    let rows: Vec<&str> = text.lines().map(str::trim_start).collect();
    rows.iter().enumerate().any(|(at, row)| {
        let row = without_modifiers(row);
        if !(row.starts_with("impl ") || row.starts_with("impl<")) {
            return false;
        }
        // Up to the line that opens the block, a few lines at most.
        let end = rows[at..]
            .iter()
            .take(8)
            .position(|row| row.contains('{') || row.ends_with(';'))
            .map_or(rows.len(), |i| at + i + 1);
        names_owner(&rows[at..end.min(rows.len())].join(" "), owner)
    })
}

/// Whether a block header names `owner` as its type.
fn names_owner(header: &str, owner: &str) -> bool {
    let header = without_modifiers(header);
    let typed = if let Some(rest) = header.strip_prefix("impl") {
        if !rest.starts_with([' ', '<']) {
            return false;
        }
        // Generic parameters of an `impl<T>` come first; a `for` in their bounds
        // (`impl<T: for<'a> Fn(&'a str)>`) is not the trait's.
        let rest = rest.trim_start();
        let rest = if rest.starts_with('<') {
            skip_generics(rest)
        } else {
            rest
        };
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
    let typed = typed.trim();
    // `Owner<T>` and `crate::Owner` name `Owner` too.
    let path: String = typed
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
        .collect();
    path.rsplit("::").next() == Some(owner)
}

/// `text` after a leading balanced `<…>`; the `>` of a `->` in a bound closes nothing.
fn skip_generics(text: &str) -> &str {
    let mut depth = 0usize;
    let mut previous = ' ';
    for (index, c) in text.char_indices() {
        let arrow = previous == '-';
        previous = c;
        match c {
            '<' => depth += 1,
            '>' if arrow => {}
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

    /// A block that opens on the line above (`mod nested {`) owns its items, not the closed
    /// `impl Exec {}` before it; `unsafe impl`, a header split after `for`, a `->` and a `for`
    /// in the generic bounds all still name their type.
    #[test]
    fn the_nearest_block_owns_a_line_whatever_its_header_spells() {
        let source = "impl Exec {}
mod nested {
    fn cancel() {}
}
impl Exec {}
pub fn helper<T>(x: T) -> bool
where
    T: Clone,
{
    fn spawn() {}
}
impl Exec {}
mod inner
{
    fn close() {}
}
unsafe impl Send for Queue {}
impl<T: Fn() -> bool> Filter<T> {
    fn keep(&self) {}
}
impl<T: for<'a> Fn(&'a str)> Parser<T> {
    fn parse(&self) {}
}
impl fmt::Debug for
    Split
{
    fn fmt(&self) {}
}
";
        let at = |needle: &str| source.lines().position(|l| l.contains(needle)).unwrap();
        assert!(!inside_impl_of(source, at("fn cancel"), "Exec"));
        assert!(!inside_impl_of(source, at("fn spawn"), "Exec"));
        assert!(!inside_impl_of(source, at("fn close"), "Exec"));
        assert!(has_impl_of(source, "Queue"));
        assert!(has_impl_of(source, "Filter"));
        assert!(inside_impl_of(source, at("fn keep"), "Filter"));
        assert!(!inside_impl_of(source, at("fn keep"), "bool"));
        assert!(inside_impl_of(source, at("fn parse"), "Parser"));
        assert!(has_impl_of(source, "Split"));
        assert!(inside_impl_of(source, at("fn fmt"), "Split"));
    }
}
