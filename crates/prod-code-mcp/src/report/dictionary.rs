/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Ordinary English words, read from the system's word list, so that a report is not refused
//! for prose that happens to name a checkout's function or type (#2101, #2103): `rebuild`,
//! `steady` or `Receiver` say nothing private, while `tako` or `PlacementStore` do.

use std::collections::HashSet;
use std::sync::LazyLock;

/// The word lists macOS (`web2`) and Debian/Ubuntu (`wamerican`, `wbritish`) install.
const WORD_LISTS: &[&str] = &[
    "/usr/share/dict/words",
    "/usr/share/dict/american-english",
    "/usr/share/dict/british-english",
];

static SYSTEM: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let mut words = HashSet::new();
    for path in WORD_LISTS {
        if let Ok(text) = std::fs::read_to_string(path) {
            words.extend(common_words(&text));
        }
    }
    words
});

/// The system's English words; empty where none is installed, so nothing more is allowed.
pub(super) fn system() -> &'static HashSet<String> {
    &SYSTEM
}

/// The entries of a word list that are common words: lowercase ASCII letters only. A
/// capitalized entry is a proper name (`Rama`), and a name is what the check is for.
pub(super) fn common_words(list: &str) -> impl Iterator<Item = String> + '_ {
    list.lines()
        .map(str::trim)
        .filter(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
        .map(str::to_string)
}

/// Whether `name`, one word in lowercase or with only its first letter capitalized, is an
/// English word of `dictionary`, also as a plural or an `-ed` / `-ing` form of one.
pub(super) fn is_english_word(name: &str, dictionary: &HashSet<String>) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() || !chars.all(|c| c.is_ascii_lowercase()) {
        return false;
    }
    let word = name.to_ascii_lowercase();
    if dictionary.contains(&word) {
        return true;
    }
    let stems = [
        word.strip_suffix("ies").map(|stem| format!("{stem}y")),
        word.strip_suffix("es").map(str::to_string),
        word.strip_suffix('s').map(str::to_string),
        word.strip_suffix("ed").map(str::to_string),
        word.strip_suffix('d').map(str::to_string),
        word.strip_suffix("ing").map(str::to_string),
        word.strip_suffix("ing").map(|stem| format!("{stem}e")),
    ];
    stems
        .into_iter()
        .flatten()
        .any(|stem| stem.len() >= 3 && dictionary.contains(&stem))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary() -> HashSet<String> {
        common_words("rebuild\nsteady\nlarge\nretry\ndecline\nreceiver\nRama\nbooster\n").collect()
    }

    #[test]
    fn prose_words_and_their_forms_are_english_and_names_are_not() {
        let words = dictionary();
        for word in [
            "rebuild",
            "steady",
            "large",
            "retries",
            "declines",
            "declined",
            "declining",
            "Receiver",
            "rebuilds",
        ] {
            assert!(is_english_word(word, &words), "`{word}` is English");
        }
        for name in [
            "tako",
            "Rama",
            "rama",
            "PlacementStore",
            "placement_store",
            "REBUILD",
            "rebuild2",
            "",
        ] {
            assert!(!is_english_word(name, &words), "`{name}` is a name");
        }
    }
}
