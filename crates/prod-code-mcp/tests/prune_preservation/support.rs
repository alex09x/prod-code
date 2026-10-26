/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::prune;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub(super) const MANIFEST: &str =
    "[package]\nname = \"prune_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

pub(super) fn capture(name: &str, text: &str) {
    // Evidence is opt-in; ordinary test runs never rewrite tracked artifacts.
    if let Ok(dir) = std::env::var("PRUNE_EVIDENCE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join(name), text).unwrap();
    }
}

pub(super) async fn scripted_prune(
    source: &str,
    apply: bool,
    force: bool,
    reachability: bool,
) -> (String, Vec<String>, prune::Pruned) {
    let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("src/lib.rs", source)]);
    let root = ws.root();
    let path = root.join("src/lib.rs");
    let original = source.to_owned();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => Value::Array(
            original
                .lines()
                .enumerate()
                .filter_map(|(i, line)| {
                    if line.starts_with("//")
                        || line.starts_with("const ")
                        || line.starts_with("/*")
                    {
                        return None;
                    }
                    let start = line.find("fn ")? + 3;
                    let name = line[start..].split('(').next()?;
                    Some(answers::document_symbol(
                        name,
                        12,
                        i as u32 + 1,
                        i as u32 + 1,
                        start as u32 + 1,
                    ))
                })
                .collect(),
        ),
        "textDocument/references" => json!([]),
        "prodCode/safeDelete" => {
            let line = params.pointer("/position/line").unwrap().as_u64().unwrap() as usize;
            let selected = original.lines().nth(line).unwrap();
            recorded.lock().unwrap().push(selected.to_owned());
            // Deliberately permissive scripted analyzer: eligibility must protect registrations.
            let lines = original.lines().collect::<Vec<_>>();
            let mut first = line;
            while first > 0
                && lines[first - 1].trim().starts_with("#[")
                && lines[first - 1].trim().ends_with(']')
            {
                first -= 1;
            }
            let returned = lines
                .iter()
                .enumerate()
                .filter(|(i, _)| !(first..=line).contains(i))
                .map(|(_, l)| format!("{l}\n"))
                .collect::<String>();
            answers::whole_file(&path, &original, &returned)
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let result = prune::prune_orphans_opts(
        gateway.addr(),
        &root,
        prod_code_mcp::dead_code::DeadCodeOptions {
            include_exported: true,
            max_files: 100,
            reachability,
        },
        apply,
        force,
        false,
        false,
    )
    .await
    .unwrap();
    let final_source = ws.read("src/lib.rs");
    let calls = calls.lock().unwrap().clone();
    (final_source, calls, result)
}

#[test]
fn actual_compiler_export_and_runtime_control() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let source = "#[unsafe(export_name = \"prune_ffi_entry\")]\nextern \"C\" fn registered() -> u32 { 73 }\n";
    let path = dir.path().join("control.rs");
    let library = dir.path().join("control.so");
    let mut transcript = String::new();
    for (label, text, expected) in [("original", source, true), ("deleted", "", false)] {
        std::fs::write(&path, text).unwrap();
        let compile = Command::new("rustc")
            .args(["--edition=2024", "--crate-type=cdylib"])
            .arg(&path)
            .arg("-o")
            .arg(&library)
            .output()
            .unwrap();
        transcript.push_str(&format!(
            "{label}: rustc exit={}\n{}",
            compile.status,
            String::from_utf8_lossy(&compile.stderr)
        ));
        assert!(compile.status.success(), "{transcript}");
        let nm = Command::new("nm")
            .args(["-D", "--defined-only"])
            .arg(&library)
            .output()
            .unwrap();
        transcript.push_str(&format!(
            "nm exit={}\n{}",
            nm.status,
            String::from_utf8_lossy(&nm.stdout)
        ));
        assert!(nm.status.success());
        assert_eq!(
            String::from_utf8_lossy(&nm.stdout).contains("prune_ffi_entry"),
            expected
        );
        let script = if expected {
            "import ctypes,sys; f=ctypes.CDLL(sys.argv[1]).prune_ffi_entry; f.restype=ctypes.c_uint32; assert f()==73; print('ABI return=73')"
        } else {
            "import ctypes,sys; lib=ctypes.CDLL(sys.argv[1]); assert not hasattr(lib,'prune_ffi_entry'); print('ABI symbol absent despite clean compilation')"
        };
        let runtime = Command::new("python3")
            .args(["-c", script])
            .arg(&library)
            .output()
            .unwrap();
        transcript.push_str(&format!(
            "runtime exit={}\n{}{}",
            runtime.status,
            String::from_utf8_lossy(&runtime.stdout),
            String::from_utf8_lossy(&runtime.stderr)
        ));
        assert!(runtime.status.success(), "{transcript}");
    }
    capture("ffi-original.rs", source);
    capture("ffi-deleted.rs", "");
    capture("ffi-compiler-runtime.txt", &transcript);
}
