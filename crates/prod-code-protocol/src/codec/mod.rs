/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Binary framing codec for prod-code WireMessage streams.

mod decoder;
mod encoder;
mod types;

#[cfg(test)]
mod tests;

pub use types::{MAX_FRAME_SIZE, ProdCodeCodec};
