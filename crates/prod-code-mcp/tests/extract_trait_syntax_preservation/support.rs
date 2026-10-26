/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::extract_trait::{Extracted, extract_trait_ext};
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::process::Command;
use std::sync::{Arc, Mutex};

pub const MANIFEST: &str =
    "[package]\nname = \"shape-api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
pub const SENTINEL: &str = "sentinel\r\n";

pub fn baseline_mode() -> bool {
    std::env::var_os("EXTRACT_TRAIT_SYNTAX_BASELINE").is_some()
}

pub fn workspace(files: &[(&str, &str)]) -> Workspace {
    let mut all = vec![("Cargo.toml", MANIFEST), ("untouched.txt", SENTINEL)];
    all.extend_from_slice(files);
    Workspace::new(&all)
}

pub fn assert_untouched(ws: &Workspace, files: &[(&str, &str)]) {
    assert_eq!(ws.read("Cargo.toml"), MANIFEST, "Cargo.toml modified");
    assert_eq!(ws.read("untouched.txt"), SENTINEL, "untouched.txt modified");
    for (rel, expected) in files {
        assert_eq!(
            fs::read(ws.path(rel)).unwrap(),
            expected.as_bytes(),
            "unexpected modification in {rel}"
        );
    }
}

/// Scripted LSP transport fixture: `references` and `diagnostics` are scripted responses,
/// never claimed as native rust-analyzer proof. Real `rustc` checks run separately.
pub async fn scripted_gateway(refs: Value, diagnostics: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/references" => refs.clone(),
        "textDocument/diagnostic" => diagnostics.clone(),
        _ => Value::Null,
    })
    .await
}

/// Scripted reference fixture combined with a real `rustc` overlay compiler check inside the
/// `textDocument/diagnostic` callback so multi-file visibility/syntax errors (e.g. E0603) block
/// unforced `apply: true` just as a real diagnostic gate does.
pub async fn rustc_diagnostic_gateway(
    initial_files: &[(&str, &str)],
    entry_rel: &str,
    refs: Value,
    cfgs: &[&str],
) -> ScriptedGateway {
    let base: BTreeMap<String, String> = initial_files
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let opened: Arc<Mutex<BTreeMap<String, String>>> = Arc::new(Mutex::new(BTreeMap::new()));
    let entry = entry_rel.to_string();
    let cfg_flags: Vec<String> = cfgs.iter().map(|s| (*s).to_string()).collect();

    ScriptedGateway::start(move |method, params| {
        let mut state = opened.lock().unwrap();
        match method {
            "prod-code/handshake" => {
                state.clear();
                Value::Null
            }
            "textDocument/didOpen" => {
                if let (Some(uri), Some(text)) = (
                    params.pointer("/textDocument/uri").and_then(Value::as_str),
                    params.pointer("/textDocument/text").and_then(Value::as_str),
                ) {
                    state.insert(uri.to_string(), text.to_string());
                }
                Value::Null
            }
            "textDocument/didChange" => {
                if let (Some(uri), Some(text)) = (
                    params.pointer("/textDocument/uri").and_then(Value::as_str),
                    params
                        .pointer("/contentChanges/0/text")
                        .and_then(Value::as_str),
                ) {
                    state.insert(uri.to_string(), text.to_string());
                }
                Value::Null
            }
            "textDocument/references" => refs.clone(),
            "textDocument/diagnostic" => {
                let mut merged = base.clone();
                for (uri, text) in state.iter() {
                    for key in merged.keys().cloned().collect::<Vec<_>>() {
                        if uri.ends_with(&key) {
                            merged.insert(key, text.clone());
                        }
                    }
                }
                let pairs: Vec<(&str, &str)> = merged
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                let flag_refs: Vec<&str> = cfg_flags.iter().map(String::as_str).collect();
                let check = compile_lib(&pairs, &entry, &flag_refs);
                if check["exit"].as_i64() == Some(0) {
                    answers::no_diagnostics()
                } else {
                    let stderr = check["stderr"].as_str().unwrap_or("compile error");
                    let first = stderr.lines().next().unwrap_or("compile error");
                    let code = if stderr.contains("E0603") {
                        "E0603"
                    } else if stderr.contains("E0046") {
                        "E0046"
                    } else {
                        "rustc"
                    };
                    answers::error_at(1, 1, code, first)
                }
            }
            _ => Value::Null,
        }
    })
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_public_extract(
    remote: SocketAddr,
    ws: &Workspace,
    rel_path: &str,
    line: u32,
    col: u32,
    methods: &[&str],
    trait_name: &str,
    migrate_callers: bool,
    apply: bool,
    force: bool,
) -> anyhow::Result<Extracted> {
    let method_vec: Vec<String> = methods.iter().map(|m| (*m).to_string()).collect();
    let before = ws.read(rel_path);
    let result = extract_trait_ext(
        remote,
        &ws.root(),
        &ws.path(rel_path),
        line,
        col,
        &method_vec,
        trait_name,
        migrate_callers,
        apply,
        force,
    )
    .await;
    record_evidence(json!({"operation": "extract_trait_ext", "path": rel_path,
    "line": line, "column": col, "methods": methods, "trait_name": trait_name,
    "migrate_callers": migrate_callers, "apply": apply, "force": force,
    "original_source": before, "disk_source_after": ws.read(rel_path),
    "result": match &result {
        Ok(plan) => json!({"rewritten": plan.rewritten.iter().map(|(path, source)|
            json!({"path": std::path::Path::new(path).strip_prefix(ws.root()).unwrap_or(std::path::Path::new(path)), "source": source})).collect::<Vec<_>>(),
            "applied": plan.applied, "diagnostics": plan.diagnostics}),
        Err(error) => json!({"error": format!("{error:#}")}),
    }}));
    result
}

#[allow(clippy::too_many_arguments)]
pub async fn run_mcp_tool_extract(
    remote: SocketAddr,
    ws: &Workspace,
    rel_path: &str,
    line: u32,
    col: u32,
    methods: &[&str],
    trait_name: &str,
    migrate_callers: bool,
    apply: bool,
    force: bool,
) -> anyhow::Result<String> {
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_extract_trait",
        json!({
            "path": rel_path,
            "line": line,
            "character": col,
            "methods": methods,
            "name": trait_name,
            "migrate_callers": migrate_callers,
            "apply": apply,
            "force": force,
        }),
    )
    .await?;
    Ok(format!("{res:?}"))
}

pub fn compile_lib(files: &[(&str, &str)], entry_rel: &str, cfgs: &[&str]) -> Value {
    let dir = tempfile::tempdir().unwrap();
    for (rel, content) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let mut cmd = Command::new(rustc);
    cmd.current_dir(dir.path()).args([
        "--edition=2021",
        "--crate-type=lib",
        "-A",
        "warnings",
        "--error-format=short",
    ]);
    for cfg in cfgs {
        cmd.args(["--cfg", cfg]);
    }
    cmd.arg(entry_rel)
        .arg("-o")
        .arg(dir.path().join("libprobe.rlib"));
    let out = cmd.output().unwrap();
    let root_str = dir.path().to_string_lossy().to_string();
    let result = json!({
        "exit": out.status.code().unwrap_or(-1),
        "stdout": String::from_utf8_lossy(&out.stdout),
        "stderr": String::from_utf8_lossy(&out.stderr).replace(&root_str, "<temp>")
    });
    record_evidence(
        json!({"subprocess": "rustc --edition=2021 --crate-type=lib -A warnings --error-format=short",
        "files": files, "entry": entry_rel, "cfgs": cfgs, "result": result}),
    );
    result
}

pub fn compile_and_run(files: &[(&str, &str)], entry_rel: &str, cfgs: &[&str]) -> Value {
    let dir = tempfile::tempdir().unwrap();
    for (rel, content) in files {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
    let bin = dir.path().join("probe_bin");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let mut cmd = Command::new(rustc);
    cmd.current_dir(dir.path())
        .args(["--edition=2021", "-A", "warnings", "--error-format=short"]);
    for cfg in cfgs {
        cmd.args(["--cfg", cfg]);
    }
    cmd.arg(entry_rel).arg("-o").arg(&bin);
    let compiled = cmd.output().unwrap();
    let root_str = dir.path().to_string_lossy().to_string();
    let mut result = json!({
        "compiler_exit": compiled.status.code().unwrap_or(-1),
        "compiler_stdout": String::from_utf8_lossy(&compiled.stdout),
        "compiler_stderr": String::from_utf8_lossy(&compiled.stderr).replace(&root_str, "<temp>"),
    });
    if compiled.status.success() {
        let ran = Command::new(&bin).current_dir(dir.path()).output().unwrap();
        result["runtime_exit"] = json!(ran.status.code().unwrap_or(-1));
        result["stdout"] = json!(String::from_utf8_lossy(&ran.stdout));
        result["stderr"] = json!(String::from_utf8_lossy(&ran.stderr).replace(&root_str, "<temp>"));
    }
    record_evidence(
        json!({"subprocess": "rustc --edition=2021 -A warnings --error-format=short; compiled binary on success",
        "files": files, "entry": entry_rel, "cfgs": cfgs, "result": result}),
    );
    result
}

pub fn record_evidence(entry: Value) {
    if std::env::var_os("EXTRACT_TRAIT_SYNTAX_CAPTURE").is_none() {
        return;
    }
    let serialized = serde_json::to_string(&entry).unwrap();
    println!("EXTRACT_TRAIT_EVIDENCE {serialized}");
    if let Ok(path) = std::env::var("EXTRACT_TRAIT_SYNTAX_CAPTURE") {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(f, "{serialized}").unwrap();
    }
}
