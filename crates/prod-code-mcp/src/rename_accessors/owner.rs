/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Which struct owns a field, and which `impl` blocks are that struct's: by the analyzer's
//! definition of the `impl`'s self type, not by a name that another module can share (#1383).

use super::is_ident;

/// The struct whose braces enclose a field: its name and the byte offset of that name.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Owner {
    pub name: String,
    pub name_at: usize,
}

/// The `{` that opens the block enclosing `at`, and the start of that block's item header: just
/// after the previous item's `}` or `;`.
fn enclosing_block(text: &str, at: usize) -> Option<(usize, usize)> {
    let mut depth = 0i32;
    let open = text[..at].char_indices().rev().find_map(|(i, c)| {
        match c {
            '}' => depth += 1,
            '{' if depth == 0 => return Some(i),
            '{' => depth -= 1,
            _ => {}
        }
        None
    })?;
    let item_start = text[..open].rfind(['}', ';']).map_or(0, |i| i + 1);
    Some((open, item_start))
}

/// Where the `where` of a declaration header starts, on whatever line it stands.
fn where_at(head: &str) -> Option<usize> {
    head.match_indices("where").map(|(i, _)| i).find(|&i| {
        let before = head[..i].chars().next_back();
        let after = head[i + 5..].chars().next();
        before.is_some_and(|c| c.is_whitespace() || c == '>' || c == ')')
            && after.is_none_or(char::is_whitespace)
    })
}

/// The struct whose braces enclose `at`.
pub(super) fn enclosing_struct(text: &str, at: usize) -> Option<Owner> {
    let (open, item_start) = enclosing_block(text, at)?;
    let head = &text[item_start..open];
    let head = &head[..where_at(head).unwrap_or(head.len())];
    let head = head.trim_end();
    let before_generics = match head.strip_suffix('>') {
        Some(_) => &head[..head.find('<')?],
        None => head,
    }
    .trim_end();
    let name_start = before_generics
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map(|(i, _)| i)?;
    before_generics[..name_start]
        .trim_end()
        .ends_with("struct")
        .then(|| Owner {
            name: before_generics[name_start..].to_string(),
            name_at: item_start + name_start,
        })
}

/// An identifier byte; every byte of a non-ASCII character counts, so a word never ends inside
/// one.
fn ident_byte(b: u8) -> bool {
    b >= 0x80 || b.is_ascii_alphanumeric() || b == b'_'
}

/// The byte offset of the self type's name of the `impl` block enclosing `at`: `Conn` in
/// `impl Conn`, `impl<T> a::Conn<T>` and `impl Display for Conn`. `None` outside an `impl`.
pub(super) fn impl_self_type(text: &str, at: usize) -> Option<usize> {
    let (open, item_start) = enclosing_block(text, at)?;
    let raw = &text[item_start..open];
    let head = &raw[..where_at(raw).unwrap_or(raw.len())];
    let lead = head.len() - head.trim_start().len();
    let after_impl = head[lead..].strip_prefix("impl")?;
    if after_impl.starts_with(is_ident) {
        return None;
    }
    let mut i = item_start + lead + 4;
    let end = item_start + head.len();
    let bytes = text.as_bytes();
    // The impl's own generics, then the trait and `for` when there are any.
    let mut depth = 0i32;
    let mut self_from = None;
    let mut first = true;
    while i < end {
        let c = bytes[i];
        match c {
            b'<' => depth += 1,
            b'>' if i > 0 && bytes[i - 1] == b'-' => {}
            b'>' => depth -= 1,
            _ if depth == 0 && ident_byte(c) && !c.is_ascii_digit() => {
                let word_end = (i..end).find(|&j| !ident_byte(bytes[j])).unwrap_or(end);
                if &text[i..word_end] == "for" {
                    self_from = Some(word_end);
                }
                if first {
                    self_from.get_or_insert(i);
                    first = false;
                }
                i = word_end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    // The last path segment of the self type, before its own generics.
    let from = self_from?;
    let mut name_at = None;
    let mut j = from;
    while j < end {
        let c = bytes[j];
        if c == b'<' {
            break;
        }
        if ident_byte(c) && (j == 0 || !ident_byte(bytes[j - 1])) {
            let word_end = (j..end).find(|&k| !ident_byte(bytes[k])).unwrap_or(end);
            if !matches!(
                &text[j..word_end],
                "dyn" | "mut" | "crate" | "self" | "super"
            ) {
                name_at = Some(j);
            }
            j = word_end;
            continue;
        }
        j += 1;
    }
    name_at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_s_struct_is_found_however_its_header_is_laid_out() {
        let t =
            "pub struct Conn<T> where T: Clone {\n    timeout: u64,\n}\nfn f() { let x = 1; }\n";
        let owner = enclosing_struct(t, t.find("timeout").unwrap()).unwrap();
        assert_eq!(
            (owner.name.as_str(), owner.name_at),
            ("Conn", t.find("Conn").unwrap())
        );
        assert!(enclosing_struct(t, t.find("x =").unwrap()).is_none());
        // A `where` on a line of its own (#1383).
        let t = "struct Conn<T>\nwhere\n    T: Clone,\n{\n    timeout: T,\n}\n";
        assert_eq!(
            enclosing_struct(t, t.find("timeout").unwrap())
                .unwrap()
                .name,
            "Conn"
        );
        let t = "struct Wide<\n    T,\n>\nwhere\n    T: Clone,\n{\n    timeout: T,\n}\n";
        assert_eq!(
            enclosing_struct(t, t.find("timeout").unwrap())
                .unwrap()
                .name,
            "Wide"
        );
        // A `where` inside a name is no clause.
        let t = "struct Somewhere {\n    timeout: u8,\n}\n";
        assert_eq!(
            enclosing_struct(t, t.find("timeout").unwrap())
                .unwrap()
                .name,
            "Somewhere"
        );
    }

    #[test]
    fn the_self_type_of_an_impl_is_found_after_generics_and_traits() {
        let name = |t: &str, inside: &str| {
            impl_self_type(t, t.find(inside).unwrap()).map(|at| {
                t[at..]
                    .chars()
                    .take_while(|c| is_ident(*c))
                    .collect::<String>()
            })
        };
        let t = "impl Conn {\n    fn timeout(&self) {}\n}\n";
        assert_eq!(name(t, "fn timeout").as_deref(), Some("Conn"));
        let t = "impl<T: Into<u8>> a::b::Conn<T>\nwhere\n    T: Clone,\n{\n    fn timeout(&self) {}\n}\n";
        assert_eq!(name(t, "fn timeout").as_deref(), Some("Conn"));
        let t =
            "impl<F: Fn() -> u8> std::fmt::Display for crate::Conn<F> {\n    fn fmt(&self) {}\n}\n";
        assert_eq!(name(t, "fn fmt").as_deref(), Some("Conn"));
        let t = "impl Trait for dyn Other {\n    fn go(&self) {}\n}\n";
        assert_eq!(name(t, "fn go").as_deref(), Some("Other"));
        let t = "mod m {\n    fn timeout() {}\n}\n";
        assert!(impl_self_type(t, t.find("fn timeout").unwrap()).is_none());
        let t = "implement {\n    fn x() {}\n}\n";
        assert!(impl_self_type(t, t.find("fn x").unwrap()).is_none());
    }
}
