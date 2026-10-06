/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use crate::extract_function::tokens::{Token, tokens};
use crate::extract_function::types::Occurrence;

/// The places in `text` whose tokens are those of `selection`, outside `exclude`. With
/// `literals`, a place may differ from it in literals of the same kind (a number for a
/// number, a string for a string); without, it must be the same token for token. When the
/// selection is statements (it ends with `;` or `}`), a place must start a statement too.
pub fn copies_of(
    selection: &str,
    text: &str,
    exclude: Option<(usize, usize)>,
    literals: bool,
) -> Vec<Occurrence> {
    let wanted = tokens(selection);
    let have = tokens(text);
    if wanted.is_empty() || wanted.len() > have.len() {
        return Vec::new();
    }
    let statements = selection.trim_end().ends_with(';') || selection.trim_end().ends_with('}');
    let token = |src: &str, (_, s, e): (Token, usize, usize)| -> String { src[s..e].to_string() };
    let mut out = Vec::new();
    let mut k = 0;
    while k + wanted.len() <= have.len() {
        let window = &have[k..k + wanted.len()];
        let mut differs = Vec::new();
        let same = wanted.iter().zip(window).enumerate().all(|(n, (w, h))| {
            let (wt, ht) = (token(selection, *w), token(text, *h));
            if wt == ht {
                return true;
            }
            let literal = matches!(w.0, Token::Number | Token::Str | Token::Char);
            if literals && literal && w.0 == h.0 {
                differs.push((n, ht));
                return true;
            }
            false
        });
        let (from, to) = (window[0].1, window[window.len() - 1].2);
        let overlaps = exclude.is_some_and(|(s, e)| from < e && s < to);
        let starts_statement = !statements
            || text[..from]
                .trim_end()
                .chars()
                .next_back()
                .is_none_or(|c| matches!(c, '{' | ';' | '}'));
        if same && !overlaps && starts_statement {
            out.push(Occurrence { from, to, differs });
            k += wanted.len();
        } else {
            k += 1;
        }
    }
    out
}

/// Every `.rs` file under the `src` of the crate that holds `file`, but `file`.
pub fn crate_sources(file: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let Some(src) = file.ancestors().find(|d| {
        d.file_name().is_some_and(|n| n == "src")
            && d.parent().is_some_and(|p| p.join("Cargo.toml").is_file())
    }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk(src, &mut out);
    out.retain(|p| p != file);
    out.sort();
    out
}

pub fn collect_copies(
    root: &Path,
    file: &Path,
    text: &str,
    selection: &str,
    start: usize,
    end: usize,
    duplicates: bool,
    parameterize: bool,
    other_files: bool,
) -> Vec<(PathBuf, String, Occurrence)> {
    let mut copies = Vec::new();
    if !duplicates {
        return copies;
    }

    for c in copies_of(selection, text, Some((start, end)), parameterize) {
        copies.push((file.to_path_buf(), text.to_string(), c));
    }
    if other_files {
        let mut others = crate_sources(file);
        for s in crate::signature_polyglot::collect_workspace_sources(
            root,
            crate::parameter_object::Language::Rust,
        ) {
            if s != file && !others.contains(&s) {
                others.push(s);
            }
        }
        for other in others {
            let Ok(other_text) = std::fs::read_to_string(&other) else {
                continue;
            };
            for c in copies_of(selection, &other_text, None, parameterize) {
                copies.push((other.clone(), other_text.clone(), c));
            }
        }
    }
    copies
}
