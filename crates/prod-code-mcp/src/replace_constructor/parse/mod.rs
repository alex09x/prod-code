/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod cpp_swift;
pub mod go;
pub mod python;
pub mod rust;
pub mod ts;

use super::types::StructDecl;
use anyhow::Result;

pub use cpp_swift::{
    parse_cpp_fields, parse_cpp_struct_decl, parse_swift_fields, parse_swift_struct_decl,
};
pub use go::{parse_go_struct_decl, parse_go_struct_fields};
pub use python::{parse_python_fields, parse_python_struct_decl};
pub use rust::{parse_rust_struct_decl, parse_rust_struct_fields};
pub use ts::{parse_javascript_fields, parse_ts_fields, parse_ts_struct_decl};

pub(crate) fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub(crate) fn is_ident_str(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        && s.chars().all(is_ident)
}

/// Extracts generic parameters `<T, U>` starting at or after `from`.
pub fn extract_generics(text: &str, from: usize) -> Option<(String, usize)> {
    let rest = text[from..].trim_start();
    if !rest.starts_with('<') {
        return None;
    }
    let offset = text[from..].len() - rest.len();
    let open = from + offset;
    let mut depth = 0i32;
    let mut end = open;
    for (i, c) in text[open..].char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    end = open + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth == 0 {
        Some((text[open..end].to_string(), end))
    } else {
        None
    }
}

/// Splits comma-separated items while respecting brackets and string literals.
pub fn split_balanced_commas(inner: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' | b'(' | b'[' | b'{' => depth += 1,
            b'>' | b')' | b']' | b'}' => depth = (depth - 1).max(0),
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b',' if depth == 0 => {
                let chunk = inner[start..i].trim();
                if !chunk.is_empty() {
                    items.push(chunk.to_string());
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let tail = inner[start..].trim();
    if !tail.is_empty() {
        items.push(tail.to_string());
    }
    items
}

/// Parses the declaration of `type_name` in `text` given its language.
pub fn parse_struct_declaration(text: &str, type_name: &str, language: &str) -> Result<StructDecl> {
    match language {
        "rust" => parse_rust_struct_decl(text, type_name),
        "go" => parse_go_struct_decl(text, type_name),
        "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => {
            parse_ts_struct_decl(text, type_name, language)
        }
        "python" => parse_python_struct_decl(text, type_name),
        "cpp" | "c" => parse_cpp_struct_decl(text, type_name, language),
        "swift" => parse_swift_struct_decl(text, type_name),
        _ => anyhow::bail!("unsupported language `{language}` for replace_constructor"),
    }
}
