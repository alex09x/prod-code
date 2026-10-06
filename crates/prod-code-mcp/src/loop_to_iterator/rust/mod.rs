/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod chain;
pub mod execute;
pub mod recognise;

pub use chain::{chain, iterator_of};
pub use execute::loop_to_iterator;
pub use recognise::recognise;
