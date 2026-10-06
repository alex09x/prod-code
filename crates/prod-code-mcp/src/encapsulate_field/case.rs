/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Helper to convert a string to PascalCase.
pub fn to_pascal_case(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize = true;
    for c in s.chars() {
        if c == '_' {
            capitalize = true;
        } else if capitalize {
            result.extend(c.to_uppercase());
            capitalize = false;
        } else {
            result.push(c);
        }
    }
    if result.is_empty() {
        s.to_string()
    } else {
        result
    }
}

/// Helper to convert a string to snake_case.
pub fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.char_indices() {
        if c.is_uppercase() {
            if i > 0 && !result.ends_with('_') {
                result.push('_');
            }
            result.extend(c.to_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

/// Helper to lowercase the first character of a string.
pub fn lowercase_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_lowercase().chain(chars).collect(),
    }
}

/// Extract the field name at a 1-based line and column in text.
pub fn field_at_line_col(text: &str, line: u32, col: u32) -> Option<String> {
    if line == 0 {
        return None;
    }
    let target_line = text.lines().nth((line - 1) as usize)?;
    let target_units = col.saturating_sub(1);
    let mut units = 0u32;
    let mut col_idx = None;
    for (idx, ch) in target_line.char_indices() {
        if units == target_units {
            col_idx = Some(idx);
            break;
        }
        let next = units + ch.len_utf16() as u32;
        if target_units < next {
            return None;
        }
        units = next;
    }
    let col_idx = col_idx.or_else(|| (units == target_units).then_some(target_line.len()))?;
    let start = target_line[..col_idx]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(col_idx, |(i, _)| i);
    let end = target_line[col_idx..]
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
        .map_or(target_line.len(), |(i, _)| col_idx + i);
    let name = &target_line[start..end];
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}
