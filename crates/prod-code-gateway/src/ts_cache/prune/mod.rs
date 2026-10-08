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
#[cfg(any(not(unix), test))]
use std::time::SystemTime;

use super::env::ts_types_cache_dir;

#[cfg(not(unix))]
mod fallback;
pub mod timestamp;
#[cfg(unix)]
mod unix;

pub use timestamp::{TMP_TS_GRACE_PERIOD, parse_tmp_ts_timestamp};

#[cfg(any(not(unix), test))]
pub(crate) fn non_unix_retention_time(
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
) -> SystemTime {
    modified.or(created).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Evicts stale type files from the shared TypeScript types cache based on age and max capacity.
/// Uses secure directory-handle-relative traversal and O_NOFOLLOW to eliminate symlink TOCTOU.
pub fn prune_stale_types_cache(max_age: Duration, max_size_bytes: u64) -> io::Result<usize> {
    let cache_dir = ts_types_cache_dir();
    prune_stale_types_cache_in(&cache_dir, max_age, max_size_bytes)
}

/// Prunes stale type cache files within the specified directory using the default 1-hour grace period for temporary files.
pub fn prune_stale_types_cache_in(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
) -> io::Result<usize> {
    prune_stale_types_cache_with_grace(cache_dir, max_age, max_size_bytes, TMP_TS_GRACE_PERIOD)
}

/// Prunes stale type cache files with a configurable grace period for abandoned temporary files.
pub fn prune_stale_types_cache_with_grace(
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

#[cfg(test)]
mod tests {
    use super::non_unix_retention_time;
    use std::time::{Duration, SystemTime};

    #[test]
    fn test_new_cache_entry_fallback_prefers_publication_mtime() {
        let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        let published = SystemTime::UNIX_EPOCH + Duration::from_secs(10);

        assert_eq!(
            non_unix_retention_time(Some(created), Some(published)),
            published
        );
        assert_eq!(non_unix_retention_time(Some(created), None), created);
        assert_eq!(non_unix_retention_time(None, None), SystemTime::UNIX_EPOCH);
    }
}
