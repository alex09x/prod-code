/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::config::GenericLspConfig;
use crate::config::discovery::which_bin;

#[test]
fn test_generic_config_defaults() {
    let py_config = GenericLspConfig::for_python();
    assert!(!py_config.command.is_empty());

    let ts_config = GenericLspConfig::for_typescript();
    assert!(!ts_config.command.is_empty());
}

#[test]
fn test_which_bin_discovery() {
    assert!(which_bin("cargo").is_ok());
    assert!(which_bin("nonexistent_binary_xyz_123").is_err());
}
