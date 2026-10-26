/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::wrap_return::{WrappedReturn, Wrapper, wrap_polyglot_ext};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::{
    fs,
    process::{Command, Output},
};

pub async fn gateway(ws: &Workspace, refs: &[(&str, usize)]) -> ScriptedGateway {
    let refs: Vec<_> = refs.iter().map(|(path, at)| {
        let text = fs::read_to_string(ws.root().join(path)).unwrap();
        let prefix = &text[..*at];
        let line = prefix.bytes().filter(|b| *b == b'\n').count();
        let col = prefix.rsplit('\n').next().unwrap().encode_utf16().count();
        serde_json::json!({"uri": url::Url::from_file_path(ws.root().join(path)).unwrap().to_string(),
            "range": {"start": {"line": line, "character": col},
                      "end": {"line": line, "character": col + 9}}})
    }).collect();
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => serde_json::json!(refs),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

pub async fn wrap(
    ws: &Workspace,
    file: &str,
    name: &str,
    refs: &[(&str, usize)],
    apply: bool,
    force: bool,
) -> anyhow::Result<WrappedReturn> {
    let before = sources(ws);
    let gw = gateway(ws, refs).await;
    let result = wrap_polyglot_ext(
        gw.addr(),
        &ws.root(),
        &ws.root().join(file),
        Some(name),
        None,
        None,
        Wrapper::Option,
        None,
        None,
        apply,
        force,
    )
    .await;
    let references: Vec<_> = refs
        .iter()
        .map(|(path, at)| {
            let prefix = &before[*path][..*at];
            serde_json::json!({"path": path, "byte": at,
            "line": prefix.bytes().filter(|b| *b == b'\n').count() + 1,
            "column": prefix.rsplit('\n').next().unwrap().encode_utf16().count() + 1})
        })
        .collect();
    let returned = match &result {
        Ok(value) => serde_json::to_value(value).unwrap(),
        Err(error) => serde_json::json!({"error": error.to_string()}),
    };
    let selected_at = before[file].find(name).unwrap();
    let selected_prefix = &before[file][..selected_at];
    let record = serde_json::json!({"test": std::thread::current().name(),
        "selected_file": file, "selected_name": name, "apply": apply, "force": force,
        "selected_position": {"byte": selected_at,
            "line": selected_prefix.bytes().filter(|b| *b == b'\n').count() + 1,
            "column": selected_prefix.rsplit('\n').next().unwrap().encode_utf16().count() + 1},
        "references": references, "before": before, "after": sources(ws), "returned": returned});
    // Preserve complete public-operation evidence without temporary workspace paths.
    eprintln!(
        "PUBLIC_WRAP {}",
        record
            .to_string()
            .replace(&ws.root().display().to_string(), "<fixture>")
    );
    result
}

fn sources(ws: &Workspace) -> std::collections::BTreeMap<String, String> {
    ignore::WalkBuilder::new(ws.root())
        .build()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() || prod_code_mcp::parameter_object::Language::of(path).is_none() {
                return None;
            }
            Some((
                path.strip_prefix(ws.root()).unwrap().display().to_string(),
                fs::read_to_string(path).ok()?,
            ))
        })
        .collect()
}

pub fn run(ws: &Workspace, program: &str, args: &[&str]) -> Output {
    let output = Command::new(program)
        .args(args)
        .current_dir(ws.root())
        .output()
        .unwrap();
    eprintln!(
        "native {program} {args:?}: exit={:?}\n{}{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub fn unchanged(ws: &Workspace, files: &[(&str, &str)]) {
    for (path, source) in files {
        assert_eq!(
            fs::read_to_string(ws.root().join(path)).unwrap(),
            *source,
            "{path}"
        );
    }
}
