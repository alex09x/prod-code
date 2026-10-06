/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Remote command execution: sync the checkout, then run a command inside its server copy and
//! stream the output back. Shared by the CLI (`prod-code exec`) and the MCP tool `code_exec`.

pub mod platform;
pub mod runner;
pub mod types;

#[cfg(test)]
mod tests;

pub use platform::{layout_only, platform_warning, subdir_of};
pub use runner::{run_polyglot_remote, run_remote};
pub use types::{PolyglotRemoteOutcome, RemoteOutcome, TailBuffer};
