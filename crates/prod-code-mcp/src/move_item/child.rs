/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::imports::{add_import, carry_imports, has_use_super_glob};
use crate::fixture::builder::lexer::Source;
use anyhow::Result;

/// Rebase paths one module deeper without changing comments, strings, raw identifiers or
/// later components of a `super::super::...` chain. The lexer also handles spaced paths.
fn rebase(text: &str) -> Result<String> {
    let source = Source::new(text)?;
    let mut out = text.to_string();
    for i in (0..source.tokens.len()).rev() {
        if !(source.is(i, "super") || source.is(i, "self"))
            || !source.is(i + 1, "::")
            || (i > 0 && source.is(i - 1, "::"))
        {
            continue;
        }
        let token = source.tokens[i];
        let replacement = if source.is(i, "super") {
            "super::super"
        } else {
            "super"
        };
        out.replace_range(token.start..token.end, replacement);
    }
    Ok(out)
}

/// Integration tests retain their original scope, including private helpers. Ordinary moves
/// carry only their explicit imports, rebased separately from the target's existing text.
pub(super) fn move_to_child(
    source: &str,
    item: &str,
    target: &str,
    inherit_parent: bool,
) -> Result<(String, String, Vec<String>)> {
    let mut item = rebase(item)?;
    let parsed = Source::new(&item)?;
    let mut i = 0;
    while parsed.is(i, "#") && parsed.is(i + 1, "[") {
        i = parsed
            .close_of(i + 1)
            .map_or(parsed.tokens.len(), |close| close + 1);
    }
    // `impl` blocks and `macro_rules!` take no visibility.
    let takes_visibility = !(parsed.is(i, "impl")
        || parsed.is(i, "macro_rules")
        || (parsed.is(i, "unsafe") && parsed.is(i + 1, "impl")));
    if let Some(token) = parsed.tokens.get(i)
        && takes_visibility
    {
        let at = token.start;
        if !parsed.is(i, "pub") {
            item.insert_str(at, "pub(super) ");
        } else if parsed.is(i + 1, "(") && parsed.is(i + 3, ")") {
            if parsed.is(i + 2, "self") {
                item.replace_range(
                    parsed.tokens[i + 2].start..parsed.tokens[i + 2].end,
                    "super",
                );
            } else if parsed.is(i + 2, "super") {
                item.replace_range(
                    parsed.tokens[i + 2].start..parsed.tokens[i + 2].end,
                    "in super::super",
                );
            }
        }
    }
    let (mut text, mut carried) = if has_use_super_glob(target) {
        (target.to_string(), Vec::new())
    } else {
        carry_imports(&rebase(source)?, &item, target)
    };
    if inherit_parent && !has_use_super_glob(&text) {
        text = add_import(&text, "use super::*;");
        carried.push("added `use super::*;` to retain the parent scope".to_string());
    }
    Ok((text, item, carried))
}

#[cfg(test)]
mod tests {
    use super::move_to_child;

    #[test]
    fn child_move_widens_items_but_not_impls_or_macros() {
        let widened = |item: &str| move_to_child("", item, "", false).unwrap().1;
        assert_eq!(widened("fn f() {}"), "pub(super) fn f() {}");
        assert_eq!(widened("#[inline]\nfn f() {}"), "#[inline]\npub(super) fn f() {}");
        assert_eq!(widened("pub(super) fn f() {}"), "pub(in super::super) fn f() {}");
        assert_eq!(widened("pub(self) fn f() {}"), "pub(super) fn f() {}");
        for item in [
            "impl Foo { fn f() {} }",
            "unsafe impl Send for Foo {}",
            "macro_rules! m { () => {}; }",
        ] {
            assert_eq!(widened(item), item);
        }
    }
}
