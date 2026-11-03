/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "move_child_struct_fields/cases.rs"]
mod cases;

use std::path::{Path, PathBuf};
use std::process::Command;

use prod_code_mcp::move_item::move_item;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

pub(crate) const MANIFEST: &str =
    "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

pub(crate) fn compile_and_run_tests(root: &Path, lib_rs: &Path) {
    let binary = root.join("fixture-test");
    let compiled = Command::new("rustc")
        .args(["--edition=2021", "--test"])
        .arg(lib_rs)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "rustc --test failed:\n{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = Command::new(&binary).output().unwrap();
    assert!(
        ran.status.success(),
        "test binary failed:\n{}",
        String::from_utf8_lossy(&ran.stdout)
    );
}

fn assert_rustc_privacy_error(root: &Path, lib_rs: &Path, expected_code: &str) {
    let out = root.join("privacy-probe.rlib");
    let compiled = Command::new("rustc")
        .args(["--edition=2021", "--crate-type=lib"])
        .arg(lib_rs)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        !compiled.status.success(),
        "expected rustc privacy error {expected_code}, but compilation succeeded"
    );
    let stderr = String::from_utf8_lossy(&compiled.stderr);
    assert!(
        stderr.contains(expected_code),
        "expected {expected_code} in stderr:\n{stderr}"
    );
}

pub(crate) fn coord_of(text: &str, needle: &str) -> (u32, u32) {
    let idx = text
        .find(needle)
        .unwrap_or_else(|| panic!("missing `{needle}` in:\n{text}"));
    let before = &text[..idx];
    let line = before.bytes().filter(|b| *b == b'\n').count() as u32 + 1;
    let line_start = before.rfind('\n').map_or(0, |p| p + 1);
    let col = text[line_start..idx].encode_utf16().count() as u32 + 1;
    (line, col)
}

pub(crate) fn all_coords_of(path: &Path, text: &str, ident: &str) -> Vec<(PathBuf, u32, u32)> {
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    for (idx, _) in text.match_indices(ident) {
        let prev_ok = !text[..idx].chars().next_back().is_some_and(is_id);
        let next_ok = !text[idx + ident.len()..].chars().next().is_some_and(is_id);
        if prev_ok && next_ok {
            let before = &text[..idx];
            let line = before.bytes().filter(|b| *b == b'\n').count() as u32 + 1;
            let line_start = before.rfind('\n').map_or(0, |p| p + 1);
            let col = text[line_start..idx].encode_utf16().count() as u32 + 1;
            out.push((path.to_path_buf(), line, col));
        }
    }
    out
}

pub(crate) fn locations_across(items: &[(PathBuf, u32, u32)]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|(path, line, col)| {
                let uri = url::Url::from_file_path(path).unwrap().to_string();
                json!({
                    "uri": uri,
                    "range": {
                        "start": { "line": line - 1, "character": col - 1 },
                        "end": { "line": line - 1, "character": col }
                    }
                })
            })
            .collect(),
    )
}

#[tokio::test]
async fn child_move_widens_only_accessed_named_fields_and_keeps_others_private() {
    let a_src = "\
#[derive(Default)]
struct Port(u16);

#[derive(Default)]
pub struct Endpoint {
    host: String,
    port: Port,
    retries: u8,
    secret: String,
    pub label: &'static str,
    pub(crate) tag: u8,
    pub(super) outer: u8,
    pub(self) local: u8,
}

#[derive(Default)]
struct Decoy {
    secret: String,
}

mod sibling;

fn decoy_secret(d: &Decoy) -> &str {
    &d.secret
}

pub fn check_endpoint() -> usize {
    let mut ep = Endpoint::default();
    ep.retries += 1;
    assert_eq!(ep.retries, 1);
    let _ = decoy_secret(&Decoy::default());
    ep.host.len() + sibling::port_value(ep) as usize
}
";
    let sibling_src = "\
use super::Endpoint;

pub(super) fn port_value(ep: Endpoint) -> u16 {
    let Endpoint { port, .. } = ep;
    port.0
}
";
    let lib_src = "\
pub mod a;

#[test]
fn moved_endpoint_still_works() {
    assert_eq!(a::check_endpoint(), 0);
}
";
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", lib_src),
        ("src/a.rs", a_src),
        ("src/a/sibling.rs", sibling_src),
    ]);
    let file = ws.path("src/a.rs");
    let sibling = ws.path("src/a/sibling.rs");
    let target = ws.path("src/a/model.rs");

    let (decl_line, decl_col) = coord_of(a_src, "Endpoint");
    let decl_end = coord_of(a_src, "pub(self) local: u8,\n}").0 + 1;
    let host_def = coord_of(a_src, "host: String");
    let retries_def = coord_of(a_src, "retries: u8");
    let decoy_secret_def = {
        let sub = &a_src[a_src.find("struct Decoy").unwrap()..];
        let base_line = a_src[..a_src.find("struct Decoy").unwrap()]
            .bytes()
            .filter(|b| *b == b'\n')
            .count() as u32;
        let (l, c) = coord_of(sub, "secret: String");
        (base_line + l, c)
    };
    let host_use = coord_of(a_src, "host.len()");
    let retries_use_1 = coord_of(a_src, "retries += 1");
    let retries_use_2 = coord_of(a_src, "retries, 1");
    let decoy_use = coord_of(a_src, "secret\n}");

    let mut refs = all_coords_of(&file, a_src, "Endpoint");
    refs.retain(|(_, l, _)| *l != decl_line);
    refs.extend(all_coords_of(&sibling, sibling_src, "Endpoint"));

    let file_for_gw = file.clone();
    let gw = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => json!([
            answers::document_symbol("Port", 23, 2, 2, 8),
            answers::document_symbol("Endpoint", 23, decl_line, decl_end, decl_col),
            answers::document_symbol("Decoy", 23, 17, 19, 8),
        ]),
        "textDocument/references" => locations_across(&refs),
        "textDocument/definition" => {
            let l = params["position"]["line"].as_u64().unwrap() as u32 + 1;
            let c = params["position"]["character"].as_u64().unwrap() as u32 + 1;
            if (l, c) == host_use {
                answers::locations(&file_for_gw, &[host_def])
            } else if (l, c) == retries_use_1 || (l, c) == retries_use_2 {
                answers::locations(&file_for_gw, &[retries_def])
            } else if (l, c) == decoy_use {
                answers::locations(&file_for_gw, &[decoy_secret_def])
            } else {
                Value::Null
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let moved = move_item(
        gw.addr(),
        &ws.root(),
        &file,
        decl_line,
        decl_col,
        &target,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(moved.applied);

    let child = ws.read("src/a/model.rs");
    assert!(child.contains("pub struct Endpoint {"), "{child}");
    assert!(child.contains("    pub(super) host: String,\n"), "{child}");
    assert!(child.contains("    pub(super) port: Port,\n"), "{child}");
    assert!(child.contains("    pub(super) retries: u8,\n"), "{child}");
    assert!(
        child.contains("    secret: String,\n"),
        "untouched private field must stay private:\n{child}"
    );
    assert!(child.contains("    pub label: &'static str,\n"), "{child}");
    assert!(child.contains("    pub(crate) tag: u8,\n"), "{child}");
    assert!(
        child.contains("    pub(in super::super) outer: u8,\n"),
        "{child}"
    );
    assert!(child.contains("    pub(super) local: u8,\n"), "{child}");

    compile_and_run_tests(&ws.root(), &ws.path("src/lib.rs"));

    // Compiler proof 1: untouched field `secret` is inaccessible even inside parent `a`.
    let a_saved = ws.read("src/a.rs");
    std::fs::write(
        &file,
        format!(
            "{a_saved}\npub fn illegal_secret(ep: &model::Endpoint) -> &str {{ &ep.secret }}\n"
        ),
    )
    .unwrap();
    assert_rustc_privacy_error(&ws.root(), &ws.path("src/lib.rs"), "E0616");
    std::fs::write(&file, &a_saved).unwrap();

    // Compiler proof 2: `pub(super) host` is NOT exposed outside `a` (e.g. in crate root).
    let lib_saved = ws.read("src/lib.rs");
    std::fs::write(
        ws.path("src/lib.rs"),
        format!(
            "{lib_saved}\npub fn illegal_outer(ep: &a::model::Endpoint) -> &str {{ &ep.host }}\n"
        ),
    )
    .unwrap();
    assert_rustc_privacy_error(&ws.root(), &ws.path("src/lib.rs"), "E0616");
    std::fs::write(ws.path("src/lib.rs"), &lib_saved).unwrap();
}
