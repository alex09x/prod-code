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
use futures_util::FutureExt;
use prod_code_mcp::replace_constructor::{
    ReplaceMode, replace_constructor_with_builder, replace_constructor_with_factory,
};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};
use std::{fs, panic::AssertUnwindSafe, path::Path};

fn execute(root: &Path, ext: &str) -> Vec<Value> {
    let compile = match ext {
        "rs" => Some(run(
            root,
            &["rustc", "--edition=2021", "main.rs", "-o", "program"],
        )),
        "cpp" => Some(run(
            root,
            &["c++", "-std=c++20", "main.cpp", "-o", "program"],
        )),
        // Untyped controls are valid with noImplicitAny disabled; typed cases still
        // receive full tsc checking. Strict null checking catches optional mistakes.
        "ts" => Some(run(
            root,
            &[
                "tsc",
                "--target",
                "es2022",
                "--module",
                "commonjs",
                "--strictNullChecks",
                "--noImplicitAny",
                "false",
                "--noEmitOnError",
                "main.ts",
            ],
        )),
        _ => None,
    };
    let mut results = compile.into_iter().collect::<Vec<_>>();
    if results.iter().all(|r| r["exit"] == 0) {
        results.push(match ext {
            "rs" | "cpp" => run(root, &["./program"]),
            "ts" => run(root, &["node", "main.js"]),
            "mjs" => run(root, &["node", "main.mjs"]),
            "py" => run(root, &["python3", "main.py"]),
            _ => panic!("unsupported fixture"),
        });
    }
    results
}

pub async fn check(id: &str, ext: &str, target: &str, source: &str, count: usize) {
    check_modes(
        id,
        ext,
        target,
        source,
        count,
        &[ReplaceMode::Factory, ReplaceMode::Builder],
    )
    .await;
}

pub async fn check_factory(id: &str, ext: &str, target: &str, source: &str, count: usize) {
    check_modes(id, ext, target, source, count, &[ReplaceMode::Factory]).await;
}

async fn check_modes(
    id: &str,
    ext: &str,
    target: &str,
    source: &str,
    count: usize,
    modes: &[ReplaceMode],
) {
    let file = format!("main.{ext}");
    let original = Workspace::new(&[(&file, source)]);
    let before = execute(&original.root(), ext);
    let mut failures = Vec::new();
    for &mode in modes {
        for (policy, apply, force) in [
            ("preview", false, false),
            ("apply", true, false),
            ("force", true, true),
        ] {
            let ws = Workspace::new(&[(&file, source)]);
            let root = ws.root();
            let gateway = ScriptedGateway::start(|method, _| match method {
                "textDocument/references" => json!([]),
                "textDocument/diagnostic" => json!({"kind":"full","items":[{
                    "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                    "severity":1,"message":"scripted transport sentinel; real compilers recorded separately"}]}),
                _ => Value::Null,
            }).await;
            let path = root.join(&file);
            let operation = async {
                match mode {
                    ReplaceMode::Factory => {
                        replace_constructor_with_factory(
                            gateway.addr(),
                            &root,
                            &path,
                            target,
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
                            &path,
                            target,
                            None,
                            apply,
                            force,
                            None,
                        )
                        .await
                    }
                }
            };
            let result = AssertUnwindSafe(operation).catch_unwind().await;
            let result =
                result.unwrap_or_else(|_| Err(anyhow::anyhow!("public operation panicked")));
            let output = fs::read_to_string(&path).unwrap();
            let unchanged = output == source;
            let mut returned = None;
            if let Ok(r) = &result {
                returned = r
                    .rewritten
                    .iter()
                    .find(|(p, _)| Path::new(p) == path)
                    .map(|(_, s)| s.clone());
            }
            // Preview execution uses a separate fixture. Applied operations must
            // execute exactly the bytes written by the public operation itself.
            let preview =
                (!apply).then(|| Workspace::new(&[(&file, returned.as_deref().unwrap_or(source))]));
            let after = execute(&preview.as_ref().map_or(root.clone(), Workspace::root), ext);
            let outcome = result.as_ref().map(|r| json!({"count":r.instantiations_rewritten,"applied":r.applied,
                "blocked":r.blocked,"fields":r.declared_fields,"diagnostics":r.diagnostics}))
                .unwrap_or_else(|e| json!({"error":format!("{e:#}").replace(&*root.to_string_lossy(), "<fixture>")}));
            record(
                &format!("review-4291-{id}-{mode}-{policy}"),
                &json!({
                    "input":bytes(&root,source),"output":bytes(&root,&output),
                    "returned":returned.as_ref().map(|s| bytes(&root,s)),
                    "before":before,"after":after,"public_result":outcome,
                    "apply":apply,"force":force,"unchanged":unchanged,
                    "transport":"scripted empty references (local structural discovery) and nonempty diagnostic sentinel; no analyzer certification"
                }),
            );
            let tag = format!("{id}-{mode}-{policy}");
            if before.iter().any(|r| r["exit"] != 0) {
                failures.push(format!("{tag}: invalid original: {before:?}"));
            }
            let sentinel = result
                .as_ref()
                .err()
                .is_some_and(|e| e.to_string().contains("scripted transport sentinel"));
            if apply && !force && ext != "rs" {
                if !sentinel {
                    failures.push(format!("{tag}: expected diagnostic refusal: {outcome}"));
                }
                if !unchanged {
                    failures.push(format!("{tag}: refusal wrote source"));
                }
                continue;
            }
            match result {
                Err(_) => failures.push(format!("{tag}: {outcome}")),
                Ok(r) => {
                    if r.applied != apply || returned.is_none() {
                        failures.push(format!(
                            "{tag}: wrong apply status or missing returned program: {outcome}"
                        ));
                    }
                    if apply && returned.as_deref() != Some(output.as_str()) {
                        failures.push(format!("{tag}: disk bytes differ from returned edits"));
                    }
                    if apply && !force && !r.diagnostics.is_empty() {
                        failures.push(format!("{tag}: non-forced apply accepted diagnostics"));
                    }
                    if r.instantiations_rewritten != count || !r.blocked.is_empty() {
                        failures.push(format!("{tag}: {outcome}"));
                    }
                    if !apply && !unchanged {
                        failures.push(format!("{tag}: preview wrote source"));
                    }
                    if after.iter().any(|r| r["exit"] != 0)
                        || after.last().unwrap()["stdout"] != before.last().unwrap()["stdout"]
                    {
                        failures.push(format!(
                            "{tag}: changed compiler/runtime behavior: {after:?}"
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

pub async fn checks(cases: &[(&str, &str, &str, &str, usize)]) {
    let mut failed = Vec::new();
    for &(id, ext, target, source, count) in cases {
        if AssertUnwindSafe(check(id, ext, target, source, count))
            .catch_unwind()
            .await
            .is_err()
        {
            failed.push(id);
        }
    }
    assert!(failed.is_empty(), "failed cases: {failed:?}");
}
