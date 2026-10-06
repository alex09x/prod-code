/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cross-worktree Python engine support: shared virtual-environment stub cache,
//! automatic stub indexing, and type checker coordination for basedpyright/mypy (Roadmap 3.6).

pub mod detect;
pub mod env;
pub mod fingerprint;
pub mod fs_ops;
pub mod merge;
pub mod prune;
pub mod seed;

pub use detect::{find_venv_stubs, is_python_project};
pub(crate) use env::is_shared_stub_cache_link;
pub use env::{
    PYTHON_STUB_CACHE_ENV, ensure_cache_dir, python_stub_cache_dir, python_stub_cache_env,
    python_stub_cache_env_for_workspace,
};
pub use merge::{merge_stubs, tree_size};
pub use prune::{
    TMP_STUB_GRACE_PERIOD, parse_tmp_stub_timestamp, prune_stale_stub_cache,
    prune_stale_stub_cache_in, prune_stale_stub_cache_with_grace,
};
pub use seed::{seed_python_worktree, seed_python_worktree_within};

#[cfg(test)]
mod tests;
