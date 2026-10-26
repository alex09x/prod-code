/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{bytes, record, run};
use prod_code_mcp::replace_constructor::{ReplaceMode, replace_constructor_impl};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

pub struct Case<'a> {
    pub id: &'a str,
    pub files: &'a [(&'a str, &'a str)],
    pub selected: &'a str,
    pub target: &'a str,
    pub count: usize,
    pub refuse: bool,
    pub blocked_preview: bool,
    // Complete selected-owner reference coordinates, expressed as source offsets.
    pub references: Vec<(&'a str, usize, usize)>,
    pub preserved: &'a [&'a str],
}

fn coordinate(text: &str, offset: usize) -> Value {
    json!({"line":text[..offset].bytes().filter(|b| *b == b'\n').count(),
        "character":text[..offset].rsplit('\n').next().unwrap().encode_utf16().count()})
}

fn execute(root: &Path, javascript: bool) -> Vec<Value> {
    if javascript {
        return vec![run(root, &["node", "main.mjs"])];
    }
    let compile = run(
        root,
        &["rustc", "--edition=2021", "main.rs", "-o", "program"],
    );
    let mut results = vec![compile];
    if results[0]["exit"] == 0 {
        results.push(run(root, &["./program"]));
    }
    results
}

pub async fn check(case: Case<'_>) {
    let mut failures = Vec::new();
    for mode in [ReplaceMode::Factory, ReplaceMode::Builder] {
        for (policy, apply, force) in [
            ("preview", false, false),
            ("apply", true, false),
            ("force", true, true),
        ] {
            let ws = Workspace::new(case.files);
            let root = ws.root();
            let javascript = case.selected.ends_with("mjs");
            let before = execute(&root, javascript);
            let mut references = Vec::new();
            for (file, offset, length) in &case.references {
                let source = case.files.iter().find(|(name, _)| name == file).unwrap().1;
                let start = coordinate(source, *offset);
                let mut end = start.clone();
                end["character"] = json!(start["character"].as_u64().unwrap() + *length as u64);
                references.push(json!({"file":file,"range":{"start":start,"end":end}}));
            }
            let response = references
                .iter()
                .map(|r| {
                    let mut r = r.clone();
                    r["uri"] = json!(
                        url::Url::from_file_path(root.join(r["file"].as_str().unwrap()))
                            .unwrap()
                            .to_string()
                    );
                    r.as_object_mut().unwrap().remove("file");
                    r
                })
                .collect::<Vec<_>>();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = requests.clone();
            let gateway = ScriptedGateway::start(move |method, params| match method {
                "textDocument/references" => {
                    captured.lock().unwrap().push(params["position"].clone());
                    json!(response)
                },
                "textDocument/diagnostic" => json!({"kind":"full","items":[{
                    "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                    "severity":1,"message":"scripted transport sentinel; native compilation is recorded separately"}]}),
                _ => Value::Null,
            }).await;
            let result = replace_constructor_impl(
                gateway.addr(),
                &root,
                &root.join(case.selected),
                case.target,
                mode,
                None,
                apply,
                force,
                None,
            )
            .await;
            let output = case
                .files
                .iter()
                .map(|(name, _)| (*name, fs::read_to_string(root.join(name)).unwrap()))
                .collect::<BTreeMap<_, _>>();
            let unchanged = case
                .files
                .iter()
                .all(|(name, source)| output[name] == *source);
            let after = execute(&root, javascript);
            let mut returned = BTreeMap::new();
            if let Ok(result) = &result {
                for (name, text) in &result.rewritten {
                    let name = Path::new(name)
                        .strip_prefix(&root)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_string();
                    returned.insert(name, text.clone());
                }
            }
            // Compile/run the exact returned preview too, without treating it as applied.
            let preview_execution = if !apply && result.is_ok() {
                for (name, text) in &returned {
                    fs::write(root.join(name), text).unwrap();
                }
                let execution = execute(&root, javascript);
                for (name, text) in &output {
                    fs::write(root.join(name), text).unwrap();
                }
                Some(execution)
            } else {
                None
            };
            let outcome = result.as_ref().map(|r| json!({"count":r.instantiations_rewritten,"blocked":r.blocked,
                "diagnostics":r.diagnostics,"applied":r.applied,"fields":r.declared_fields}))
                .unwrap_or_else(|e| json!({"error":format!("{e:#}").replace(&*root.to_string_lossy(), "<fixture>")}));
            record(
                &format!("repair-{}-{mode}-{policy}", case.id),
                &json!({
                    "input":case.files.iter().map(|(name, text)| (*name, bytes(&root, text))).collect::<BTreeMap<_, _>>(),
                    "output":output.iter().map(|(name, text)| (*name, bytes(&root, text))).collect::<BTreeMap<_, _>>(),
                    "returned":returned.iter().map(|(name, text)| (name, bytes(&root, text))).collect::<BTreeMap<_, _>>(),
                    "references":references,"reference_bytes":bytes(&root, &serde_json::to_string(&references).unwrap()),
                    "selected_reference_requests":*requests.lock().unwrap(),
                    "transport":"scripted exact selected-owner references and nonempty diagnostic sentinel; no analyzer certification",
                    "apply":apply,"force":force,"unchanged":unchanged,
                    "before":before,"after":after,"preview_execution":preview_execution,"public_result":outcome
                }),
            );
            let tag = format!("{}-{mode}-{policy}", case.id);
            if before.iter().any(|r| r["exit"] != 0) && !case.refuse {
                failures.push(format!("{tag}: invalid input: {before:?}"));
            }
            // Native Rust validation can omit the pull sentinel (#1352); other
            // files expose it. Neither scripted branch certifies analyzer success.
            let sentinel_refusal = result
                .as_ref()
                .err()
                .is_some_and(|e| e.to_string().contains("scripted transport sentinel"));
            if case.refuse || (apply && !force && (javascript || sentinel_refusal)) {
                let refused = result.is_err()
                    || (!apply
                        && case.blocked_preview
                        && result.as_ref().is_ok_and(|r| !r.blocked.is_empty()));
                if !refused || !unchanged {
                    failures.push(format!(
                        "{tag}: expected byte-preserving refusal: {outcome}"
                    ));
                }
                continue;
            }
            match result {
                Err(_) => failures.push(format!("{tag}: {outcome}")),
                Ok(result) => {
                    if result.instantiations_rewritten != case.count || !result.blocked.is_empty() {
                        failures.push(format!("{tag}: {outcome}"));
                    }
                    let execution = preview_execution.as_ref().unwrap_or(&after);
                    if execution.iter().any(|r| r["exit"] != 0)
                        || execution.last().unwrap()["stdout"] != before.last().unwrap()["stdout"]
                    {
                        failures.push(format!("{tag}: native behavior differs: {execution:?}"));
                    }
                    for preserved in case.preserved {
                        let texts = if apply {
                            &output.values().cloned().collect::<Vec<_>>()
                        } else {
                            &returned.values().cloned().collect::<Vec<_>>()
                        };
                        if !texts.iter().any(|s| s.contains(preserved)) {
                            failures.push(format!("{tag}: lost {preserved}"));
                        }
                    }
                    if !apply && !unchanged {
                        failures.push(format!("{tag}: preview wrote source"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
