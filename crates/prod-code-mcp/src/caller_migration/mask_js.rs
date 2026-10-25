/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "mask_js_await.rs"]
mod await_context;
#[path = "mask_js_yield.rs"]
mod yield_context;

pub(crate) fn ambiguous_regex_prefix(text: &[u8], code: &[bool]) -> bool {
    yield_context::ambiguous_regex_prefix(text, code)
        || await_context::ambiguous_regex_prefix(text, code)
}

/// A keyword spelling after member access is a value, so its following slash
/// divides rather than starting a regex. Search the existing code mask so
/// comments and whitespace cannot hide the dot or private-name marker.
pub(super) fn keyword_allows_regex(text: &[u8], code: &[bool], word_start: usize) -> bool {
    let previous = text[..word_start]
        .iter()
        .zip(&code[..word_start])
        .rev()
        .find(|(byte, executable)| **executable && !byte.is_ascii_whitespace());
    if previous.is_some_and(|(byte, _)| matches!(byte, b'.' | b'#')) {
        return false;
    }
    if &text[word_start..] == b"of" {
        return for_of_operator(text, code, word_start);
    }
    if &text[word_start..] == b"yield" {
        return yield_context::is_operator(&text[..word_start], &code[..word_start])
            .unwrap_or(true);
    }
    regex_prefix_keyword(&text[word_start..])
}

fn regex_prefix_keyword(word: &[u8]) -> bool {
    [
        &b"return"[..],
        b"typeof",
        b"void",
        b"delete",
        b"throw",
        b"new",
        b"in",
        b"instanceof",
        b"case",
        b"yield",
        b"await",
        b"else",
        b"do",
    ]
    .contains(&word)
}

/// `of` is an identifier except between a for-loop binding and its iterable.
/// An ordinary value named `of` must leave a subsequent division operand visible.
fn for_of_operator(text: &[u8], code: &[bool], word_start: usize) -> bool {
    let prefix: Vec<_> = text[..word_start]
        .iter()
        .zip(&code[..word_start])
        .map(|(&b, &executable)| if executable { b } else { b' ' })
        .collect();
    let mut depth = 0;
    let Some(open) = prefix.iter().rposition(|b| match b {
        b')' => {
            depth += 1;
            false
        }
        b'(' if depth > 0 => {
            depth -= 1;
            false
        }
        b'(' => true,
        _ => false,
    }) else {
        return false;
    };
    let (mut before, mut word) = trailing_word(&prefix[..open]);
    if word == b"await" {
        (before, word) = trailing_word(before);
    }
    if word != b"for" || before.trim_ascii_end().ends_with(b".") {
        return false;
    }

    let mut binding = prefix[open + 1..].trim_ascii_end();
    let mut invert = false;
    loop {
        let (before, word) = trailing_word(binding);
        // A property spelling remains a value, even when it is a keyword.
        let member =
            before.trim_ascii_end().ends_with(b".") || before.trim_ascii_end().ends_with(b"#");
        if word == b"of" && !member {
            // In `for (of of iterable)` the first `of` is a binding; in
            // `for (x of of / divisor)` the second one is an iterable value.
            // Walk rather than recurse over consecutive contextual spellings.
            invert = !invert;
            binding = before.trim_ascii_end();
            continue;
        }
        let value = if word.is_empty() {
            binding
                .last()
                .is_some_and(|b| matches!(b, b')' | b']' | b'}'))
        } else {
            member || (!matches!(word, b"const" | b"let" | b"var") && !regex_prefix_keyword(word))
        };
        return value ^ invert;
    }
}

fn trailing_word(text: &[u8]) -> (&[u8], &[u8]) {
    let text = text.trim_ascii_end();
    let start = text
        .iter()
        .rposition(|b| !b.is_ascii_alphanumeric() && !matches!(b, b'_' | b'$' | 0x80..=0xff))
        .map_or(0, |i| i + 1);
    (&text[..start], &text[start..])
}
