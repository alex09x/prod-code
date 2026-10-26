/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::paths::{join_checked, slash_path};
use std::path::Path;

pub(crate) fn strip_ansi_codes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Cargo short-output paths are command-cwd-relative or absolute. A matching
/// directory prefix never establishes repository origin.
pub(crate) fn qualify_short_error(
    command_root: &Path,
    subdir: Option<&str>,
    error: String,
) -> String {
    let Some((position, rest)) = error.split_once(": error") else {
        return error;
    };
    let mut parts = position.rsplitn(3, ':');
    let (Some(col), Some(row), Some(file)) = (parts.next(), parts.next(), parts.next()) else {
        return error;
    };
    let path = Path::new(file);
    let qualified = if path.is_absolute() {
        // Shadow execution preserves the server workspace path at its mount
        // point. Only that explicit response mapping can rebase an absolute path.
        path.strip_prefix(command_root)
            .ok()
            .and_then(|rel| join_checked(Path::new(""), rel))
            .map_or_else(|| file.to_string(), |rel| slash_path(&rel))
    } else if let Some(joined) = join_checked(Path::new(subdir.unwrap_or("")), path) {
        slash_path(&joined)
    } else {
        format!("{}/{file}", subdir.unwrap_or("."))
    };
    format!("{qualified}:{row}:{col}: error{rest}")
}
