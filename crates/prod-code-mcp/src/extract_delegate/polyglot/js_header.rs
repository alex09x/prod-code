/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Where a JS/TS class's header starts, and whether it is exported: the place a generated
//! helper class goes in front of it.

use std::ops::Range;

use crate::extract_delegate::common::is_ident;

/// `s` without the modifier `kw` it ends with. A modifier counts only in code: `// export`
/// above a class is a comment (#1409).
fn strip_kw<'a>(s: &'a str, kw: &str, non_code: &[Range<usize>]) -> Option<&'a str> {
    s.strip_suffix(kw).filter(|r| {
        !r.chars().next_back().is_some_and(is_ident)
            && !non_code.iter().any(|n| n.contains(&r.len()))
    })
}

pub(super) fn class_header_bounds(
    src: &str,
    (span_start, after_dec, id_start, exp): (usize, usize, usize, Option<usize>),
    non_code: &[Range<usize>],
) -> (bool, usize) {
    if let Some(exp) = exp {
        return (true, exp.min(span_start));
    }
    let class_kw = src.get(after_dec..id_start).map_or(span_start, |s| {
        s.match_indices("class")
            .map(|(rel, _)| after_dec + rel)
            .find(|&p| {
                !non_code.iter().any(|r| r.contains(&p))
                    && !src[..p].chars().next_back().is_some_and(is_ident)
                    && !src[p + 5..].chars().next().is_some_and(is_ident)
            })
            .unwrap_or(span_start)
    });
    let mut h = src[..class_kw].trim_end();
    while let Some(r) =
        strip_kw(h, "abstract", non_code).or_else(|| strip_kw(h, "declare", non_code))
    {
        h = r.trim_end();
    }
    let h = strip_kw(h, "default", non_code).map_or(h, str::trim_end);
    strip_kw(h, "export", non_code).map_or((false, span_start.min(class_kw)), |p| {
        (true, p.len().min(span_start))
    })
}
