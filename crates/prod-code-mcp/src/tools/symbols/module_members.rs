/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Who owns a member: a module is a member of a module, never of a type (#2045), and an `impl`
//! block's label names its type (#2053).

use std::path::Path;

use super::SymbolHit;
use super::matching::module_path_ends_with;

/// The candidates of a qualified lookup that the qualifiers can name. Every item stays; a module
/// stays only when the qualifiers are its parent modules, whether the index lists it at its
/// `mod name;` line or at its own file. `Exec::cancel` is not the `#[path] mod cancel;` of a
/// file that also holds an `impl Exec`, which `code_definition` answered with that module's
/// first line.
pub(crate) fn modules_of_qualifiers(
    root: &Path,
    candidates: Vec<SymbolHit>,
    qualifiers: &[&str],
    name: &str,
) -> Vec<SymbolHit> {
    let own: Vec<&str> = qualifiers.iter().copied().chain([name]).collect();
    candidates
        .into_iter()
        .filter(|hit| {
            !matches!(hit.kind, "Module" | "Namespace" | "Package")
                || module_path_ends_with(root, &hit.path, qualifiers)
                || module_path_ends_with(root, &hit.path, &own)
        })
        .collect()
}

/// `impl Trait for Type::name` and `impl Type::name` as `Type::name`: the label rust-analyzer
/// gives an `impl` block names the owner of its items, and the closest-names list of a failed
/// lookup offers members spelled that way (#2053). Anything else is returned as it was.
pub(crate) fn impl_owner_spelled_as_type(symbol: &str) -> String {
    let segments: Vec<&str> = symbol.split("::").collect();
    let Some((name, owners)) = segments.split_last() else {
        return symbol.to_string();
    };
    let Some(label) = owners.last().map(|owner| owner.trim()) else {
        return symbol.to_string();
    };
    let Some(rest) = label.strip_prefix("impl ") else {
        return symbol.to_string();
    };
    let owner = rest.rsplit(" for ").next().unwrap_or(rest).trim();
    if owner.is_empty() || owner.contains(char::is_whitespace) {
        return symbol.to_string();
    }
    let mut path: Vec<&str> = owners[..owners.len() - 1].to_vec();
    path.push(owner);
    path.push(name);
    path.join("::")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #2053: the suggested `impl Element for TextElement::paint` asks for `TextElement::paint`.
    #[test]
    fn an_impl_label_names_its_type() {
        for (asked, meant) in [
            ("impl Element for TextElement::paint", "TextElement::paint"),
            ("impl Render for TextInput::render", "TextInput::render"),
            ("impl TextElement::new", "TextElement::new"),
            (
                "ui::impl Element for TextElement::paint",
                "ui::TextElement::paint",
            ),
            ("TextElement::paint", "TextElement::paint"),
            ("paint", "paint"),
            ("implement::paint", "implement::paint"),
        ] {
            assert_eq!(impl_owner_spelled_as_type(asked), meant, "{asked}");
        }
    }

    fn hit(root: &Path, path: &str, kind: &'static str, line: u32) -> SymbolHit {
        SymbolHit {
            path: root.join(path),
            name: "cancel".to_string(),
            kind,
            container: None,
            line,
            col: 5,
        }
    }

    fn kept(root: &Path, hits: Vec<SymbolHit>, qualifiers: &[&str]) -> Vec<(String, u32)> {
        modules_of_qualifiers(root, hits, qualifiers, "cancel")
            .into_iter()
            .map(|hit| (hit.kind.to_string(), hit.line))
            .collect()
    }

    /// #2045: `Exec::cancel` named the `mod cancel;` of `src/liq.rs`.
    #[test]
    fn a_module_is_not_the_member_of_a_type() {
        let root = Path::new("/ws");
        let module = || hit(root, "src/liq.rs", "Module", 4);
        assert!(kept(root, vec![module()], &["Exec"]).is_empty());
        let method = hit(root, "src/liq_cancel.rs", "Method", 5);
        assert_eq!(
            kept(root, vec![module(), method], &["Exec"]),
            [("Method".to_string(), 5)]
        );
    }

    /// A module asked for by its parent module is still found, at its declaration or its file.
    #[test]
    fn a_module_of_the_named_module_is_kept() {
        let root = Path::new("/ws");
        let declared = hit(root, "src/liq.rs", "Module", 4);
        assert_eq!(
            kept(root, vec![declared], &["liq"]),
            [("Module".to_string(), 4)]
        );
        let file = hit(root, "src/liq/cancel.rs", "Module", 1);
        assert_eq!(
            kept(root, vec![file], &["liq"]),
            [("Module".to_string(), 1)]
        );
    }
}
