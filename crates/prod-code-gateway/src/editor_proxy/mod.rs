/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The editor's own language server on the node (#332).
//!
//! An editor that runs `prod-code lsp` wants the language server it would run locally
//! (rust-analyzer, gopls, clangd) with everything that comes with it.

pub mod child;
pub mod command;
pub mod probe;
pub(crate) mod probe_task;
pub mod proxy;
pub(crate) mod reader;
pub mod registry;

#[cfg(test)]
mod tests;

pub use command::{
    ServerCommand, enabled, server_command, server_command_for_workspace, to_server,
};
pub use probe::{
    DEFAULT_HEALTH_PROBE_INTERVAL, DEFAULT_HEALTH_RESPONSE_TIMEOUT, EditorProxyOptions,
    HEALTH_PROBE_ID_PREFIX, HEALTH_PROBE_METHOD, MAX_IDLE_PROBE_TIMEOUTS, ProbeState,
    health_probe_sequence, valid_dispatch_response,
};
pub use proxy::{run, run_with_budgets, run_with_options};
pub use registry::{EditorServers, Registration};
