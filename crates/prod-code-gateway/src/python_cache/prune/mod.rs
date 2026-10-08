/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::io;
use std::path::Path;
use std::time::Duration;

use super::env::python_stub_cache_dir;

#[cfg(not(unix))]
mod fallback;
pub mod timestamp;
#[cfg(unix)]
pub(crate) mod unix;

pub use timestamp::{TMP_STUB_GRACE_PERIOD, parse_tmp_stub_timestamp};

/// Evicts stale stub files from the shared stub cache based on age and max capacity.
/// Uses secure directory-handle-relative traversal and O_NOFOLLOW to eliminate symlink TOCTOU.
pub fn prune_stale_stub_cache(max_age: Duration, max_size_bytes: u64) -> io::Result<usize> {
    let cache_dir = python_stub_cache_dir();
    prune_stale_stub_cache_in(&cache_dir, max_age, max_size_bytes)
}

/// Prunes stale stub cache files within the specified directory using the default 1-hour grace period for temporary files.
pub fn prune_stale_stub_cache_in(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
) -> io::Result<usize> {
    prune_stale_stub_cache_with_grace(cache_dir, max_age, max_size_bytes, TMP_STUB_GRACE_PERIOD)
}

/// Prunes stale stub cache files with a configurable grace period for abandoned temporary stub files.
pub fn prune_stale_stub_cache_with_grace(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
    tmp_grace_period: Duration,
) -> io::Result<usize> {
    #[cfg(unix)]
    {
        unix::prune_unix(cache_dir, max_age, max_size_bytes, tmp_grace_period)
    }

    #[cfg(not(unix))]
    {
        fallback::prune_fallback(cache_dir, max_age, max_size_bytes, tmp_grace_period)
    }
}
