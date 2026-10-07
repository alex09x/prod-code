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
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace space #1%25");
    std::fs::create_dir_all(&root).unwrap();
    let uri = crate::lsp::workspace_file_uri(&root).unwrap();

    assert!(!uri.contains(' '));
    assert!(uri.contains("%23"));
    assert_eq!(url::Url::parse(&uri).unwrap().to_file_path().unwrap(), root);
}

#[test]
fn workspace_file_uri_resolves_relative_roots() {
    let relative_root = std::path::Path::new(".");
    let normalized = crate::lsp::normalize_workspace_root(relative_root).unwrap();
    let uri = crate::lsp::workspace_file_uri(relative_root)
        .expect("relative workspace roots should be accepted");

    assert_eq!(normalized, std::fs::canonicalize(".").unwrap());
    assert_eq!(
        url::Url::parse(&uri).unwrap().to_file_path().unwrap(),
        normalized
    );
}

#[cfg(unix)]
#[test]
fn workspace_root_normalization_preserves_symlink_root_identity() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    std::fs::create_dir_all(&target).unwrap();
    let alias = temp.path().join("alias");
    symlink(&target, &alias).unwrap();

    let normalized = crate::lsp::normalize_workspace_root(&alias).unwrap();

    assert_eq!(normalized, alias);
}

#[cfg(unix)]
#[test]
fn workspace_root_normalization_preserves_symlink_parent_semantics() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let target_parent = temp.path().join("actual-parent");
    let target = target_parent.join("target");
    std::fs::create_dir_all(&target).unwrap();
    let alias = temp.path().join("alias");
    symlink(&target, &alias).unwrap();

    let normalized = crate::lsp::normalize_workspace_root(&alias.join(".."))
        .expect("existing symlink parents should resolve");

    assert_eq!(normalized, std::fs::canonicalize(&target_parent).unwrap());
    assert!(crate::lsp::normalize_workspace_root(&temp.path().join("missing/..")).is_err());
}
