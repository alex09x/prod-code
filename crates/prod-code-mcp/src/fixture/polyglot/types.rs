/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::mock::MethodSignature;

/// The shape of a type across languages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolyglotShape {
    /// A struct, interface, class or object with named fields: `(name, type)`.
    Record(Vec<(String, String)>),
    /// A tuple struct or positional sequence of types.
    Tuple(Vec<String>),
    /// A unit struct or void type.
    Unit,
    /// An enum and its variant names.
    Enum(Vec<String>),
    /// An interface, trait, or protocol with method signatures.
    Interface { methods: Vec<MethodSignature> },
    /// An interface with both named properties and method signatures.
    InterfaceWithFields {
        methods: Vec<MethodSignature>,
        fields: Vec<(String, String)>,
    },
}

pub fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

pub fn lower_camel_case(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub(crate) fn strip_comments(line: &str) -> &str {
    let mut line = line.trim();
    if let Some(pos) = line.find("//") {
        line = line[..pos].trim();
    }
    if let Some(pos) = line.find('#') {
        line = line[..pos].trim();
    }
    line
}

pub(crate) fn strip_go_tags(line: &str) -> &str {
    if let Some(pos) = line.find('`') {
        line[..pos].trim()
    } else {
        line
    }
}

pub(crate) fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 1;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn split_comma_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' | '[' | '<' | '{' => {
                depth += 1;
                current.push(c);
            }
            ')' | ']' | '>' | '}' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                out.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}
