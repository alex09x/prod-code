/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A real gopls behind the scripted gateway, for the Go refactorings' real-server tests.

mod bridge;
mod module;
mod process;

pub use bridge::GoplsBridge;
pub use module::{GoModule, require_go_toolchain};
pub use process::uri;
