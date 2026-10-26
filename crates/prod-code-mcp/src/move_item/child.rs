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
/// later components of a `super::super::...` chain. Restrictions use their own grammar.
fn rebase(text: &str, edition: ra_ap_syntax::Edition) -> Result<String> {
    use ra_ap_syntax::{AstNode, ast};
    let source = super::rust_syntax::parse(text, edition);
    let mut out = text.to_string();
    let mut edits = Vec::new();
    for path in source.syntax().descendants().filter_map(ast::Path::cast) {
        if path.qualifier().is_some() {
            continue;
        }
        let Some(segment) = path.segment() else {
            continue;
        };
        let head = segment.syntax().text().to_string();
        let nested = path
            .syntax()
            .ancestors()
            .filter_map(ast::Module::cast)
            .filter(|m| {
                m.item_list().is_some_and(|items| {
                    super::rust_syntax::range(items.syntax())
                        .contains(&super::rust_syntax::range(segment.syntax()).start)
                })
            })
            .count();
        let mut chain = path.clone();
        let mut supers = usize::from(head == "super");
        while let Some(parent) = chain.syntax().parent().and_then(ast::Path::cast) {
            if parent
                .segment()
                .is_some_and(|s| s.syntax().text().to_string() == "super")
            {
                supers += 1;
                chain = parent;
            } else {
                break;
            }
        }
        if head == "self" && super::rust_syntax::grouped_self(&path) {
            continue;
        }
        // The selected item does not include its old surrounding module. A chain that
        // reaches that boundary (including from a local inline module) must grow too.
        if (head == "super" && supers >= nested) || (head == "self" && nested == 0) {
            let visibility = super::rust_syntax::in_visibility(&path);
            let has_in = path
                .syntax()
                .ancestors()
                .find_map(ast::Visibility::cast)
                .is_some_and(|v| {
                    v.syntax()
                        .descendants_with_tokens()
                        .filter_map(|n| n.into_token())
                        .any(|t| t.text() == "in")
                });
            edits.push((
                super::rust_syntax::range(segment.syntax()),
                if head == "super" {
                    if visibility && !has_in {
                        "in super::super"
                    } else {
                        "super::super"
                    }
                } else {
                    "super"
                },
            ));
        }
    }
    for (span, replacement) in edits.into_iter().rev() {
        out.replace_range(span, replacement);
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
    edition: ra_ap_syntax::Edition,
) -> Result<(String, String, Vec<String>)> {
    let mut item = rebase(item, edition)?;
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
        }
    }
    let (mut text, mut carried) = if has_use_super_glob(target) {
        (target.to_string(), Vec::new())
    } else {
        carry_imports(&rebase(source, edition)?, &item, target)
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
        let widened = |item: &str| {
            move_to_child("", item, "", false, ra_ap_syntax::Edition::CURRENT)
                .unwrap()
                .1
        };
        assert_eq!(widened("fn f() {}"), "pub(super) fn f() {}");
        assert_eq!(
            widened("#[inline]\nfn f() {}"),
            "#[inline]\npub(super) fn f() {}"
        );
        assert_eq!(
            widened("pub(super) fn f() {}"),
            "pub(in super::super) fn f() {}"
        );
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
