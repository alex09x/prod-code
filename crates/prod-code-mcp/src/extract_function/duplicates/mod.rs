/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod search;
pub mod validate;

pub(crate) use search::collect_copies_with_ownership;
pub use search::{collect_copies, copies_of, crate_sources};
pub use validate::process_and_validate_duplicates;
pub(crate) use validate::process_and_validate_duplicates_with_ownership;
