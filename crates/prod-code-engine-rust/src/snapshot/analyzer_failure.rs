/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What a diagnostics pass does when rust-analyzer panics on one `use` item (#2025).
//!
//! After the analyzer's own diagnostics, a pass asks each `use` item for its assists, and reads
//! the unused imports from them. The analyzer computes every assist to answer, and a panic in
//! one the pass never looks at took the whole pass with it: the file had no diagnostics at all.
//! The item is now asked on its own; a panic costs its answer only, and the item says so.
//!
//! A panic in the analyzer's own diagnostics is not caught here. Nothing in that file was type
//! checked, and the caller reports it as such (`prod_code_protocol::ANALYZER_PANIC_CODE`), so
//! that no edit to it is taken for checked.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::types::FileDiagnostic;

/// The code of the hint on a `use` item the analyzer failed on. The rest of the file was
/// checked, so this is not the code of a file that was not.
pub const ANALYZER_FAILED: &str = "analyzer-failed";

/// What `ask` answers, or the message of the panic it ended in. A query a write cancelled does
/// not panic here: the analyzer returns it as an error inside `T`.
pub(super) fn caught<T>(ask: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(ask)).map_err(|panic| panic_text(panic.as_ref()))
}

/// The message a panic carried.
fn panic_text(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_string())
}

/// The diagnostic on a `use` item whose assists the analyzer failed to compute, at the item's
/// 1-based `start` and `end`. A hint: the file is checked as usual, and only this item is not
/// looked at for unused imports.
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

    #[test]
    fn a_panic_is_caught_with_its_message_and_an_answer_passes_through() {
        assert_eq!(
            caught(|| -> u32 { unreachable!("mapping more input nodes") }),
            Err("internal error: entered unreachable code: mapping more input nodes".to_string())
        );
        // A panic with an owned message is read as well.
        assert_eq!(
            caught(|| -> u32 { panic!("{} nodes", 3) }),
            Err("3 nodes".to_string())
        );
        // What the analyzer returns for a query a write cancelled is an answer, not a panic.
        let cancelled: Result<Result<u32, &str>, String> = caught(|| Err("cancelled"));
        assert_eq!(cancelled, Ok(Err("cancelled")));

        let said = use_item_notice("3 nodes", (9, 1), (9, 31));
        assert_eq!(
            (said.severity.as_str(), said.line, said.end_col),
            ("weak", 9, 31)
        );
        assert!(
            said.message.contains("not checked for unused imports")
                && said.message.contains("3 nodes"),
            "{}",
            said.message
        );
    }
}
