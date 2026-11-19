/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The nested projects a symbol search could not ask (#2054).
//!
//! A search across projects keeps going when one project's node or server fails: the other
//! projects, and every fallback after the search, still answer. What was left unasked is noted
//! here, for the one answer that would otherwise read as final, `code_symbols`'s "nothing of
//! that name", and only within [`collect`]; elsewhere a note goes nowhere.

use std::cell::RefCell;
use std::future::Future;

tokio::task_local! {
    static UNANSWERED: RefCell<Vec<String>>;
}

/// Notes a project left unasked and why, when a [`collect`] is listening.
pub(crate) fn note(reason: String) {
    let _ = UNANSWERED.try_with(|notes| {
        let mut notes = notes.borrow_mut();
        if !notes.contains(&reason) {
            notes.push(reason);
        }
    });
}

/// Runs `search`, and returns its output with the projects it noted as left unasked.
pub(crate) async fn collect<T>(search: impl Future<Output = T>) -> (T, Vec<String>) {
    UNANSWERED
        .scope(RefCell::new(Vec::new()), async {
            let output = search.await;
            (output, UNANSWERED.with(|notes| notes.take()))
        })
        .await
}

/// The sentence an answer that found no symbol of the name adds when projects went unasked.
pub(crate) fn sentence(unanswered: &[String]) -> String {
    if unanswered.is_empty() {
        return String::new();
    }
    format!(
        "\nNot every project was searched, so the symbol may still exist: {}. Retry once the node serving it is available, or read the declaration's file with code_source.",
        unanswered.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A note outside [`collect`] goes nowhere; inside, each reason is kept once.
    #[tokio::test]
    async fn notes_reach_only_the_search_that_listens() {
        note("lost".to_string());
        let ((), notes) = collect(async {
            note("the swift project at `swift`: its language server failed".to_string());
            note("the swift project at `swift`: its language server failed".to_string());
        })
        .await;
        assert_eq!(notes.len(), 1);
        assert!(sentence(&notes).contains("Not every project was searched"));
        assert!(sentence(&[]).is_empty());
    }
}
