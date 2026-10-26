/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "rust_move_preservation/boundaries.rs"]
mod boundaries;
#[path = "rust_move_preservation/cases.rs"]
mod cases;
#[path = "rust_move_preservation/links.rs"]
mod links;
#[path = "rust_move_preservation/owners.rs"]
mod owners;
#[path = "rust_move_preservation/ownership_bindings.rs"]
mod ownership_bindings;
#[path = "rust_move_preservation/ownership_controls.rs"]
mod ownership_controls;
#[path = "rust_move_preservation/ownership_review.rs"]
mod ownership_review;
#[path = "rust_move_preservation/remaining.rs"]
mod remaining;
#[path = "rust_move_preservation/residual.rs"]
mod residual;
#[path = "rust_move_preservation/review.rs"]
mod review;
#[path = "rust_move_preservation/support.rs"]
mod support;

use prod_code_mcp::{move_item::move_item, move_module::move_module};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::*;

#[tokio::test]
async fn original_outstanding_public_moves() {
    let audit: Value = serde_json::from_str(include_str!(
        "../../../evidence/codex-review-audit/rust-moves-eleven-current.json"
    ))
    .unwrap();
    let mut records = Vec::new();
    let mut failures = Vec::new();
    for finding in audit["findings"].as_array().unwrap() {
        if finding["status"] != "outstanding" {
            continue;
        }
        for case in finding["source_predicted_cases"].as_array().unwrap() {
            let id = case["case_id"].as_str().unwrap();
            let layout = case["complete_source_layout"].as_object().unwrap();
            let files: Vec<_> = layout
                .iter()
                .map(|(p, t)| (p.as_str(), t.as_str().unwrap()))
                .collect();
            let ws = Workspace::new(&files);
            #[cfg(unix)]
            if let Some(links) = case["symlinks"].as_object() {
                for (p, t) in links {
                    let link = ws.path(p);
                    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
                    std::os::unix::fs::symlink(t.as_str().unwrap(), link).unwrap();
                }
            }
            let before = snapshot(&ws.root());
            let before_compiler = compiler(&ws.root(), id, false);
            let args = &case["public_entry"]["arguments"];
            let file = ws.path(args["path"].as_str().unwrap());
            let target = ws.path(args["to"].as_str().unwrap());
            let trace = Arc::new(Mutex::new(Vec::new()));
            let recorded = trace.clone();
            let source = std::fs::read_to_string(&file).unwrap();
            let name = match id {
                "item-multiline-derive-reattaches" => "Moved",
                "binary-item-declared-in-library" => "helper",
                _ => "api",
            };
            let start = args["line"].as_u64().unwrap_or(1) as u32;
            let end = if name == "helper" { 3 } else { start };
            let source_path = file.clone();
            let remote = ScriptedGateway::start(move |method, params| {
                let response = match method {
                    "textDocument/documentSymbol" => {
                        json!([answers::document_symbol(name, 12, start, end, 1)])
                    }
                    "textDocument/references" if name == "helper" => {
                        let col = source.lines().nth(4).unwrap().find("helper").unwrap();
                        answers::locations(&source_path, &[(5, col as u32 + 1)])
                    }
                    "textDocument/references" => json!([]),
                    "textDocument/diagnostic" => answers::no_diagnostics(),
                    _ => Value::Null,
                };
                recorded.lock().unwrap().push(json!({
                    "method": method, "params": params, "response": response
                }));
                response
            })
            .await;
            let returned = if case["public_entry"]["tool"] == "code_move" {
                match move_item(
                    remote.addr(),
                    &ws.root(),
                    &file,
                    start,
                    args["character"].as_u64().unwrap() as u32,
                    &target,
                    true,
                    args["force"].as_bool().unwrap_or(false),
                )
                .await
                {
                    Ok(plan) => json!({"applied": plan.applied, "sources": plan.rewritten,
                        "diagnostics": plan.diagnostics, "unmatched": plan.unmatched}),
                    Err(e) => json!({"error": format!("{e:#}")}),
                }
            } else {
                match move_module(remote.addr(), &ws.root(), &file, &target).await {
                    Ok(mut plan) => {
                        let result = plan.write(false).map_err(|e| format!("{e:#}"));
                        json!({"applied": plan.applied, "sources": plan.rewritten,
                            "removed": plan.removed, "write_result": result,
                            "diagnostics": plan.diagnostics, "unmatched": plan.unmatched})
                    }
                    Err(e) => json!({"error": format!("{e:#}")}),
                }
            };
            let after = snapshot(&ws.root());
            let after_compiler = compiler(&ws.root(), id, true);
            let safe_link = id == "directory-link-deletes-shared-module"
                && returned.get("error").is_some()
                && before == after;
            let rejected_owner = id == "target-owned-only-by-binary"
                && returned.get("error").is_some()
                && before == after;
            let correct = match id {
                "directory-link-deletes-shared-module" => safe_link,
                "target-owned-only-by-binary" => rejected_owner,
                "item-multiline-derive-reattaches" => {
                    ws.read("src/dest.rs").contains("#[derive(\n    Clone,\n)]")
                        && !ws.read("src/source.rs").contains("#[derive")
                }
                "attributed-missing-file-declaration-duplicated" => {
                    ws.read("src/lib.rs").matches("mod util;").count() == 1
                }
                "module-cfg-block-doc-reattach" => {
                    ws.read("src/c.rs").contains("/** B module documentation.")
                        && !ws.read("src/a.rs").contains("#[cfg")
                }
                "super-string-macro-nested-scope" => {
                    ws.read("src/c/b.rs").contains("\"super::help\"")
                        && ws.read("src/c/b.rs").contains("text!(super::help)")
                }
                "module-shared-by-lib-and-bin" => !ws.read("src/main.rs").contains("mod a;"),
                _ => returned["applied"] == true,
            } && (safe_link || rejected_owner || after_compiler["passed"] == true);
            if !correct {
                failures.push(id.to_string());
            }
            records.push(json!({"case": id, "finding": finding["comment_id"],
                "provenance": "public Rust move functions; scripted document symbols/references/clean diagnostics, independent real rustc compilation and executable probes",
                "before": before, "before_compiler": before_compiler,
                "returned": returned, "after": after, "after_compiler": after_compiler,
                "analyzer_transcript": *trace.lock().unwrap(), "passed": correct}));
        }
    }
    save_evidence(&records);
    assert!(failures.is_empty(), "preservation failures: {failures:?}");
}
