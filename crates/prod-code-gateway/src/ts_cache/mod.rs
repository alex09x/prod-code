/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cross-worktree TypeScript & JavaScript engine support: shared global `@types/*`
//! and declaration cache, automated worktree type resolution, and vtsls/tsc coordination (Roadmap 3.5).

pub mod detect;
pub mod env;
pub mod fs_ops;
pub mod merge;
pub mod prune;
pub mod roots;
pub mod seed;

pub use detect::{
    find_project_types, find_project_types_within, has_declaration_files,
    has_declaration_files_within, is_typescript_project,
};
pub use env::{TS_TYPES_CACHE_ENV, ensure_cache_dir, ts_types_cache_dir, ts_types_cache_env};
pub use merge::{merge_types, merge_types_within, tree_size, tree_size_within};
pub use prune::{
    TMP_TS_GRACE_PERIOD, parse_tmp_ts_timestamp, prune_stale_types_cache,
    prune_stale_types_cache_in, prune_stale_types_cache_with_grace,
};
pub use roots::{
    approved_target, build_approved_roots, find_enclosing_project_root, is_target_approved,
};
pub use seed::{seed_typescript_worktree, seed_typescript_worktree_within};

#[cfg(test)]
pub(crate) use seed::coordinate_tsconfig;

#[cfg(test)]
mod tests;
