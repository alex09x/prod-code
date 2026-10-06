/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::lang::Language;
use super::scope::has_complete_expression_boundaries;
use super::tokenize::tokenize_polyglot;
use super::types::{PolyOccurrence, PolyTokenKind};

pub fn find_duplicates_in_text(
    selection: &str,
    text: &str,
    exclude: Option<(usize, usize)>,
    parameterize: bool,
    lang: Language,
) -> Vec<PolyOccurrence> {
    let wanted = tokenize_polyglot(selection, lang);
    let have = tokenize_polyglot(text, lang);
    if wanted.is_empty() || wanted.len() > have.len() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut k = 0;
    while k + wanted.len() <= have.len() {
        let window = &have[k..k + wanted.len()];
        let mut differs = Vec::new();
        let matches = wanted.iter().zip(window).enumerate().all(|(idx, (w, h))| {
            if w.text == h.text {
                return true;
            }
            if parameterize
                && w.kind == h.kind
                && matches!(
                    w.kind,
                    PolyTokenKind::Number | PolyTokenKind::Str | PolyTokenKind::Char
                )
            {
                differs.push((idx, h.text.clone()));
                return true;
            }
            false
        });

        let (from, to) = (window[0].start, window.last().unwrap().end);
        let overlaps = exclude.is_some_and(|(s, e)| from < e && s < to);

        if matches && !overlaps && has_complete_expression_boundaries(text, from, to) {
            out.push(PolyOccurrence {
                start: from,
                end: to,
                differs,
            });
            k += wanted.len();
        } else {
            k += 1;
        }
    }

    out
}
