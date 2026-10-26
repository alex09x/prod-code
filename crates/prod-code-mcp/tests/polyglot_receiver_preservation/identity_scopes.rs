/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::to_method::convert_to_method_polyglot;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::process::Command;

fn output(command: &mut Command, argv: &[&str]) -> Value {
    let result = command.output().expect("required native tool");
    json!({"argv": argv, "exit": result.status.code(),
        "stdout": String::from_utf8_lossy(&result.stdout),
        "stderr": String::from_utf8_lossy(&result.stderr)})
}

fn native(extension: &str, source: &str) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let filename = format!("case.{extension}");
    std::fs::write(dir.path().join(&filename), source).unwrap();
    if extension == "cpp" {
        let compiler = output(
            Command::new("c++").current_dir(dir.path()).args([
                "-std=c++17",
                &filename,
                "-o",
                "case",
            ]),
            &["c++", "-std=c++17", &filename, "-o", "case"],
        );
        let runtime = (compiler["exit"] == 0).then(|| {
            output(
                Command::new(dir.path().join("case")).current_dir(dir.path()),
                &["./case"],
            )
        });
        json!({"compiler": compiler, "runtime": runtime})
    } else {
        let program = if extension == "py" { "python3" } else { "node" };
        let runtime = output(
            Command::new(program).current_dir(dir.path()).arg(&filename),
            &[program, &filename],
        );
        let compiler = (extension == "ts").then(|| {
            output(
                Command::new("tsc").current_dir(dir.path()).args([
                    "--strict", "--noEmit", "--target", "esnext", "--module", "esnext", &filename,
                ]),
                &[
                    "tsc", "--strict", "--noEmit", "--target", "esnext", "--module", "esnext",
                    &filename,
                ],
            )
        });
        json!({"compiler": compiler, "runtime": runtime})
    }
}

fn valid(record: &Value, expected: &str) -> bool {
    (record["compiler"].is_null() || record["compiler"]["exit"] == 0)
        && record["runtime"]["exit"] == 0
        && record["runtime"]["stdout"] == expected
}

#[tokio::test]
async fn review28_receiver_identity_and_signature_scopes() {
    let cases: Value = serde_json::from_str(include_str!(
        "../../../../evidence/codex-review-audit/issue-1317/repair28/cases.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in cases.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let ext = case["extension"].as_str().unwrap();
        let before = case["before"].as_str().unwrap();
        let expected = case["expected_stdout"].as_str().unwrap();
        let original = native(ext, before);
        if !valid(&original, expected) {
            failures.push(format!("{name}: invalid original: {original}"));
        }
        let file = format!("case.{ext}");
        let caller_file = format!("caller.{ext}");
        let caller = if ext == "py" {
            "# untouched\n"
        } else {
            "// untouched\n"
        };
        let ws = Workspace::new(&[(&file, before), (&caller_file, caller)]);
        let path = ws.root().join(&file);
        let gateway = ScriptedGateway::start(|method, _| match method {
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        })
        .await;
        for force in [false, true] {
            let result = convert_to_method_polyglot(
                gateway.addr(),
                &ws.root(),
                &path,
                Some("C"),
                "same",
                true,
                force,
            )
            .await;
            let after = result.as_ref().ok().map(|r| r.rewritten[0].1.clone());
            let returned = after.as_deref().map(|s| native(ext, s));
            let selected_unchanged = std::fs::read_to_string(&path).unwrap() == before;
            let caller_unchanged =
                std::fs::read_to_string(ws.root().join(&caller_file)).unwrap() == caller;
            eprintln!(
                "REPAIR28_SOURCE {}",
                json!({"name": name, "extension": ext,
                "line": case["line"], "column": case["column"], "force": force,
                "before": before, "after": after, "original": original,
                "returned": returned, "error": result.as_ref().err().map(ToString::to_string),
                "selected_unchanged": selected_unchanged, "caller_unchanged": caller_unchanged,
                "analyzer": "scripted empty diagnostics; native checks recorded separately"})
            );
            if case["refuse"] == true {
                if !result
                    .as_ref()
                    .err()
                    .is_some_and(|e| e.to_string().contains("unsupported"))
                    || !selected_unchanged
                    || !caller_unchanged
                {
                    failures.push(format!("{name}/{force}: expected atomic refusal"));
                }
            } else if !returned.as_ref().is_some_and(|r| valid(r, expected)) {
                failures.push(format!("{name}/{force}: conversion/native result failed"));
            } else if let Some(literal) = case["literal"].as_str()
                && !after.as_ref().unwrap().contains(literal)
            {
                failures.push(format!("{name}/{force}: literal/comment bytes changed"));
            }
            std::fs::write(&path, before).unwrap();
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
