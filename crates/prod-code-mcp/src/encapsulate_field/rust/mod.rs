/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod accessors;
pub mod analysis;
pub mod execute;

pub use accessors::accessors;
pub(crate) use analysis::chain_start;
pub use analysis::{access_at, field_at, inherent_impl, is_generic, owner_at, returns_by_value};
pub use execute::encapsulate;
