/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::{prune, tools};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
#[path = "prune_preservation/support.rs"]
mod support;
use support::{MANIFEST, capture, scripted_prune};

#[tokio::test]
async fn attribute_reachable_candidates_never_reach_destructive_planning() {
    let mut transcript = String::new();
    for (case, attribute) in [
        "#[no_mangle]",
        "#[unsafe(no_mangle)]",
        "#[export_name = \"entry\"]",
        "#[unsafe(\n export_name = \"entry\"\n)]",
        "#[ctor::ctor]",
        "#[test_case]",
        "#[some_registration]",
        "#[cfg_attr(feature = \"ffi\", no_mangle)]",
        "#[proc_macro_attribute]",
    ]
    .into_iter()
    .enumerate()
    {
        let source = format!(
            "{attribute}\n// attached across comments and blank lines\n\n#[allow(dead_code)]\nextern \"C\" fn entry() -> u32 {{ 73 }}\n"
        );
        for reachability in [false, true] {
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let (returned, calls, result) =
                    scripted_prune(&source, apply, force, reachability).await;
                capture(&format!("attribute-{case}-original.rs"), &source);
                capture(&format!("attribute-{case}-returned.rs"), &returned);
                transcript.push_str(&format!("case={case} reachability={reachability} apply={apply} force={force}; scripted references=[]; scripted clean diagnostics; safeDelete calls={calls:?}\n{}", result.render()));
                assert!(
                    calls.is_empty(),
                    "{attribute}: safeDelete was called: {calls:?}"
                );
                assert_eq!(returned, source);
                assert!(result.removed.is_empty());
            }
        }
    }
    capture("attribute-cases.txt", &transcript);
    let source = "#[custom_registry]\nmod registered {\nfn entry() {}\n}\n";
    let (_, calls, _) = scripted_prune(source, true, false, false).await;
    assert!(
        calls.is_empty(),
        "an attributed container protects its declarations"
    );
}

#[tokio::test]
async fn ordinary_orphans_and_harmless_attributes_still_prune() {
    for (case, source) in [
        "fn orphan() {}\n",
        "#[allow(dead_code)]\n#[inline(never)]\nfn orphan() {}\n",
        "// #[no_mangle]\nfn orphan() {}\n",
        "#[no_mangle] extern \"C\" fn retained() {}\nfn orphan() {}\n",
        "const TEXT: &str = r#\"#[no_mangle] fn fake() {}\"#;\nfn orphan() {}\n",
        "/* #[registry] /* nested */ fn fake() {} */\nfn orphan() {}\n",
        "macro_rules! unrelated { ($x:tt) => { # $x }; }\nfn orphan() {}\n",
    ]
    .into_iter()
    .enumerate()
    {
        for apply in [false, true] {
            let (returned, calls, result) = scripted_prune(source, apply, false, false).await;
            capture(&format!("orphan-{case}-original.rs"), source);
            if apply {
                capture(&format!("orphan-{case}-returned.rs"), &returned);
            }
            assert_eq!(calls.len(), 1, "{source}: {calls:?}");
            assert!(calls[0].contains("fn orphan"));
            assert_eq!(result.removed[0].name, "orphan");
            assert_eq!(result.applied, apply);
            if source.contains("fn retained") {
                assert!(returned.contains("fn retained"));
            }
            if apply {
                assert!(!returned.contains("fn orphan"));
            } else {
                assert_eq!(returned, source);
            }
        }
    }
}

/// Opt-in live analyzer execution is separate from the deliberately scripted eligibility
/// tests. The evidence run supplies a remote gateway; ordinary offline test runs do not.
#[tokio::test]
async fn live_rust_analyzer_orphan_positive() {
    let Ok(remote) = std::env::var("PRUNE_REAL_REMOTE") else {
        eprintln!("live analyzer not requested; no actual analyzer claim");
        return;
    };
    let source = "#[unsafe(export_name = \"prune_ffi_entry\")]\nextern \"C\" fn registered() -> u32 { 73 }\n\nfn orphan() -> u32 { 5 }\n";
    let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("src/lib.rs", source)]);
    let root = ws.root();
    let remote = remote.parse().unwrap();
    let dry = prune::prune_orphans(remote, &root, 100, false, false)
        .await
        .unwrap();
    capture("live-original.rs", source);
    capture("live-dry-result.txt", &dry.render());
    assert_eq!(
        dry.removed
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>(),
        ["orphan"]
    );
    assert_eq!(ws.read("src/lib.rs"), source);
    let applied = tools::execute_tool(remote, &root, "code_prune_orphans", json!({"apply":true}))
        .await
        .unwrap();
    capture("live-returned.rs", &ws.read("src/lib.rs"));
    capture(
        "live-apply-result.txt",
        &format!("actual analyzer via public MCP dispatch\n{applied:?}"),
    );
    assert!(!applied.is_error);
    assert!(ws.read("src/lib.rs").contains("fn registered"));
    assert!(!ws.read("src/lib.rs").contains("fn orphan"));
    let compile = std::process::Command::new("rustc")
        .args(["--edition=2021", "--crate-type=cdylib"])
        .arg(root.join("src/lib.rs"))
        .arg("-o")
        .arg(root.join("live.so"))
        .output()
        .unwrap();
    capture(
        "live-compiler.txt",
        &format!(
            "actual rustc exit={}\n{}",
            compile.status,
            String::from_utf8_lossy(&compile.stderr)
        ),
    );
    assert!(compile.status.success());
    let nm = std::process::Command::new("nm")
        .args(["-D", "--defined-only"])
        .arg(root.join("live.so"))
        .output()
        .unwrap();
    capture(
        "live-export-symbols.txt",
        &format!(
            "actual nm exit={}\n{}",
            nm.status,
            String::from_utf8_lossy(&nm.stdout)
        ),
    );
    assert!(nm.status.success());
    assert!(String::from_utf8_lossy(&nm.stdout).contains("prune_ffi_entry"));
    let runtime = std::process::Command::new("python3").args(["-c", "import ctypes,sys; f=ctypes.CDLL(sys.argv[1]).prune_ffi_entry; f.restype=ctypes.c_uint32; assert f()==73; print('ABI return=73')"]).arg(root.join("live.so")).output().unwrap();
    capture(
        "live-runtime.txt",
        &format!(
            "actual runtime exit={}\n{}{}",
            runtime.status,
            String::from_utf8_lossy(&runtime.stdout),
            String::from_utf8_lossy(&runtime.stderr)
        ),
    );
    assert!(runtime.status.success());
}

#[tokio::test]
async fn managed_and_mixed_scopes_fail_before_any_query_or_write() {
    for (manifest, content, file, source) in [
        (
            "go.mod",
            "module probe\n\ngo 1.22\n",
            "main.go",
            "package probe\nfunc orphan() {}\n",
        ),
        (
            "pyproject.toml",
            "[project]\nname='probe'\nversion='0.1'\n",
            "main.py",
            "def orphan(): pass\n",
        ),
        ("package.json", "{}", "index.ts", "function orphan() {}\n"),
        ("Cargo.toml", MANIFEST, "src/lib.rs", "fn orphan() {}\n"),
    ] {
        let ws = Workspace::new(&[(manifest, content), (file, source)]);
        if manifest == "Cargo.toml" {
            std::fs::write(ws.root().join("helper.py"), "def registered(): pass\n").unwrap();
        }
        let gateway = ScriptedGateway::start(|method, _| match method {
            "textDocument/documentSymbol" => {
                json!([answers::document_symbol("orphan", 12, 2, 2, 6)])
            }
            "textDocument/references" => json!([]),
            "prodCode/safeDelete" => answers::rpc_error(-32601, "method not found"),
            _ => Value::Null,
        })
        .await;
        for apply in [false, true] {
            let result = tools::execute_tool(
                gateway.addr(),
                &ws.root(),
                "code_prune_orphans",
                json!({"apply":apply,"force":true}),
            )
            .await;
            capture(
                "managed-result.txt",
                &format!(
                    "scripted managed engine; safeDelete replies method-not-found\n{result:?}"
                ),
            );
            let err = result.expect_err("Rust-only boundary");
            assert!(format!("{err:#}").contains("Rust-only"), "{err:#}");
            assert_eq!(ws.read(file), source);
        }
        assert_eq!(gateway.calls(), 0);
    }
}

#[tokio::test]
async fn a_non_rust_deletion_reply_cannot_partially_apply() {
    let source = "fn orphan() {}\n";
    let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("src/lib.rs", source)]);
    let root = ws.root();
    let rust_path = root.join("src/lib.rs");
    let other_path = root.join("unrequested.py");
    let other = other_path.clone();
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => json!([answers::document_symbol("orphan", 12, 1, 1, 4)]),
        "textDocument/references" => json!([]),
        "prodCode/safeDelete" => json!({"changes": {
            answers::uri(&rust_path): [{"range":{"start":{"line":0,"character":0},"end":{"line":1,"character":0}},"newText":""}],
            answers::uri(&other): [{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"pass\n"}]
        }}),
        _ => Value::Null,
    }).await;
    let err = prune::prune_orphans(gateway.addr(), &root, 100, true, true)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("Rust-only"));
    assert_eq!(ws.read("src/lib.rs"), source);
    assert!(!other_path.exists());
}
