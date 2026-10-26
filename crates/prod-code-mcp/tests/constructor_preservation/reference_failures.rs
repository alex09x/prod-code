/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{bytes, record};
use prod_code_mcp::replace_constructor::{
    ReplaceMode, replace_constructor_with_builder, replace_constructor_with_factory,
};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

const MANIFEST: &str = "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n[[bin]]\nname='fixture'\npath='main.rs'\n";
// Exact selected type and caller from the historical confirmed-super failure.
const MAIN: &str = "#[path=\"model.rs\"] mod model;\npub struct Config { pub value: i32 }\nfn main() { println!(\"{}\", model::make().value); }\n";
const MODEL: &str = "pub fn make() -> super::Config { super::Config { value: 7 } }\n";
const UNUSED: &str = "pub struct Config { pub value: i32 }\nfn main() {}\n";
const ERROR: &str = "reference discovery sentinel: query failed before returning locations";

#[derive(Clone, Copy)]
enum Reply {
    Error,
    Malformed,
    Nonempty,
    Empty,
}

fn position(text: &str, at: usize) -> Value {
    json!({"line":text[..at].bytes().filter(|b| *b == b'\n').count(),
        "character":text[..at].rsplit('\n').next().unwrap().encode_utf16().count()})
}

async fn check(id: &str, main: &str, model: &str, reply: Reply) {
    let mut failures = Vec::new();
    for mode in [ReplaceMode::Factory, ReplaceMode::Builder] {
        for (policy, apply, force) in [
            ("preview", false, false),
            ("apply", true, false),
            ("force", true, true),
        ] {
            let files = [
                ("main.rs", main),
                ("model.rs", model),
                ("Cargo.toml", MANIFEST),
            ];
            let ws = Workspace::new(&files);
            let root = ws.root();
            let uri = url::Url::from_file_path(root.join("model.rs"))
                .unwrap()
                .to_string();
            let locations: Vec<_> = model.match_indices("Config").map(|(at, _)| {
                json!({"uri":uri, "range":{"start":position(model, at), "end":position(model, at + 6)}})
            }).collect();
            let (answer, response) = match reply {
                Reply::Error => (
                    answers::rpc_error(-32603, ERROR),
                    json!({"error":{"code":-32603,"message":ERROR}}),
                ),
                Reply::Malformed => {
                    let result =
                        json!([locations[0], {"uri":uri,"range":{"end":{"line":0,"character":6}}}]);
                    (result.clone(), json!({"result":result}))
                }
                Reply::Nonempty => (json!(locations), json!({"result":locations})),
                Reply::Empty => (json!([]), json!({"result":[]})),
            };
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = requests.clone();
            let gateway = ScriptedGateway::start(move |method, params| {
                if method.starts_with("textDocument/") || method == "prod-code/shadow-run" {
                    captured
                        .lock()
                        .unwrap()
                        .push(json!({"method":method,"params":params}));
                }
                match method {
                    "textDocument/references" => answer.clone(),
                    "textDocument/diagnostic" => answers::no_diagnostics(),
                    _ => Value::Null,
                }
            })
            .await;
            let result = match mode {
                ReplaceMode::Factory => {
                    replace_constructor_with_factory(
                        gateway.addr(),
                        &root,
                        &root.join("main.rs"),
                        "Config",
                        None,
                        apply,
                        force,
                        None,
                    )
                    .await
                }
                ReplaceMode::Builder => {
                    replace_constructor_with_builder(
                        gateway.addr(),
                        &root,
                        &root.join("main.rs"),
                        "Config",
                        None,
                        apply,
                        force,
                        None,
                    )
                    .await
                }
            };
            let disk: BTreeMap<_, _> = files
                .iter()
                .map(|(name, _)| (*name, ws.read(name)))
                .collect();
            let returned: BTreeMap<_, _> = result
                .as_ref()
                .ok()
                .map(|r| {
                    r.rewritten
                        .iter()
                        .map(|(path, text)| {
                            (
                                std::path::Path::new(path)
                                    .strip_prefix(&root)
                                    .unwrap()
                                    .to_str()
                                    .unwrap()
                                    .to_string(),
                                text.clone(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let outcome = result
                .as_ref()
                .map(|r| {
                    json!({"count":r.instantiations_rewritten,
                "applied":r.applied,"blocked":r.blocked,"diagnostics":r.diagnostics})
                })
                .unwrap_or_else(|e| json!({"error":format!("{e:#}")}));
            let requests = requests.lock().unwrap().clone();
            let evidence = json!({"mode":mode,"apply":apply,"force":force,
                "input":files.iter().map(|(name, text)| (*name, bytes(&root, text))).collect::<BTreeMap<_, _>>(),
                "returned":returned.iter().map(|(name, text)| (name, bytes(&root, text))).collect::<BTreeMap<_, _>>(),
                "disk":disk.iter().map(|(name, text)| (*name, bytes(&root, text))).collect::<BTreeMap<_, _>>(),
                "requests":requests,"reference_response":response,"public_result":outcome,
                "transport":"real scripted TCP gateway; explicit LSP error or exact successful locations; no live analyzer completeness claim"});
            // Preserve payload bytes, replacing only the temporary workspace prefix.
            let evidence: Value = serde_json::from_str(
                &serde_json::to_string(&evidence)
                    .unwrap()
                    .replace(&*root.to_string_lossy(), "<fixture>"),
            )
            .unwrap();
            let tag = format!("reference-{id}-{mode}-{policy}");
            record(&tag, &evidence);
            let refs: Vec<_> = requests
                .iter()
                .filter(|r| r["method"] == "textDocument/references")
                .collect();
            let expected = json!({"textDocument":{"uri":url::Url::from_file_path(root.join("main.rs")).unwrap().to_string()},
                "position":position(main, main.find("struct Config").unwrap() + 7),
                "context":{"includeDeclaration":false}});
            if refs.len() != 1 || refs[0]["params"] != expected {
                failures.push(format!("{tag}: wrong selected-type query: {refs:?}"));
            }
            let unchanged = files.iter().all(|(name, text)| disk[name] == *text);
            if matches!(reply, Reply::Error | Reply::Malformed) {
                let cause = if matches!(reply, Reply::Error) {
                    ERROR
                } else {
                    "has no file or start position"
                };
                let expected_error = result.as_ref().err().is_some_and(|e| {
                    let chain = format!("{e:#}");
                    chain.contains("reference discovery incomplete")
                        && chain.contains(cause)
                        && chain.contains("nothing was written")
                });
                let validated = requests.iter().any(|r| {
                    matches!(
                        r["method"].as_str(),
                        Some(
                            "textDocument/didChange"
                                | "textDocument/diagnostic"
                                | "prod-code/shadow-run"
                        )
                    )
                });
                if !expected_error || !unchanged || !returned.is_empty() || validated {
                    failures.push(format!("{tag}: expected early byte-preserving error; unchanged={unchanged}, validated={validated}: {outcome}"));
                }
                continue;
            }
            match result {
                Err(e) => failures.push(format!("{tag}: unexpected error: {e:#}")),
                Ok(r) => {
                    let expected_count = usize::from(matches!(reply, Reply::Nonempty));
                    if r.instantiations_rewritten != expected_count
                        || r.applied != apply
                        || !r.blocked.is_empty()
                        || !r.diagnostics.is_empty()
                        || returned.is_empty()
                    {
                        failures.push(format!("{tag}: unexpected successful result: {outcome}"));
                    }
                    if apply {
                        if returned
                            .iter()
                            .any(|(name, text)| disk[name.as_str()] != *text)
                            || unchanged
                        {
                            failures.push(format!("{tag}: returned and applied bytes differ"));
                        }
                    } else if !unchanged {
                        failures.push(format!("{tag}: preview wrote source"));
                    }
                    if matches!(reply, Reply::Nonempty) {
                        let needle = match mode {
                            ReplaceMode::Factory => "super::Config::new(7)",
                            ReplaceMode::Builder => "super::Config::builder().value(7).build()",
                        };
                        if !returned
                            .get("model.rs")
                            .is_some_and(|text| text.contains(needle))
                        {
                            failures.push(format!(
                                "{tag}: exact caller was not rewritten: {returned:?}"
                            ));
                        }
                    } else if returned.contains_key("model.rs") || disk["model.rs"] != model {
                        failures.push(format!("{tag}: unused caller file changed"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn reference_failures_explicit_query_error() {
    check("rpc-error", MAIN, MODEL, Reply::Error).await;
}

#[tokio::test]
async fn reference_failures_malformed_locations() {
    check("malformed", MAIN, MODEL, Reply::Malformed).await;
}

#[tokio::test]
async fn reference_failures_successful_nonempty_controls() {
    check("confirmed-super", MAIN, MODEL, Reply::Nonempty).await;
    check(
        "known-owner",
        &MAIN.replace("#[path=\"model.rs\"] mod model;", "mod model;"),
        MODEL,
        Reply::Nonempty,
    )
    .await;
}

#[tokio::test]
async fn reference_failures_successful_empty_unused_type() {
    check("unused", UNUSED, "pub fn other() {}\n", Reply::Empty).await;
}
