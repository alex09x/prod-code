/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

pub(crate) fn hash_slice(slice: &[u64]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for &h in slice {
        h.hash(&mut hasher);
    }
    hasher.finish()
}

pub(crate) fn hash_string(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn multiset_jaccard(a: &[String], b: &[String]) -> f64 {
    let mut counts_a: HashMap<&str, usize> = HashMap::new();
    for s in a {
        if !s.is_empty() {
            *counts_a.entry(s.as_str()).or_default() += 1;
        }
    }
    let mut counts_b: HashMap<&str, usize> = HashMap::new();
    for s in b {
        if !s.is_empty() {
            *counts_b.entry(s.as_str()).or_default() += 1;
        }
    }
    let mut all_keys: HashSet<&str> = counts_a.keys().copied().collect();
    all_keys.extend(counts_b.keys().copied());

    let mut intersection = 0usize;
    let mut union = 0usize;
    for k in all_keys {
        let ca = counts_a.get(k).copied().unwrap_or(0);
        let cb = counts_b.get(k).copied().unwrap_or(0);
        intersection += ca.min(cb);
        union += ca.max(cb);
    }
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Normalizes a source code line: strips comments and normalizes tokens.
pub(crate) fn normalize_line(line: &str, parameterized: bool) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
    {
        return String::new();
    }

    // Strip inline comments
    let code_part = if let Some(idx) = trimmed.find("//") {
        &trimmed[..idx]
    } else if let Some(idx) = trimmed.find('#') {
        &trimmed[..idx]
    } else {
        trimmed
    };

    if !parameterized {
        return code_part.split_whitespace().collect::<Vec<_>>().join(" ");
    }

    // Parameterized normalization: normalize variable names to $id and literals to $lit
    let mut result = String::with_capacity(code_part.len());
    let words = code_part.split_whitespace();

    for word in words {
        if is_numeric_literal(word) {
            result.push_str("$lit ");
        } else if word.starts_with('"') && word.ends_with('"') {
            result.push_str("$str ");
        } else {
            result.push_str(word);
            result.push(' ');
        }
    }

    result.trim_end().to_string()
}

pub(crate) fn is_numeric_literal(s: &str) -> bool {
    let clean = s.trim_end_matches([',', ';', ')', '}', ']']);
    clean.parse::<f64>().is_ok()
}
