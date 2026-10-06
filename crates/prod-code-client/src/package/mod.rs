/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Package management, version parity, and integrity verification for prod-code.

mod checksum;
mod ops;
mod types;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use checksum::{compute_sha256, fetch_official_checksums, parse_checksums_file};
pub use ops::{run_package_install, run_package_status, run_package_sync, run_package_verify};
#[allow(unused_imports)]
pub use types::{
    PackageSubcommands, PackageType, detect_package_type, detect_package_type_with_fs,
};
