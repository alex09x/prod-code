/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use std::collections::BTreeSet;

use super::specifiers::is_symbol_used;

pub(super) fn missing_module_dependencies(
    source: &str,
    moved_item: &str,
    target: &str,
    moved_name: &str,
) -> Vec<String> {
    let source_names = top_level_bindings(source);
    let target_names = top_level_bindings(target);
    source_names
        .difference(&target_names)
        .filter(|name| name.as_str() != moved_name && is_symbol_used(moved_item, name))
        .cloned()
        .collect()
}

fn top_level_bindings(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in source
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
    {
        let line = line.trim();
        let declaration = line
            .strip_prefix("async def ")
            .or_else(|| line.strip_prefix("def "))
            .or_else(|| line.strip_prefix("class "));
        if let Some(declaration) = declaration {
            let name = declaration
                .split(['(', ':', ' '])
                .next()
                .unwrap_or_default();
            if valid_identifier(name) {
                names.insert(name.to_string());
            }
            continue;
        }
        if let Some(imported) = line
            .strip_prefix("from ")
            .and_then(|line| line.split_once(" import ").map(|(_, names)| names))
        {
            for name in imported.split(',') {
                let name = name.trim().split(" as ").last().unwrap_or_default();
                if valid_identifier(name) {
                    names.insert(name.to_string());
                }
            }
            continue;
        }
        if let Some(imported) = line.strip_prefix("import ") {
            for name in imported.split(',') {
                let name = name.trim().split(" as ").last().unwrap_or_default();
                let binding = name.split('.').next().unwrap_or_default();
                if valid_identifier(binding) {
                    names.insert(binding.to_string());
                }
            }
            continue;
        }
        if let Some((left, _)) = line.split_once('=') {
            let name = left.split(':').next().unwrap_or_default().trim();
            if valid_identifier(name) {
                names.insert(name.to_string());
            }
        }
    }
    names
}

fn valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

#[cfg(test)]
#[path = "python_dependencies_tests.rs"]
mod tests;
