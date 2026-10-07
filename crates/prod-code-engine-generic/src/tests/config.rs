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

#[test]
fn workspace_file_uri_escapes_reserved_path_characters() {
    let root = std::env::temp_dir().join("workspace space #1%25");
    let uri = crate::lsp::workspace_file_uri(&root).unwrap();

    assert!(!uri.contains(' '));
    assert!(uri.contains("%23"));
    assert_eq!(url::Url::parse(&uri).unwrap().to_file_path().unwrap(), root);
}

#[test]
fn workspace_file_uri_resolves_relative_roots() {
    let relative_root = std::path::Path::new("./missing/..");
    let normalized = crate::lsp::normalize_workspace_root(relative_root).unwrap();
    let uri = crate::lsp::workspace_file_uri(relative_root)
        .expect("relative workspace roots should be accepted");

    assert_eq!(normalized, std::env::current_dir().unwrap());
    assert_eq!(
        url::Url::parse(&uri).unwrap().to_file_path().unwrap(),
        normalized
    );
}
