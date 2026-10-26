/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::{ScriptedGateway, answers};
use std::path::Path;
use std::process::Command;

pub fn location(root: &Path, file: &str, source: &str, needle: &str) -> serde_json::Value {
    let at = source.find(needle).expect("unique semantic token");
    let before = &source[..at];
    serde_json::json!({
        "uri": url::Url::from_file_path(root.join(file)).unwrap().to_string(),
        "range": {"start": {
            "line": before.bytes().filter(|b| *b == b'\n').count(),
            "character": before.rsplit('\n').next().unwrap().encode_utf16().count()
        }}
    })
}

pub async fn gateway(refs: Vec<serde_json::Value>) -> ScriptedGateway {
    gateway_with_declarations(refs, Vec::new()).await
}

pub async fn gateway_with_declarations(
    refs: Vec<serde_json::Value>,
    declarations: Vec<serde_json::Value>,
) -> ScriptedGateway {
    ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => {
            let mut found = refs.clone();
            if params
                .pointer("/context/includeDeclaration")
                .and_then(|v| v.as_bool())
                == Some(true)
            {
                found.extend(declarations.clone());
            }
            serde_json::json!(found)
        }
        "textDocument/declaration" => serde_json::json!(declarations),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

pub fn run(root: &Path, program: &str, args: &[&str]) -> String {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|e| panic!("required runtime {program}: {e}"));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!(
        "{program} {args:?}: status={} stdout={stdout:?} stderr={stderr:?}",
        output.status
    );
    assert!(output.status.success(), "{program}: {stderr}");
    stdout
}

pub fn evidence(root: &Path, file: &str, label: &str) {
    let text = std::fs::read_to_string(root.join(file)).unwrap();
    eprintln!("{label} {file}:\n{text}");
    run(root, "sha256sum", &[file]);
}
