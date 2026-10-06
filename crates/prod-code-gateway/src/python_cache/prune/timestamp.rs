/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::time::{Duration, SystemTime};

/// Extracts the creation timestamp from a temporary stub filename formatted as `.tmp-stub-{pid}-{nonce}-{ts:x}`.
pub fn parse_tmp_stub_timestamp(name: &str) -> Option<SystemTime> {
    let parts: Vec<&str> = name.split('-').collect();
    if parts.len() == 5 && parts[0] == ".tmp" && parts[1] == "stub" {
        let _pid: u32 = parts[2].parse().ok()?;
        let _nonce: u64 = parts[3].parse().ok()?;
        let ts_str = parts[4];
        if let Ok(nanos) = u128::from_str_radix(ts_str, 16) {
            // Must be a reasonable epoch timestamp (after year 2020: ~1.57e18 nanos)
            const MIN_VALID_NANOS: u128 = 1_500_000_000_000_000_000;
            if nanos >= MIN_VALID_NANOS {
                let secs = u64::try_from(nanos / 1_000_000_000).ok()?;
                let subsec = (nanos % 1_000_000_000) as u32;
                return SystemTime::UNIX_EPOCH.checked_add(Duration::new(secs, subsec));
            }
        }
    }
    None
}

/// Grace period for temporary stub files created during atomic copy.
/// Any temporary stub file older than this threshold is considered abandoned by a crashed process
/// and safely unlinked during pruning.
pub const TMP_STUB_GRACE_PERIOD: Duration = Duration::from_secs(3600);
