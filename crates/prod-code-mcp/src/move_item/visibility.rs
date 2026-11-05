/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A private item moved into a sibling module is still private there, so the module it came
//! from, which still names it, can no longer reach it (E0432, E0425). It becomes `pub(crate)`,
//! the narrowest visibility that reaches a sibling (#1933, #1893).

use anyhow::{Context, Result};
use ra_ap_syntax::{
    AstNode, Edition, NodeOrToken, SyntaxKind, ast, ast::HasName, ast::HasVisibility,
};

use super::rust_syntax::{parse, range};

/// `item` with `pub(crate) ` in front of its first keyword (after attributes and doc comments),
/// or `None` when it has a visibility already or takes none (`impl`, `macro_rules!`).
pub(crate) fn with_crate_visibility(item: &str, edition: Edition) -> Option<String> {
    let file = parse(item, edition);
    let node = file
        .syntax()
        .children()
        .find(|n| ast::Item::can_cast(n.kind()))?;
    if ast::Impl::can_cast(node.kind()) || ast::MacroRules::can_cast(node.kind()) {
        return None;
    }
    if node.children().find_map(ast::Visibility::cast).is_some() {
        return None;
    }
    // Attributes, whitespace and comments (doc comments included, whether the parser gives them
    // as tokens or as nodes) come before the first keyword.
    let leading = |text: &str| {
        let text = text.trim_start();
        text.is_empty() || text.starts_with("//") || text.starts_with("/*")
    };
    let first = node.children_with_tokens().find(|element| match element {
        NodeOrToken::Node(n) => n.kind() != SyntaxKind::ATTR && !leading(&n.text().to_string()),
        NodeOrToken::Token(t) => !leading(t.text()),
    })?;
    let at = match &first {
        NodeOrToken::Node(n) => range(n).start,
        NodeOrToken::Token(t) => usize::from(t.text_range().start()),
    };
    let mut widened = item.to_string();
    widened.insert_str(at, "pub(crate) ");
    Some(widened)
}

/// The function `name` in `item` with `pub(crate) ` in front of its declaration, when it has no
/// visibility; `item` unchanged when it has one or holds no such function.
pub(super) fn widen_private_function(item: &str, name: &str, edition: Edition) -> Result<String> {
    let tree = parse(item, edition);
    let Some(function) = tree
        .syntax()
        .descendants()
        .filter_map(ast::Fn::cast)
        .find(|function| {
            function
                .name()
                .is_some_and(|item_name| item_name.text() == name)
        })
    else {
        return Ok(item.to_string());
    };
    if function.visibility().is_some() {
        return Ok(item.to_string());
    }
    let token = function
        .syntax()
        .children_with_tokens()
        .find_map(|element| match element {
            NodeOrToken::Token(token) => Some(token),
            NodeOrToken::Node(_) => None,
        })
        .context("private function has no declaration token")?;
    let leading_whitespace = token.text().len() - token.text().trim_start().len();
    let insertion = usize::from(token.text_range().start()) + leading_whitespace;
    let mut widened = item.to_string();
    widened.insert_str(insertion, "pub(crate) ");
    Ok(widened)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_private_item_gets_pub_crate_after_its_attributes_and_docs() {
        let edition = Edition::Edition2021;
        assert_eq!(
            with_crate_visibility("fn helper() {}\n", edition).as_deref(),
            Some("pub(crate) fn helper() {}\n")
        );
        assert_eq!(
            with_crate_visibility("/// Docs.\n#[inline]\nasync fn helper() {}\n", edition)
                .as_deref(),
            Some("/// Docs.\n#[inline]\npub(crate) async fn helper() {}\n")
        );
        assert_eq!(
            with_crate_visibility("struct Shape;\n", edition).as_deref(),
            Some("pub(crate) struct Shape;\n")
        );
        for already in [
            "pub fn helper() {}\n",
            "pub(super) fn helper() {}\n",
            "impl Shape {}\n",
            "macro_rules! m { () => {} }\n",
        ] {
            assert_eq!(with_crate_visibility(already, edition), None, "{already}");
        }
    }

    #[test]
    fn private_function_visibility_is_widened_after_attributes() {
        let moved = "#[inline]\nasync fn helper() {}\n";
        assert_eq!(
            widen_private_function(moved, "helper", Edition::CURRENT).unwrap(),
            "#[inline]\npub(crate) async fn helper() {}\n"
        );
    }

    #[test]
    fn existing_visibility_and_non_functions_are_preserved() {
        let public = "pub(super) fn helper() {}\n";
        assert_eq!(
            widen_private_function(public, "helper", Edition::CURRENT).unwrap(),
            public
        );
        let structure = "struct Helper;\n";
        assert_eq!(
            widen_private_function(structure, "Helper", Edition::CURRENT).unwrap(),
            structure
        );
    }
}
