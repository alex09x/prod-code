/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::engine::RustEngine;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const LIB_SOURCE: &str = "#[cfg_attr(feature = \"shared\", path = \"../../cfg_shared.rs\")]\n\
mod cfg_shared;\n\
#[ path = \"../../spaced_shared.rs\" ]\n\
mod spaced_shared;\n\
pub fn call_cfg_shared() { cfg_shared::external_cfg_function(); }\n\
pub fn call_spaced_shared() { spaced_shared::external_spaced_function(); }\n";

struct WorkspacePaths {
    lib: PathBuf,
    cfg_shared: PathBuf,
    spaced_shared: PathBuf,
}

fn paths_at(root: &Path) -> WorkspacePaths {
    WorkspacePaths {
        lib: root.join("consumer/src/lib.rs"),
        cfg_shared: root.join("cfg_shared.rs"),
        spaced_shared: root.join("spaced_shared.rs"),
    }
}

fn write_workspace(root: &Path, version: &str) -> WorkspacePaths {
    let paths = paths_at(root);
    std::fs::create_dir_all(root.join("consumer/src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"consumer\"]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("prod-code.toml"),
        "[rust]\nfeatures = \"all\"\nproc_macro_srv = \"disabled\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("consumer/Cargo.toml"),
        format!(
            "[package]\nname = \"consumer\"\nversion = \"{version}\"\nedition = \"2021\"\n\n[features]\ndefault = [\"shared\"]\nshared = []\n"
        ),
    )
    .unwrap();
    std::fs::write(&paths.lib, LIB_SOURCE).unwrap();
    std::fs::write(&paths.cfg_shared, "pub fn external_cfg_function() {}\n").unwrap();
    std::fs::write(
        &paths.spaced_shared,
        "pub fn external_spaced_function() {}\n",
    )
    .unwrap();
    paths
}

#[cfg(unix)]
fn symlinked_root(temp: &TempDir, actual: &Path) -> PathBuf {
    let alias = temp.path().join("workspace-link");
    std::os::unix::fs::symlink(actual, &alias).unwrap();
    alias
}

#[cfg(not(unix))]
fn symlinked_root(_temp: &TempDir, actual: &Path) -> PathBuf {
    actual.to_path_buf()
}

fn one_based_column(line: &str, needle: &str) -> u32 {
    line.find(needle).expect("symbol in fixture source") as u32 + 1
}

fn same_file(left: &Path, right: &Path) -> bool {
    std::fs::canonicalize(left).ok() == std::fs::canonicalize(right).ok()
}

#[test]
fn path_attributes_support_symlinked_roots_cfg_attr_and_whitespace() {
    let temp = tempfile::tempdir().unwrap();
    let actual_root = temp.path().join("workspace");
    write_workspace(&actual_root, "0.1.0");
    let workspace_root = symlinked_root(&temp, &actual_root);
    let paths = paths_at(&workspace_root);
    let engine = RustEngine::load(&workspace_root).expect("load the feature-enabled workspace");

    for (function, target) in [
        ("external_cfg_function", &paths.cfg_shared),
        ("external_spaced_function", &paths.spaced_shared),
    ] {
        let symbol = engine
            .workspace_symbols(function, 10)
            .expect("search workspace symbols")
            .into_iter()
            .find(|symbol| symbol.name == function)
            .unwrap_or_else(|| panic!("workspace symbol {function} was not returned"));
        assert!(engine.file_id_for_path(&symbol.path).is_some());
        assert!(
            engine
                .document_symbols(&symbol.path)
                .unwrap()
                .iter()
                .any(|candidate| candidate.name == function)
        );
        let call_name = if function == "external_cfg_function" {
            "call_cfg_shared"
        } else {
            "call_spaced_shared"
        };
        let call = engine
            .workspace_symbols(call_name, 10)
            .expect("find the consumer function")
            .into_iter()
            .find(|candidate| candidate.name == call_name)
            .unwrap_or_else(|| panic!("workspace symbol {call_name} was not returned"));
        let line = if function == "external_cfg_function" {
            LIB_SOURCE.lines().nth(4).unwrap()
        } else {
            LIB_SOURCE.lines().nth(5).unwrap()
        };
        let definitions = engine
            .goto_definition(&call.path, call.line, one_based_column(line, function))
            .expect("resolve the external function from its consumer");
        assert!(
            definitions.iter().any(
                |definition| definition.name == function && same_file(&definition.path, target)
            ),
            "expected {function} in {}, got {definitions:?}",
            target.display()
        );
    }
}

#[test]
fn worktree_manifest_reload_keeps_external_path_modules() {
    let temp = tempfile::tempdir().unwrap();
    let base_root = temp.path().join("base");
    let copy_root = temp.path().join("worktree");
    write_workspace(&base_root, "0.1.0");
    let copy = write_workspace(&copy_root, "0.2.0");
    let mut engine = RustEngine::load(&base_root).expect("load the base workspace");

    engine
        .reload_worktree(&copy_root)
        .expect("load the worktree with different manifests");
    let symbols = engine
        .document_symbols(&copy.cfg_shared)
        .expect("external path module should be outlined in the worktree");
    assert!(
        symbols
            .iter()
            .any(|symbol| symbol.name == "external_cfg_function"),
        "{symbols:?}"
    );
    let definitions = engine
        .goto_definition(
            &copy.lib,
            5,
            one_based_column(LIB_SOURCE.lines().nth(4).unwrap(), "external_cfg_function"),
        )
        .expect("resolve the external path module in the worktree");
    assert!(
        definitions.iter().any(|definition| {
            definition.name == "external_cfg_function"
                && same_file(&definition.path, &copy.cfg_shared)
        }),
        "{definitions:?}"
    );
}
