/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::extract_delegate::{Extracted, extract_delegate_polyglot};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

// Each anchor names one precise semantic occurrence chosen by the fixture author.
// Unrelated same-name tokens are deliberately absent from this list.
pub fn at(path: &Path, source: &str, anchor: &str, token: &str) -> Value {
    assert_eq!(
        source.matches(anchor).count(),
        1,
        "ambiguous anchor {anchor}"
    );
    let offset = source.find(anchor).unwrap() + anchor.find(token).unwrap();
    let (line, col) = prod_code_mcp::signature::position_at(source, offset).unwrap();
    json!({"uri": url::Url::from_file_path(path).unwrap().to_string(),
        "range": {"start": {"line": line - 1, "character": col - 1}}})
}

pub async fn gateway(
    file: &Path,
    source: &str,
    owner: &str,
    field: &str,
    declaration: &str,
    refs: Vec<Value>,
) -> ScriptedGateway {
    let location = at(file, source, declaration, field);
    let start = location["range"]["start"].clone();
    let symbols = json!([{"name": owner, "kind": 5, "children": [{
        "name": field, "kind": 8, "selectionRange": {"start": start, "end": start}
    }]}]);
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => symbols.clone(),
        "textDocument/references" => json!(refs),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn extract(
    ws: &Workspace,
    gw: &ScriptedGateway,
    filename: &str,
    owner: &str,
    selected: &str,
    methods: &[&str],
    helper: &str,
    field: &str,
    apply: bool,
) -> anyhow::Result<Extracted> {
    extract_delegate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path(filename),
        Some(owner),
        None,
        None,
        &[selected.into()],
        &methods.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
        helper,
        field,
        apply,
        true,
        None,
    )
    .await
}

pub fn run(ws: &Workspace, program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .current_dir(ws.root())
        .env("GOWORK", "off")
        .output()
        .unwrap_or_else(|e| panic!("{program}: {e}"));
    assert!(
        out.status.success(),
        "{program} {args:?}: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

pub fn evidence(name: &str, before: &str, after: &str, output: &str) {
    let final_review = std::env::var_os("DELEGATE_FINAL_EVIDENCE").is_some();
    if final_review && name != "external.js" {
        return;
    }
    let recovered = final_review || std::env::var_os("DELEGATE_REVIEW_EVIDENCE").is_some();
    if !recovered && std::env::var_os("DELEGATE_EVIDENCE").is_none() {
        return;
    }
    let destination = if final_review {
        "issue-1335-boundaries/runtime"
    } else if recovered {
        "issue-1335-recovered/runtime"
    } else {
        "issue-1335"
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evidence/codex-review-audit")
        .join(destination);
    fs::create_dir_all(&dir).unwrap();
    for (suffix, value) in [("before", before), ("after", after), ("stdout", output)] {
        if recovered && name == "external.js" && suffix != "stdout" {
            continue; // The lossless encoding below is the authoritative CRLF pair.
        }
        fs::write(dir.join(format!("{name}.{suffix}")), value).unwrap();
    }
    if recovered && name == "external.js" {
        let bytes: Vec<Value> = [("before", before), ("after", after)].iter().map(|(phase, value)| {
            let hex: String = value.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
            let out = Command::new("node").args(["-e",
                "console.log(require('crypto').createHash('sha256').update(Buffer.from(process.argv[1], 'hex')).digest('hex'))",
                &hex]).output().unwrap();
            assert!(out.status.success());
            let sha256 = String::from_utf8(out.stdout).unwrap().trim().to_string();
            json!({"phase": phase, "encoding": "hex", "bytes": hex, "decoded_sha256": sha256})
        }).collect();
        fs::write(
            dir.join("external.js.bytes.json"),
            serde_json::to_string_pretty(&bytes).unwrap() + "\n",
        )
        .unwrap();
    }
}
