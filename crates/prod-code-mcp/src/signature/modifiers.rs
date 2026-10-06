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
use std::net::SocketAddr;
use std::path::Path;

use crate::signature::parse::{is_ident, split_at_top_level};
use crate::signature::references::references;
use crate::signature::types::Declared;
use crate::signature::util::{display, position_at};

/// The declared return type of the function whose parameter list closes at `close` (`()` when it
/// declares none), and `header` with it replaced by `returns`.
pub fn with_return_type(header: &str, close: usize, returns: &str) -> (String, String) {
    let returns = returns.trim();
    match crate::wrap_return::declared_return(header, close) {
        Some((start, end)) => {
            let was = header[start..end].to_string();
            let mut out = header.to_string();
            if returns == "()" || returns.is_empty() {
                // ` -> T` goes, from the arrow on.
                let arrow = header[..start].rfind("->").unwrap_or(start);
                let from = header[..arrow].trim_end().len();
                out.replace_range(from..end, "");
            } else {
                out.replace_range(start..end, returns);
            }
            (was, out)
        }
        None => {
            let mut out = header.to_string();
            if returns != "()" && !returns.is_empty() {
                out.insert_str(close + 1, &format!(" -> {returns}"));
            }
            ("()".to_string(), out)
        }
    }
}

/// The visibility of the function whose name starts at `name_at`, and `text` with it replaced by
/// `visibility` (`private` removes it).
pub fn with_visibility(text: &str, name_at: usize, visibility: &str) -> Option<(String, String)> {
    let fn_kw = text[..name_at].trim_end().strip_suffix("fn")?.len();
    let line_start = text[..fn_kw].rfind('\n').map_or(0, |i| i + 1);
    let indent_end =
        line_start + (text[line_start..].len() - text[line_start..].trim_start().len());
    let head = &text[indent_end..fn_kw];
    let (was, rest_at) = if let Some(rest) = head.strip_prefix("pub(") {
        let close = rest.find(')')?;
        (head[..4 + close + 1].to_string(), 4 + close + 1)
    } else if head.starts_with("pub ") {
        ("pub".to_string(), 3)
    } else {
        ("private".to_string(), 0)
    };
    let rest = head[rest_at..].trim_start();
    let visibility = visibility.trim();
    let new_head = if visibility == "private" || visibility.is_empty() {
        rest.to_string()
    } else {
        format!("{visibility} {rest}")
    };
    let mut out = text.to_string();
    out.replace_range(indent_end..fn_kw, &new_head);
    Some((was, out))
}

/// `text` with `async` put in front of the `fn` at `fn_at` (before `unsafe`, which comes after
/// it), or taken away.
pub fn with_async(text: &str, fn_at: usize, want: bool) -> String {
    let line_start = text[..fn_at].rfind('\n').map_or(0, |i| i + 1);
    let head = &text[line_start..fn_at];
    let mut out = text.to_string();
    if want {
        let at = match head.rfind("unsafe ") {
            Some(i) if head[i + "unsafe ".len()..].trim().is_empty() => line_start + i,
            _ => fn_at,
        };
        out.insert_str(at, "async ");
    } else if let Some(i) = head.rfind("async ") {
        out.replace_range(line_start + i..line_start + i + "async ".len(), "");
    }
    out
}

/// Whether the code at `at` is in the body of an `async fn`.
pub fn in_async_fn(text: &str, at: usize) -> bool {
    let Some((open, _)) = crate::introduce_variable::enclosing_body(text, at) else {
        return false;
    };
    let Some(fn_at) = text[..open].rfind("fn ") else {
        return false;
    };
    let start = text[..fn_at]
        .rfind(['\n', ';', '}', '{'])
        .map_or(0, |i| i + 1);
    text[start..fn_at].split_whitespace().any(|w| w == "async")
}

/// A parameter the body still uses cannot just disappear. The analyzer knows where a local
/// is used; the caller gets the list instead of a file that no longer compiles.
pub async fn check_dropped_parameters(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    declared: &[Declared],
    dropped: &[String],
    open: usize,
    close: usize,
    text: &str,
    force: bool,
) -> Result<()> {
    if dropped.is_empty() || force {
        return Ok(());
    }
    let mut used = Vec::new();
    for gone in dropped {
        let d = declared
            .iter()
            .find(|d| &d.name == gone)
            .with_context(|| format!("`{gone}` is not a declared parameter"))?;
        anyhow::ensure!(
            is_ident(gone),
            "`{gone}` is a pattern, and whether the body still uses what it binds cannot be \
             asked; nothing was written. Pass `force: true` to remove it anyway"
        );
        // The name itself, not the start of `mut b: T` or of an attribute before it.
        let head = split_at_top_level(&d.raw, ':').map_or(d.raw.as_str(), |(h, _)| h);
        let at = text[open..close]
            .find(&d.raw)
            .and_then(|i| head.rfind(gone.as_str()).map(|n| open + i + n))
            .with_context(|| {
                format!("`{gone}` is not where the parameter list says; nothing was written")
            })?;
        let (l, c) = position_at(text, at)?;
        let refs = references(remote, root, file, l, c)
            .await
            .with_context(|| {
                format!("cannot ask whether the body still uses `{gone}`; nothing was written")
            })?;
        let inside: Vec<String> = refs
            .into_iter()
            .filter(|(p, rl, _)| p == file && *rl != l)
            .map(|(p, rl, rc)| format!("{}:{rl}:{rc}", display(root, &p)))
            .collect();
        if !inside.is_empty() {
            used.push(format!(
                "`{gone}` is used {} time(s): {}",
                inside.len(),
                inside.join(", ")
            ));
        }
    }
    if !used.is_empty() {
        anyhow::bail!(
            "these parameters are still used by the body:\n  {}\npass `force: true` to \
             remove them anyway and fix the body afterwards",
            used.join("\n  ")
        );
    }
    Ok(())
}
