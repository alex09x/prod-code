/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Changing a declared type, and reporting the whole shape of what that breaks.
//!
//! It moves the declaration and then tells the truth about the size of the job, site by site,
//! before any of it is done. Where a site's error is exactly the old type meeting the new one it
//! says what conversion would fix it. With `convert` it goes one step further and writes
//! `.into()` at those sites — but only where the analyzer, checking the whole overlay again,
//! accepts it. A wrong `.into()` inserted at forty call sites is the kind of plausible damage the
//! rest of these tools exist to avoid, so a conversion that does not type-check is taken back and
//! its site stays in the report, and a set of conversions that breaks anything else is dropped.

#![allow(clippy::collapsible_if, clippy::needless_range_loop)]

mod call_matching;
mod conversion;
mod execute;
mod matching;
mod sites;
mod spans;
mod transitive;
mod types;

#[cfg(test)]
mod tests;

pub use conversion::{into_call, language_conversion, suggest, type_name};
pub use execute::{migrate, migrate_ext};
pub use spans::{
    declared_type_span, declared_type_span_polyglot, expression_span, find_symbol_decl_offset,
};
pub use types::{Conversion, Migration, Site};
