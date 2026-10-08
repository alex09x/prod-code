/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Shadow runs: run a command once per hypothesis, a hypothesis being a set of
//! proposed file contents laid over the server workspace.

pub mod hypothesis_view;
pub mod in_place;
pub mod overlay;
pub mod process;
pub mod root;
pub mod runner;
pub mod sccache;
pub mod staging;
pub mod tail_buffer;
pub mod types;

pub use in_place::run_in_place;
pub use overlay::run_overlay;
pub use root::{
    ShadowRootOwner, default_root, overlay_unavailable, ram_shadow_root, remove_shadow_dir,
};
pub use runner::run_shadow;
pub use sccache::{ensure_sccache_server, find_sccache};
pub use tail_buffer::TailBuffer;
pub use types::{DEFAULT_TAIL_BYTES, HYPOTHESIS_DIR_PREFIX, Job, OWNERSHIP_LOCK_FILE};

#[cfg(test)]
mod tests;
