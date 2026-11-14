/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What a diagnostics pass does when rust-analyzer panics in it (#2025).
//!
//! A pass asks the analyzer several things: the file's own diagnostics in one call, and the
//! assists of each `use` item, from which the unused imports are read. A panic in any of those
//! calls took the whole pass with it: the file had no diagnostics at all, and nothing said why.
//! Each call is now made on its own, and a call that panics costs only its own answer, which
//! the pass names where an editor shows it.

use anyhow::Result;
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::types::FileDiagnostic;

/// The code of a diagnostic that says the analyzer failed on a file or on a part of it.
pub const ANALYZER_FAILED: &str = "analyzer-failed";

/// What `ask` answers, or the message of the panic it ended in. A query a write cancelled does
/// not panic here: the analyzer returns it as an error inside `T`.
pub(super) fn caught<T>(ask: impl FnOnce() -> T) -> std::result::Result<T, String> {
    catch_unwind(AssertUnwindSafe(ask)).map_err(|panic| panic_text(panic.as_ref()))
}

/// What `full` computes, or what `syntax_only` computes and the analyzer's own words when
/// `full` panics. A cancelled query is an error of either, as it was: the pass is run again
/// for the newer state.
pub(super) fn or_syntax_only<T, E>(
    full: impl FnOnce() -> std::result::Result<T, E>,
    syntax_only: impl FnOnce() -> std::result::Result<T, E>,
) -> Result<(T, Option<String>)>
where
    E: std::error::Error + Send + Sync + 'static,
{
    match caught(full) {
        Ok(done) => Ok((done?, None)),
        Err(why) => Ok((syntax_only()?, Some(why))),
    }
}

/// The message a panic carried.
fn panic_text(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_string())
}

/// The diagnostic on the first line of `text`, a file whose own diagnostics the analyzer
/// failed to compute. A warning, not an error: the file is not known to be wrong, it is not
/// fully checked.
pub(super) fn file_notice(why: &str, text: &str) -> FileDiagnostic {
    let first_line = text.lines().next().unwrap_or_default();
    FileDiagnostic {
        code: ANALYZER_FAILED.to_string(),
        message: format!(
            "rust-analyzer failed while checking this file, so type errors and its other \
             semantic diagnostics are missing here. Syntax errors, unused imports and unresolved \
             paths are still reported; `cargo check` reports the rest. The analyzer said: {why}"
        ),
        severity: "warning".to_string(),
        line: 1,
        col: 1,
        end_line: 1,
        end_col: 1 + first_line.encode_utf16().count() as u32,
        unused: false,
    }
}

/// The diagnostic on a `use` item whose assists the analyzer failed to compute, at the item's
/// 1-based `start` and `end`. A hint: the rest of the file is checked as usual, and only this
/// item is not looked at for unused imports.
pub(super) fn use_item_notice(why: &str, start: (u32, u32), end: (u32, u32)) -> FileDiagnostic {
    FileDiagnostic {
        code: ANALYZER_FAILED.to_string(),
        message: format!(
            "rust-analyzer failed on this `use` item, so it is not checked for unused imports; \
             `cargo check` reports them. The analyzer said: {why}"
        ),
        severity: "weak".to_string(),
        line: start.0,
        col: start.1,
        end_line: end.0,
        end_col: end.1,
        unused: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Error;

    #[test]
    fn a_panic_in_the_full_pass_leaves_the_syntax_pass_and_the_reason() {
        let (found, why) = or_syntax_only(
            || -> Result<Vec<&str>, Error> { unreachable!("mapping more input nodes") },
            || Ok(vec!["syntax error"]),
        )
        .unwrap();
        assert_eq!(found, ["syntax error"]);
        let why = why.expect("the reason is kept");
        assert!(why.contains("mapping more input nodes"), "{why}");
        let said = file_notice(&why, "//! Модуль\nfn main() {}\n");
        assert_eq!(said.severity, "warning");
        // The first line, in UTF-16 columns.
        assert_eq!(
            (said.line, said.col, said.end_line, said.end_col),
            (1, 1, 1, 11)
        );
        assert!(
            said.message.contains("mapping more input nodes")
                && said.message.contains("`cargo check`"),
            "{}",
            said.message
        );

        // A panic with an owned message is read as well.
        assert_eq!(
            caught(|| -> u32 { panic!("{} nodes", 3) }),
            Err("3 nodes".to_string())
        );
        assert_eq!(caught(|| 7), Ok(7));
    }

    #[test]
    fn a_pass_that_does_not_panic_is_untouched_and_a_cancelled_one_stays_cancelled() {
        let (found, why) = or_syntax_only(
            || Ok(vec!["type mismatch"]),
            || -> Result<Vec<&str>, Error> { unreachable!("the syntax pass is the fallback") },
        )
        .unwrap();
        assert_eq!((found, why), (vec!["type mismatch"], None));

        // What the analyzer returns for a query a write cancelled is an error, not a panic.
        let cancelled = or_syntax_only(
            || -> Result<(), Error> { Err(Error::other("the query was cancelled")) },
            || -> Result<(), Error> { unreachable!("a cancelled pass has no fallback") },
        )
        .expect_err("a cancelled pass has no answer");
        assert!(crate::is_salsa_cancelled(&cancelled), "{cancelled:#}");
    }
}
