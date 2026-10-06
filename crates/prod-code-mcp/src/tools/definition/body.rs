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

use super::range::outlined_range;

/// The most lines of a definition `body: true` shows.
pub(crate) const MAX_BODY_LINES: usize = 300;

/// The code of the definition at 1-based `line`/`col` of `uri`, numbered (#306): the item's
/// range from the file's outline or, for a file outside the checkout or a server without an
/// outline, the lines its brackets (or, after a `:`, its indentation) span. The doc comments,
/// attributes and decorators right above it come with it.
pub(crate) async fn definition_body(
    remote: SocketAddr,
    root: &Path,
    uri: &str,
    line: u32,
    col: u32,
) -> Result<String> {
    let path = crate::remote_fs::uri_to_path(uri);
    let external = crate::remote_fs::is_external(root, &path);
    let text = if external {
        let (bytes, _) = crate::remote_fs::read_remote_file(remote, &path, 0).await?;
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?
    };
    let lines: Vec<&str> = text.lines().collect();
    anyhow::ensure!(!lines.is_empty(), "{path} is empty");
    let start = (line as usize).saturating_sub(1).min(lines.len() - 1);
    let outlined = if external {
        None
    } else {
        outlined_range(
            remote,
            root,
            Path::new(&path),
            start,
            col.saturating_sub(1) as usize,
        )
        .await
    };
    let (first, last) = match outlined {
        Some((first, last)) if last >= start => (first.min(start), last),
        _ => (start, item_end(&lines, start)),
    };
    Ok(numbered_lines(
        &lines,
        with_leading_docs(&lines, first),
        last,
    ))
}

/// The last line of the item that starts on 0-based line `start`: where its first `{` is
/// closed; for a line that ends in `:` (Python), the last line indented deeper; else the line
/// itself (`type A = B;`).
pub(crate) fn item_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i64;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(start).take(2000) {
        let chars: Vec<char> = line.chars().collect();
        let mut quote: Option<char> = None;
        let mut k = 0;
        while k < chars.len() {
            let c = chars[k];
            if let Some(q) = quote {
                if c == '\\' {
                    k += 1;
                } else if c == q {
                    quote = None;
                }
                k += 1;
                continue;
            }
            match c {
                '"' | '`' => quote = Some(c),
                // A comment's brackets are not the code's.
                '/' if chars.get(k + 1) == Some(&'/') => break,
                // A char literal (`'{'`, `'\\''`), not a lifetime (`'a`).
                '\'' if chars.get(k + 2) == Some(&'\'') => k += 2,
                '\'' if chars.get(k + 1) == Some(&'\\') && chars.get(k + 3) == Some(&'\'') => {
                    k += 3
                }
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
            k += 1;
        }
        if opened && depth <= 0 {
            return i;
        }
        let trimmed = line.trim_end();
        if !opened && i == start {
            if trimmed.ends_with(':') {
                let indent = line.len() - line.trim_start().len();
                let mut last = start;
                for (j, next) in lines.iter().enumerate().skip(start + 1) {
                    if next.trim().is_empty() {
                        continue;
                    }
                    if next.len() - next.trim_start().len() <= indent {
                        break;
                    }
                    last = j;
                }
                return last;
            }
            if trimmed.ends_with(';') {
                return start;
            }
        }
        if !opened && i > start + 3 {
            return start;
        }
    }
    start
}

/// The first line of the doc comments, attributes and decorators right above 0-based `first`.
pub(crate) fn with_leading_docs(lines: &[&str], mut first: usize) -> usize {
    while first > 0 {
        let above = lines[first - 1].trim_start();
        let doc = ["///", "//", "#[", "@", "/*", "*"]
            .iter()
            .any(|prefix| above.starts_with(prefix));
        if !doc {
            break;
        }
        first -= 1;
    }
    first
}

/// Lines `first..=last` (0-based), numbered from 1, at most [`MAX_BODY_LINES`] of them.
pub(crate) fn numbered_lines(lines: &[&str], first: usize, last: usize) -> String {
    let last = last.min(lines.len().saturating_sub(1));
    let shown = last.min(first + MAX_BODY_LINES - 1);
    let width = (shown + 1).to_string().len();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate().take(shown + 1).skip(first) {
        out.push_str(&format!("{:>width$} | {line}\n", i + 1));
    }
    if shown < last {
        out.push_str(&format!("… {} more line(s)\n", last - shown));
    }
    out.trim_end().to_string()
}
