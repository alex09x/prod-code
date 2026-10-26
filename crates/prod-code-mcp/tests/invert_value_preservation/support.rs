/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::cases::Case;
use super::proof::{native, record};
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

pub const MANIFEST: &str =
    "[package]\nname = \"invert_probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n";
const SENTINEL: &str = "// exact unrelated file\npub const ENABLED: bool = true;\n";

fn references(file: &Path, source: &str) -> Vec<Value> {
    let code = prod_code_mcp::extract_trait::lex::lexical_code(source);
    source.match_indices("enabled").filter(|(at, _)| code[*at]).skip(1)
        .map(|(at, _)| {
            let (line, col) = prod_code_mcp::signature::position_at(source, at).unwrap();
            json!({"uri": url::Url::from_file_path(file).unwrap().to_string(), "range": {"start": {"line": line-1, "character": col-1}}})
        }).collect()
}

pub async fn matrix(case: Case) {
    record("fixture", "Cargo.toml", MANIFEST);
    record("fixture", "unrelated.rs", SENTINEL);
    record(case.id, "original.rs", &case.source);
    let (valid, before) = native(&case, &case.source, "original");
    assert!(valid, "{} original must compile and run", case.id);
    let mut failures = Vec::new();
    for verify in [false, true] {
        for force in [false, true] {
            for apply in [false, true] {
                let label = format!("apply-{apply}-compile-{verify}-force-{force}");
                let ws = Workspace::new(&[
                    ("Cargo.toml", MANIFEST),
                    ("src/main.rs", &case.source),
                    ("src/unrelated.rs", SENTINEL),
                ]);
                let file = ws.path("src/main.rs");
                let refs = references(&file, &case.source);
                let proposal = Arc::new(Mutex::new(None::<String>));
                let captured = proposal.clone();
                let shadows = Arc::new(Mutex::new(0));
                let count = shadows.clone();
                let gw = ScriptedGateway::start(move |method, params| match method {
                    "textDocument/references" => json!(refs),
                    "textDocument/diagnostic" => answers::no_diagnostics(),
                    "textDocument/didOpen" | "textDocument/didChange" => {
                        let text = params
                            .pointer("/textDocument/text")
                            .or_else(|| params.pointer("/contentChanges/0/text"))
                            .and_then(Value::as_str);
                        if let Some(text) = text.filter(|t| t.contains("disabled")) {
                            *captured.lock().unwrap() = Some(text.into());
                        }
                        Value::Null
                    }
                    "prod-code/shadow-run" => {
                        *count.lock().unwrap() += 1;
                        json!({"exit_code": 0})
                    }
                    _ => Value::Null,
                })
                .await;
                let (line, col) = prod_code_mcp::signature::position_at(
                    &case.source,
                    case.source.find("enabled").unwrap(),
                )
                .unwrap();
                let mut args = json!({"path":"src/main.rs", "line":line, "character":col, "new_name":"disabled", "apply":apply, "force":force});
                if verify {
                    args["verify"] = json!("compile");
                }
                let response =
                    execute_tool(gw.addr(), &ws.root(), "code_invert_boolean", args.clone()).await;
                let report = match &response {
                    Ok(r) => serde_json::to_string_pretty(r).unwrap(),
                    Err(e) => format!("error: {e:#}"),
                };
                record(
                    case.id,
                    &format!("{label}.response.txt"),
                    &format!(
                        "public code_invert_boolean {args}\nSCRIPTED references and empty diagnostics; SCRIPTED compile-gate exit=0 (not compiler proof).\n{report}\nshadow_calls={}",
                        shadows.lock().unwrap()
                    ),
                );
                let returned = proposal
                    .lock()
                    .unwrap()
                    .clone()
                    .expect("the validation overlay must expose the exact proposal");
                record(case.id, &format!("{label}.returned.rs"), &returned);
                let disk = ws.read("src/main.rs");
                record(case.id, &format!("{label}.disk.rs"), &disk);
                let should_write = apply && (case.block.is_none() || force);
                if &disk
                    != if should_write {
                        &returned
                    } else {
                        &case.source
                    }
                {
                    failures.push(format!(
                        "{} {label}: exact write/no-write assertion failed",
                        case.id
                    ));
                }
                assert_eq!(ws.read("Cargo.toml"), MANIFEST);
                assert_eq!(ws.read("src/unrelated.rs"), SENTINEL);
                record(
                    case.id,
                    &format!("{label}.assertions.txt"),
                    &format!(
                        "expected_write={should_write}\nmain_exact={}\nmanifest_exact=true\nunrelated_exact=true\n",
                        &disk
                            == if should_write {
                                &returned
                            } else {
                                &case.source
                            }
                    ),
                );
                if let Some(block) = case.block {
                    if !report.contains(block) {
                        failures.push(format!("{} {label}: missing blocker {block}", case.id));
                    }
                    if case.serde && !report.contains("Deserialize") {
                        failures.push(format!("{} {label}: missing Deserialize blocker", case.id));
                    }
                    if apply && !force && verify && *shadows.lock().unwrap() != 0 {
                        failures.push(format!(
                            "{} {label}: blocked plan reached compile gate",
                            case.id
                        ));
                    }
                } else if response.as_ref().map_or(true, |r| r.is_error) {
                    failures.push(format!("{} {label}: useful positive refused", case.id));
                }
                // Every distinct proposal is checked by native Rust independently of canned replies.
                if !apply && !force && !verify {
                    let (compiled, after) = native(&case, &returned, "returned");
                    if case.block.is_none() {
                        assert!(compiled, "{} positive result must compile", case.id);
                        assert_eq!(after, before, "{} runtime preservation", case.id);
                    } else if case.semantic_control && compiled {
                        assert_ne!(
                            after, before,
                            "{} control must demonstrate semantic loss despite compiler success",
                            case.id
                        );
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

pub async fn incomplete() {
    let source = "struct Flags { enabled: bool }\nfn main() { let f = Flags { enabled: true }; println!(\"{}\", f.enabled); }\n";
    for missing_file in [false, true] {
        for verify in [false, true] {
            for force in [false, true] {
                let ws = Workspace::new(&[
                    ("Cargo.toml", MANIFEST),
                    ("src/main.rs", source),
                    ("src/unrelated.rs", SENTINEL),
                ]);
                let mut refs = references(&ws.path("src/main.rs"), source);
                refs.push(json!({"uri": url::Url::from_file_path(ws.path(if missing_file {"src/missing.rs"} else {"src/unrelated.rs"})).unwrap().to_string(), "range":{"start":{"line":0,"character":0}}}));
                let gw = ScriptedGateway::start(move |method, _| match method {
                    "textDocument/references" => json!(refs),
                    "textDocument/diagnostic" => answers::no_diagnostics(),
                    "prod-code/shadow-run" => panic!("incomplete plan reached compile gate"),
                    _ => Value::Null,
                })
                .await;
                let mut args = json!({"path":"src/main.rs", "line":1,"character":16,"new_name":"disabled","apply":true,"force":force});
                if verify {
                    args["verify"] = json!("compile");
                }
                let result =
                    execute_tool(gw.addr(), &ws.root(), "code_invert_boolean", args.clone()).await;
                assert!(
                    result.is_err(),
                    "unmatched or unreadable references must refuse even when forced"
                );
                assert_eq!(ws.read("src/main.rs"), source);
                assert_eq!(ws.read("src/unrelated.rs"), SENTINEL);
                assert_eq!(ws.read("Cargo.toml"), MANIFEST);
                record(
                    "incomplete",
                    &format!("missing-{missing_file}-compile-{verify}-force-{force}.txt"),
                    &format!(
                        "public code_invert_boolean {args}\nSCRIPTED stale or unreadable reference; empty diagnostics\n{:#}\nall files exact=true\n",
                        result.unwrap_err()
                    ),
                );
            }
        }
    }
}
