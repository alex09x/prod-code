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
use std::time::Duration;

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

#[test]
fn python_picks_the_first_available_server_in_priority_order() {
    let env = env_of(&[]);
    let cases: [(&'static [&'static str], &str, &[&str]); 5] = [
        (
            &["basedpyright-langserver", "ruff"],
            "basedpyright-langserver",
            &["--stdio"],
        ),
        (
            &["pyright-langserver", "ruff"],
            "pyright-langserver",
            &["--stdio"],
        ),
        (&["ruff"], "ruff", &["server"]),
        (&[], "pylsp", &[]),
        (&["pylsp"], "pylsp", &[]),
    ];
    for (bins, cmd, args) in cases {
        let c = GenericLspConfig::python_with(only(bins), &env);
        assert_eq!(c.command, cmd, "{bins:?}");
        assert_eq!(c.args, strs(args), "{bins:?}");
        let pyright = cmd.contains("pyright");
        assert_eq!(c.retain_open_documents, pyright);
        assert!(matches!(c.ready, ReadySignal::Log(_)) == pyright);
        assert!(matches!(c.ready, ReadySignal::Unknown) != pyright);
        assert_eq!(
            c.initialization_options,
            pyright.then(|| json!({"python": {"analysis": {"stubPath": "typings"}}}))
        );
        assert!(c.env.is_empty());
        assert_eq!(c.request_timeout, DEFAULT_REQUEST_TIMEOUT);
        assert_eq!(c.health_probe_interval, Some(DEFAULT_HEALTH_PROBE_INTERVAL));
        assert_eq!(c.max_retained_documents, DEFAULT_MAX_RETAINED_DOCUMENTS);
    }
}

#[test]
fn python_forwards_stub_and_path_variables_to_the_server() {
    let env = env_of(&[
        ("PROD_CODE_PYTHON_STUB_CACHE", "/cache/stubs"),
        ("MYPYPATH", "/m"),
        ("TYPINGS_PATH", "/t"),
    ]);
    let c = GenericLspConfig::python_with(only(&["pyright-langserver"]), env);
    let mut want = HashMap::new();
    want.insert(
        "PROD_CODE_PYTHON_STUB_CACHE".to_string(),
        "/cache/stubs".to_string(),
    );
    want.insert("MYPYPATH".to_string(), "/m".to_string());
    want.insert("TYPINGS_PATH".to_string(), "/t".to_string());
    assert_eq!(c.env, want);
    assert_eq!(
        c.initialization_options,
        Some(json!({"python": {"analysis": {"stubPath": "/cache/stubs"}}}))
    );
}

#[test]
fn cpp_has_a_background_index_and_validation_drops_it() {
    let c = GenericLspConfig::for_cpp();
    assert_eq!(c.command, "clangd");
    assert_eq!(
        c.args,
        strs(&[
            "--background-index",
            "--header-insertion=never",
            "--use-dirty-headers",
            "--log=error",
            "--compile-commands-dir=build",
        ])
    );
    assert!(matches!(c.ready, ReadySignal::Progress));
    assert_eq!(c.index_wait, INDEX_WAIT);
    let v = GenericLspConfig::for_cpp_validation();
    assert_eq!(v.command, "clangd");
    assert_eq!(
        v.args,
        strs(&[
            "--header-insertion=never",
            "--use-dirty-headers",
            "--log=error",
            "--compile-commands-dir=build",
            "--background-index=false",
        ])
    );
    assert!(matches!(v.ready, ReadySignal::HoldsQuestions));
}

#[test]
fn swift_falls_back_to_xcrun_only_on_macos() {
    let cases = [
        (&["sourcekit-lsp"][..], true, "sourcekit-lsp", vec![]),
        (&["sourcekit-lsp"][..], false, "sourcekit-lsp", vec![]),
        (&[][..], true, "xcrun", strs(&["sourcekit-lsp"])),
        (&[][..], false, "sourcekit-lsp", vec![]),
    ];
    for (bins, macos, cmd, args) in cases {
        let c = GenericLspConfig::swift_with(only(bins), env_of(&[]), macos);
        assert_eq!(
            (c.command.as_str(), c.args),
            (cmd, args),
            "{bins:?} {macos}"
        );
        assert!(c.env.is_empty());
        assert!(matches!(c.ready, ReadySignal::Progress));
        assert_eq!(c.index_wait, Duration::from_secs(4));
    }
    let c = GenericLspConfig::swift_with(
        only(&[]),
        env_of(&[
            ("SWIFTPM_MODULECACHE_OVERRIDE", "/a"),
            ("SWIFT_MODULE_CACHE_PATH", "/b"),
            ("CLANG_MODULE_CACHE_PATH", "/c"),
        ]),
        false,
    );
    let mut env: Vec<_> = c.env.into_iter().collect();
    env.sort();
    assert_eq!(
        env,
        vec![
            ("CLANG_MODULE_CACHE_PATH".to_string(), "/c".to_string()),
            ("SWIFTPM_MODULECACHE_OVERRIDE".to_string(), "/a".to_string()),
            ("SWIFT_MODULE_CACHE_PATH".to_string(), "/b".to_string()),
        ]
    );
}

#[test]
fn java_kotlin_and_csharp_servers() {
    for (bins, cmd) in [
        (&["jdtls", "java-language-server"][..], "jdtls"),
        (&["java-language-server"][..], "java-language-server"),
        (&[][..], "jdtls"),
    ] {
        let c = GenericLspConfig::java_with(only(bins));
        assert_eq!(c.command, cmd);
        assert!(c.args.is_empty());
        assert!(matches!(c.ready, ReadySignal::Progress));
        assert_eq!(
            c.initialization_options,
            Some(json!({
                "settings": {"java": {"autobuild": {"enabled": true}}},
                "extendedClientCapabilities": {"progressReportProvider": true,
                                               "classFileContentsSupport": true}
            }))
        );
    }
    let k = GenericLspConfig::for_kotlin();
    assert_eq!(k.command, "kotlin-language-server");
    assert!(k.args.is_empty() && k.env.is_empty() && k.initialization_options.is_none());
    assert!(matches!(k.ready, ReadySignal::Progress));
    for (bins, cmd, args) in [
        (&["csharp-ls", "omnisharp"][..], "csharp-ls", vec![]),
        (&["omnisharp"][..], "omnisharp", strs(&["-lsp"])),
        (&[][..], "csharp-ls", vec![]),
    ] {
        let c = GenericLspConfig::csharp_with(only(bins));
        assert_eq!((c.command.as_str(), c.args), (cmd, args), "{bins:?}");
        assert!(c.initialization_options.is_none());
        assert!(matches!(c.ready, ReadySignal::Progress));
    }
}
