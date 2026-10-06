/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cross-worktree Swift engine support: shared Swift/Clang ModuleCache,
//! SwiftPM package checkout and artifact seeding, and compilation workspace
//! state relocation (Roadmap 3.7).

pub mod env;
pub mod fs_ops;
pub mod prune;
pub mod seed;
pub mod workspace;

pub use env::{
    SWIFT_MODULE_CACHE_ENV, ensure_cache_dir, swift_module_cache_dir, swift_module_cache_env,
};
#[cfg(test)]
pub(crate) use fs_ops::merge_cache_files;
pub use prune::{prune_stale_module_cache, prune_stale_module_cache_in};
pub use seed::{seed_swift_worktree, seed_swift_worktree_within};
pub use workspace::{find_swift_packages, relocate_swiftpm_workspace_state, tree_size};

#[cfg(test)]
mod tests;
