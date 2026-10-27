/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Swift names a module may give its own types. A literal passed as the standard `Int` runs no
//! code of the program's; passed as a module's own `Int`, it runs that type's
//! `init(integerLiteral:)`, which may do anything. So a bare standard name is taken for the
//! standard type only when no Swift file of the checkout declares a type of that name (#1512).

use std::collections::BTreeSet;
use std::path::Path;

use super::effects::SWIFT_LITERAL_TYPES;
use super::types::Param;

/// How many Swift files are read before the checkout counts as declaring every name.
const SCAN_LIMIT: usize = 5000;

/// The standard literal type names a Swift file under `root` declares a type of. When the
/// checkout is too large to read, every name counts as declared: none is assumed standard.
pub(crate) fn shadowed_literal_types(root: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut read = 0usize;
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "swift") || !path.is_file() {
            continue;
        }
        read += 1;
        if read > SCAN_LIMIT {
            return SWIFT_LITERAL_TYPES.iter().map(|n| n.to_string()).collect();
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        found.extend(
            SWIFT_LITERAL_TYPES
                .iter()
                .filter(|name| declares(&text, name))
                .map(|name| name.to_string()),
        );
    }
    found
}

/// Whether `text` declares a type `name`: `struct`, `class`, `enum`, `actor` or `typealias`
/// followed by the name as a whole word.
pub(crate) fn declares(text: &str, name: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    ["struct", "class", "enum", "actor", "typealias"]
        .iter()
        .any(|keyword| {
            text.match_indices(keyword).any(|(at, _)| {
                let before = text[..at].chars().next_back();
                let rest = &text[at + keyword.len()..];
                let after_keyword = rest.chars().next();
                let rest = rest.trim_start();
                !before.is_some_and(ident)
                    && after_keyword.is_some_and(char::is_whitespace)
                    && rest.starts_with(name)
                    && !rest[name.len()..].chars().next().is_some_and(ident)
            })
        })
}

/// `params` with a bare type the checkout shadows named as the module's own, so that a literal
/// passed as it is not taken for inert.
pub(crate) fn with_shadowed(params: &[Param], shadowed: &BTreeSet<String>) -> Vec<Param> {
    params
        .iter()
        .map(|p| {
            let mut p = p.clone();
            if let Some(ty) = &p.ty {
                let base = ty.trim().trim_end_matches(['?', '!']);
                if shadowed.contains(base) {
                    p.ty = Some(format!("ThisModule.{}", ty.trim()));
                }
            }
            p
        })
        .collect()
}
