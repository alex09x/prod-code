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

/// Grace period for temporary files created during atomic copy (1 hour).
pub const TMP_TS_GRACE_PERIOD: Duration = Duration::from_secs(3600);

/// Parses the timestamp from a `.tmp-ts-{pid}-{nonce}-{ts:x}` filename.
/// Returns `None` if the name does not match the expected format or if the timestamp
/// is prior to the year 2020.
pub fn parse_tmp_ts_timestamp(name: &str) -> Option<SystemTime> {
    let parts: Vec<&str> = name.split('-').collect();
    if parts.len() == 5 && parts[0] == ".tmp" && parts[1] == "ts" {
        let _pid: u32 = parts[2].parse().ok()?;
        let _nonce: u64 = parts[3].parse().ok()?;
        let ts_str = parts[4];
        if let Ok(nanos) = u128::from_str_radix(ts_str, 16) {
            // Must be a reasonable epoch timestamp (after year 2020: ~1.57e18 nanos)
            const MIN_VALID_NANOS: u128 = 1_500_000_000_000_000_000;
            if nanos >= MIN_VALID_NANOS {
                let secs = (nanos / 1_000_000_000) as u64;
                let subsec = (nanos % 1_000_000_000) as u32;
                return Some(SystemTime::UNIX_EPOCH + Duration::new(secs, subsec));
            }
        }
    }
    None
}
