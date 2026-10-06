/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::Diagnostic;

pub fn parse_colon_diagnostics(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let mut parts = line.splitn(4, ':');
            let file = parts.next()?.trim();
            let ln = parts.next()?.trim().parse::<u64>().ok()?;
            let col_part = parts.next()?.trim();
            let rest = parts.next()?.trim();
            if file.is_empty() || file.contains(' ') {
                return None;
            }
            let (col, message) = match col_part.parse::<u64>() {
                Ok(col) => (Some(col), rest.to_string()),
                Err(_) => (None, format!("{col_part}: {rest}")),
            };
            let (level, message) = if let Some(m) = message.strip_prefix("error:") {
                ("error", m.trim().to_string())
            } else if let Some(m) = message.strip_prefix("warning:") {
                ("warning", m.trim().to_string())
            } else if let Some(m) = message.strip_prefix("fatal error:") {
                ("error", m.trim().to_string())
            } else if message.starts_with("note:") {
                return None;
            } else {
                ("error", message)
            };
            Some(Diagnostic {
                level: level.to_string(),
                code: None,
                message,
                file: Some(file.to_string()),
                line: Some(ln),
                column: col,
            })
        })
        .collect()
}

/// Rewrites diagnostic file paths that the tool printed as absolute server paths into
/// checkout-relative ones.
pub fn relativize_diagnostics(diagnostics: &mut [Diagnostic], server_root: &str) {
    if server_root.is_empty() {
        return;
    }
    let prefix = format!("{}/", server_root.trim_end_matches('/'));
    for diagnostic in diagnostics {
        if let Some(file) = diagnostic.file.as_mut()
            && let Some(rel) = file.strip_prefix(&prefix)
        {
            *file = rel.to_string();
        }
    }
}
