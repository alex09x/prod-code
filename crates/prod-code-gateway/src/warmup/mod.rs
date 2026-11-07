/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Loading again, after a restart, the workspaces that were in use before it (#1976).
//!
//! The first request to a workspace is slow and the next ones are fast (Rust `definition`: p50
//! 2.0 s the first time, 8 ms from the 51st), and a restart makes every workspace the first
//! again. The gateway therefore keeps a short record of the workspaces it loaded
//! ([`RecentWorkspaces`]) and, when it starts, loads the newest of them one at a time
//! ([`restore`]), unless the node is busy or short of memory. What is loaded and for which
//! languages is set in the `[warmup]` section of `gateway.toml` ([`WarmupConfig`]).

mod policy;
mod prebuild;
mod recent;
mod restore;
mod start;

pub use policy::{LanguagePolicy, WarmupConfig, When};
pub use recent::{Recent, RecentWorkspaces};
pub use restore::{Ahead, Outcome, Skip, restore};
pub(crate) use start::run;
