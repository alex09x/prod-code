/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use serde_json::json;
use std::cell::Cell;

fn only(bins: &'static [&'static str]) -> impl Fn(&str) -> bool {
    move |name| bins.contains(&name)
}

fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
    move |k| {
        pairs
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| OsString::from(*v))
    }
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn no_root() -> Option<PathBuf> {
    None
}

/// A global npm root with a native platform compiler and/or a tsserver.js.
fn global_root(native: bool, tsserver: bool) -> (tempfile::TempDir, Option<PathBuf>) {
    let dir = tempfile::tempdir().unwrap();
    let ts = dir.path().join("typescript");
    if tsserver {
        std::fs::create_dir_all(ts.join("lib")).unwrap();
        std::fs::write(ts.join("lib").join("tsserver.js"), "").unwrap();
    }
    let mut tsc = None;
    if native {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => other,
        };
        let lib = ts
            .join("node_modules")
            .join("@typescript")
            .join(format!("typescript-{os}-{arch}"))
            .join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("tsc"), "").unwrap();
        tsc = Some(lib.join("tsc"));
    }
    (dir, tsc)
}

/// The production path never resolves the npm root when `tsgo` answers (#1794).
#[test]
fn production_typescript_path_skips_npm_when_tsgo_is_found() {
    let tsgo = PathBuf::from("/opt/tsgo");
    let calls = Cell::new(0);
    let npm = || {
        calls.set(calls.get() + 1);
        None
    };
    let c = GenericLspConfig::typescript_resolved(
        || Some(tsgo.clone()),
        npm,
        only(&["vtsls"]),
        env_of(&[]),
    );
    assert_eq!(c.command, "/opt/tsgo");
    assert_eq!(c.initialization_options, None);
    assert_eq!(
        calls.get(),
        0,
        "npm root must not be resolved when tsgo is found"
    );

    let command = GenericLspConfig::typescript_command_with(
        || Some(tsgo.clone()),
        || {
            calls.set(calls.get() + 1);
            None
        },
        only(&["vtsls"]),
    );
    assert_eq!(command, "/opt/tsgo");
    assert_eq!(
        calls.get(),
        0,
        "installed check must not resolve the npm root"
    );
}

/// Without `tsgo`, one npm root answer serves both the native lookup and tsserver.
#[test]
fn production_typescript_path_resolves_npm_root_once() {
    let (dir, _) = global_root(false, true);
    let calls = Cell::new(0);
    let npm = || {
        calls.set(calls.get() + 1);
        Some(dir.path().to_path_buf())
    };
    let c = GenericLspConfig::typescript_resolved(no_root, npm, only(&["vtsls"]), env_of(&[]));
    assert_eq!(c.command, "vtsls");
    let lib = dir.path().join("typescript").join("lib");
    let opts = c.initialization_options.unwrap();
    assert_eq!(
        opts["typescript"]["tsserver"]["path"],
        json!(lib.to_string_lossy())
    );
    assert_eq!(calls.get(), 1);

    let (native_dir, tsc) = global_root(true, true);
    let calls = Cell::new(0);
    let npm = || {
        calls.set(calls.get() + 1);
        Some(native_dir.path().to_path_buf())
    };
    let c = GenericLspConfig::typescript_resolved(no_root, npm, only(&["vtsls"]), env_of(&[]));
    assert_eq!(Some(PathBuf::from(&c.command)), tsc);
    assert!(matches!(c.ready, ReadySignal::HoldsQuestions));
    assert_eq!(c.initialization_options, None);
    assert_eq!(calls.get(), 1);

    let calls = Cell::new(0);
    let command = GenericLspConfig::typescript_command_with(
        no_root,
        || {
            calls.set(calls.get() + 1);
            Some(dir.path().to_path_buf())
        },
        only(&["vtsls"]),
    );
    assert_eq!(command, "vtsls");
    assert_eq!(calls.get(), 1);
}

#[test]
fn typescript_prefers_native_then_vtsls_then_typescript_language_server() {
    let native = PathBuf::from("/opt/tsgo");
    let c = GenericLspConfig::typescript_with(Some(native), no_root, only(&["vtsls"]), env_of(&[]));
    assert_eq!(c.command, "/opt/tsgo");
    assert_eq!(c.args, strs(&["--lsp", "--stdio"]));
    assert!(matches!(c.ready, ReadySignal::HoldsQuestions));
    assert_eq!(c.initialization_options, None);

    let c = GenericLspConfig::typescript_with(None, no_root, only(&["vtsls"]), env_of(&[]));
    assert_eq!((c.command.as_str(), c.args), ("vtsls", strs(&["--stdio"])));
    assert!(matches!(c.ready, ReadySignal::Progress));
    assert_eq!(
        c.initialization_options,
        Some(json!({
            "vtsls": {"autoUseWorkspaceTsdk": true,
                      "experimental": {"completion": {"enableServerSideFuzzyMatch": true}}},
            "typescript": {"preferences": {"includeInlayParameterNameHints": "none"},
                           "tsserver": {"maxTsServerMemory": 4096}}
        }))
    );

    for bins in [&["typescript-language-server"][..], &[][..]] {
        let c = GenericLspConfig::typescript_with(None, no_root, only(bins), env_of(&[]));
        assert_eq!(c.command, "typescript-language-server");
        assert_eq!(c.args, strs(&["--stdio"]));
        assert!(matches!(c.ready, ReadySignal::Unknown));
        assert_eq!(c.initialization_options, None);
    }
}

#[test]
fn typescript_points_tsserver_at_the_global_typescript_install() {
    let (dir, _) = global_root(false, true);
    let lib_str = dir
        .path()
        .join("typescript")
        .join("lib")
        .to_string_lossy()
        .into_owned();
    let root = || Some(dir.path().to_path_buf());

    let c = GenericLspConfig::typescript_with(None, root, only(&[]), env_of(&[]));
    assert_eq!(
        c.initialization_options,
        Some(json!({
            "tsserver": {"path": lib_str},
            "preferences": {"includeInlayParameterNameHints": "none"}
        }))
    );
    let c = GenericLspConfig::typescript_with(None, root, only(&["vtsls"]), env_of(&[]));
    let opts = c.initialization_options.unwrap();
    assert_eq!(opts["typescript"]["tsserver"]["path"], json!(lib_str));
    assert_eq!(
        opts["typescript"]["tsserver"]["maxTsServerMemory"],
        json!(4096)
    );

    // A global root without a tsserver.js adds nothing.
    let empty = tempfile::tempdir().unwrap();
    let c = GenericLspConfig::typescript_with(
        None,
        || Some(empty.path().to_path_buf()),
        only(&[]),
        env_of(&[]),
    );
    assert_eq!(c.initialization_options, None);
}

#[test]
fn typescript_type_cache_extends_node_path() {
    let c = GenericLspConfig::typescript_with(
        None,
        no_root,
        only(&[]),
        env_of(&[("PROD_CODE_TS_TYPES_CACHE", "/types"), ("NODE_PATH", "/nm")]),
    );
    assert_eq!(
        c.env.get("PROD_CODE_TS_TYPES_CACHE").map(String::as_str),
        Some("/types")
    );
    assert_eq!(
        c.env.get("NODE_PATH").map(String::as_str),
        Some("/nm:/types")
    );
    let c = GenericLspConfig::typescript_with(
        None,
        no_root,
        only(&[]),
        env_of(&[("PROD_CODE_TS_TYPES_CACHE", "/types")]),
    );
    assert_eq!(c.env.get("NODE_PATH").map(String::as_str), Some("/types"));
    let c = GenericLspConfig::typescript_with(
        None,
        no_root,
        only(&[]),
        env_of(&[("NODE_PATH", "/nm")]),
    );
    assert!(c.env.is_empty());
}
