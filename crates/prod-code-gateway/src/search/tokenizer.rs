/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Lowercased word tokens: splits on non-alphanumerics, then on camelCase humps, and drops
/// the words that carry no signal in a query.
pub fn tokenize(text: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "a", "an", "of", "to", "in", "on", "for", "and", "or", "is", "are", "was", "were",
        "be", "we", "do", "does", "did", "our", "it", "its", "that", "this", "with", "how", "what",
        "where", "when", "which", "who", "why", "from", "by", "at", "as", "into", "out", "if",
        "then", "than", "so", "but", "not", "no", "yes", "can", "will", "would", "should", "get",
        "set", "new", "use", "used", "using",
    ];
    let mut out = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric()) {
        if raw.is_empty() {
            continue;
        }
        for part in split_humps(raw) {
            let lower = part.to_lowercase();
            if lower.len() > 1 && !STOP.contains(&lower.as_str()) {
                out.push(singular(&lower));
            }
        }
    }
    out
}

/// Folds a simple English plural so `nodes` and `node` are the same term. Deliberately crude:
/// no stemmer, just a trailing `s` on a word long enough for it to mean plural.
pub(crate) fn singular(word: &str) -> String {
    if word.len() > 3 && word.ends_with('s') && !word.ends_with("ss") && !word.ends_with("us") {
        word[..word.len() - 1].to_string()
    } else {
        word.to_string()
    }
}

/// `parseHTTPResponse` → [parse, HTTP, Response]; `snake_case` arrives already split.
pub(crate) fn split_humps(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let cur = chars[i];
        let boundary = (prev.is_lowercase() && cur.is_uppercase())
            || (prev.is_uppercase()
                && cur.is_uppercase()
                && chars.get(i + 1).is_some_and(|n| n.is_lowercase()));
        if boundary {
            parts.push(chars[start..i].iter().collect());
            start = i;
        }
    }
    parts.push(chars[start..].iter().collect());
    parts
        .into_iter()
        .filter(|p: &String| !p.is_empty())
        .collect()
}
