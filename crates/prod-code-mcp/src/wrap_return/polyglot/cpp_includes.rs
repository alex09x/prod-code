/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::cpp_search::Search;
use super::{cpp_boundary::code, cpp_signature::conditional};
use crate::{caller_migration::mask::lexical_code_mask, parameter_object::Language};
use std::path::{Path, PathBuf};

/// Carry a proven-global context along every include edge. A header included
/// within a namespace, block or conditional cannot supply global fallback facts.
/// Precise declaration references can still establish its selected membership.
pub(super) fn includes(file: &Path, text: &str, search: &Search) -> Vec<(PathBuf, bool)> {
    let mask = lexical_code_mask(text, Language::Cpp);
    let clean = code(text);
    let mut offset = 0;
    text.split_inclusive('\n')
        .filter_map(|line| {
            let begin = offset;
            offset += line.len();
            let trimmed = line.trim_start();
            let hash = begin + line.len() - trimmed.len();
            if !mask.get(hash).copied().unwrap_or(false) {
                return None;
            }
            let after = trimmed
                .strip_prefix('#')?
                .trim_start()
                .strip_prefix("include")?
                .trim_start();
            let (rest, delimiter) = if let Some(rest) = after.strip_prefix('"') {
                (rest, '"')
            } else {
                (after.strip_prefix('<')?, '>')
            };
            let spec = rest.split(delimiter).next()?;
            let path = search.resolve(file, spec, delimiter == '"')?;
            let depth = clean[..hash].bytes().fold(0isize, |depth, b| match b {
                b'{' => depth + 1,
                b'}' => depth - 1,
                _ => depth,
            });
            Some((
                std::fs::canonicalize(&path).unwrap_or(path),
                depth == 0 && !conditional(text, hash),
            ))
        })
        .collect()
}
