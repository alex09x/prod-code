/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod execute;
pub mod insertion;
pub mod locate;

pub use execute::extract;
pub use insertion::{field_insertion, literal_insertion};
pub use locate::{
    braces_kind, constructor_brace, impl_blocks, method_at, self_literals, struct_braces,
};
