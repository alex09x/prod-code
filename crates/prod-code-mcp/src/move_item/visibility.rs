/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use ra_ap_syntax::{AstNode, Edition, NodeOrToken, ast, ast::HasName, ast::HasVisibility};

use super::rust_syntax;

pub(super) fn widen_private_function(item: &str, name: &str, edition: Edition) -> Result<String> {
    let tree = rust_syntax::parse(item, edition);
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
    use ra_ap_syntax::Edition;

    use super::widen_private_function;

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
