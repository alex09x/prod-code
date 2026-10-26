/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{convert, run};
use prod_code_mcp::to_method::convert_to_method_polyglot;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

#[tokio::test]
async fn review23_swift_prefix_slash_followed_by_division() {
    for (name, gap) in [
        ("prefix_division_space", " "),
        ("prefix_division_tab", "\t"),
    ] {
        let before = format!(
            "prefix operator /+\nprefix func /+ (rhs: Int) -> Int {{ rhs + 1 }}\nclass C {{\n    var value = 5\n    static func same(c: C) -> Int {{ return /+c.value{gap}/ 2 }}\n}}\nprint(C.same(c: C()))\n"
        );
        let (after, _) = convert(&before, "swift", true).await;
        eprintln!(
            "REMAINING_SOURCE {}",
            serde_json::json!({
                "name": name, "extension": "swift", "before": before,
                "after": after, "expected": "3\n"
            })
        );
        assert!(
            after.contains(&format!("return /+self.value{gap}/ 2")),
            "{after}"
        );
    }
}

#[tokio::test]
async fn review23_swift_regex_whitespace_controls() {
    for (name, regex) in [
        ("bare_escaped_space", r"/c.value\ /"),
        ("bare_space_class", "/c.value[ ]/"),
        ("extended_trailing_space", "#/c.value /#"),
    ] {
        let before = format!(
            "class C {{\n    static func same(c: C) -> Regex<Substring> {{ return {regex} }}\n}}\nprint(\"c.value \".wholeMatch(of: C.same(c: C())) != nil)\n"
        );
        let (after, _) = convert(&before, "swift", true).await;
        eprintln!(
            "REMAINING_SOURCE {}",
            serde_json::json!({
                "name": name, "extension": "swift", "before": before,
                "after": after, "expected": "true\n"
            })
        );
        assert!(after.contains(&format!("return {regex}")), "{after}");
    }
}

#[tokio::test]
async fn review23_typescript_receiver_query_refuses_atomically() {
    let mut accepted = Vec::new();
    for annotation in [
        "{ value: typeof c.value }",
        "() => typeof c.value",
        "{ read: () => { value: typeof c.value } }",
        "Array<() => typeof /* query */ c.value>",
        r"{ value: typeof \u0063.value }",
        "{ value: `value-${typeof c.value}` }",
    ] {
        let before = format!(
            "class C {{\n    value = 7;\n    static same(c: C): {annotation} {{ throw Error('stop'); }}\n}}\n"
        );
        let ws = Workspace::new(&[("case.ts", &before), ("caller.txt", "untouched")]);
        let gateway = ScriptedGateway::start(|method, _| match method {
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        })
        .await;
        for force in [false, true] {
            let path = ws.root().join("case.ts");
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
            eprintln!(
                "REMAINING_SOURCE {}",
                serde_json::json!({
                    "name": annotation, "extension": "ts", "force": force,
                    "before": before, "after": result.as_ref().ok().map(|r| &r.rewritten[0].1),
                    "error": result.as_ref().err().map(ToString::to_string)
                })
            );
            match result {
                Ok(value) => {
                    accepted.push(value.rewritten[0].1.clone());
                    std::fs::write(&path, &before).unwrap();
                }
                Err(error) => {
                    assert!(
                        error
                            .to_string()
                            .contains("unsupported TypeScript result type query"),
                        "{error}"
                    );
                    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
                    assert_eq!(
                        std::fs::read_to_string(ws.root().join("caller.txt")).unwrap(),
                        "untouched"
                    );
                }
            }
        }
    }
    assert!(
        accepted.is_empty(),
        "accepted dependent result annotations: {accepted:#?}"
    );
}

#[tokio::test]
async fn review23_typescript_independent_result_queries_stay_supported() {
    for annotation in [
        "{ value: number }",
        "() => number",
        "{ value: typeof other.value }",
        "{ read: () => typeof other.value }",
        "{ c: number; typeof: number }",
        "{ value: 'typeof c.value' }",
    ] {
        let before = format!(
            "const other = {{ value: 7 }};\nclass C {{\n    value = 7;\n    static same(c: C): {annotation} {{ throw Error('stop'); }}\n}}\nconsole.log('ok');\n"
        );
        let (after, _) = convert(&before, "ts", true).await;
        assert!(after.contains(&format!("same(): {annotation}")), "{after}");
        assert_eq!(run("node", "ts", &after), run("node", "ts", &before));
    }
}

fn node_check(extension: &str, source: &str) -> (i32, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(format!("case.{extension}"));
    std::fs::write(&file, source).unwrap();
    let output = std::process::Command::new("node")
        .arg(&file)
        .output()
        .unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[tokio::test]
async fn review27_escaped_ts_and_swift_unbalanced_paren_repairs() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../evidence/codex-review-audit/issue-1317/repair27/cases.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in fixture["typescript"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let ext = case["extension"].as_str().unwrap();
        let before = case["before"].as_str().unwrap();
        let refuse = case["refuse"].as_bool().unwrap();
        let expected_stdout = case["expected_stdout"].as_str().unwrap();
        let line = case["line"].as_u64().unwrap();
        let column = case["column"].as_u64().unwrap();
        let (orig_exit, orig_out, orig_err) = node_check(ext, before);
        if orig_exit != 0 || orig_out != expected_stdout {
            failures.push(format!(
                "{name} original runtime ({orig_exit}: {orig_out:?} / {orig_err})"
            ));
        }
        let file = format!("case.{ext}");
        let caller_file = format!("caller.{ext}");
        let caller = "export const untouched = 'caller';\n";
        let ws = Workspace::new(&[(&file, before), (&caller_file, caller)]);
        let path = ws.root().join(&file);
        let gateway = ScriptedGateway::start(|method, _| match method {
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
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
            let renamed_uses = result.as_ref().ok().map(|r| r.renamed_uses);
            let rewritten_calls = result.as_ref().ok().map(|r| r.rewritten_calls);
            let error = result.as_ref().err().map(ToString::to_string);
            let selected_unchanged = std::fs::read_to_string(&path).unwrap() == before;
            let caller_unchanged =
                std::fs::read_to_string(ws.root().join(&caller_file)).unwrap() == caller;
            let runtime = after.as_deref().map(|code| node_check(ext, code));
            eprintln!(
                "REPAIR27_TS_SOURCE {}",
                serde_json::json!({
                    "name": name, "extension": ext, "before": before, "after": after,
                    "error": error, "force": force, "refuse": refuse,
                    "renamed_uses": renamed_uses, "rewritten_calls": rewritten_calls,
                    "selected_unchanged": selected_unchanged, "caller_unchanged": caller_unchanged,
                    "original_runtime": {"exit": orig_exit, "stdout": orig_out},
                    "returned_runtime": runtime.as_ref().map(|(c, out, err)| {
                        serde_json::json!({"exit": c, "stdout": out, "stderr": err})
                    }),
                    "line": line, "column": column,
                    "analyzer": "scripted empty diagnostics; native tsc checked separately"
                })
            );
            if refuse {
                let expected_error = case["expected_error"].as_str().unwrap();
                if !error.as_ref().is_some_and(|e| e.contains(expected_error))
                    || !selected_unchanged
                    || !caller_unchanged
                {
                    failures.push(format!("{name} force={force}: expected atomic refusal"));
                }
            } else if let Some(after) = after {
                let fragment = case["expected_fragment"].as_str().unwrap();
                let (ret_exit, ret_out, ret_err) = runtime.unwrap();
                if !after.contains(fragment)
                    || std::fs::read_to_string(&path).unwrap() != after
                    || !caller_unchanged
                    || ret_exit != 0
                    || ret_out != expected_stdout
                {
                    failures.push(format!(
                        "{name} force={force}: bad output or runtime ({ret_exit}: {ret_out:?} / {ret_err})"
                    ));
                }
            } else {
                failures.push(format!("{name} force={force}: {error:?}"));
            }
            std::fs::write(&path, before).unwrap();
            std::fs::write(ws.root().join(&caller_file), caller).unwrap();
        }
    }
    for case in fixture["swift"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let before = case["before"].as_str().unwrap();
        let expected = case["expected"].as_str().unwrap();
        let fragment = case["expected_fragment"].as_str().unwrap();
        let caller_frag = case["expected_caller"].as_str().unwrap();
        let line = case["line"].as_u64().unwrap();
        let column = case["column"].as_u64().unwrap();
        let (after, result) = convert(before, "swift", true).await;
        eprintln!(
            "REPAIR27_SWIFT_SOURCE {}",
            serde_json::json!({
                "name": name, "extension": "swift", "before": before, "after": after,
                "expected": expected, "renamed_uses": result.renamed_uses,
                "rewritten_calls": result.rewritten_calls, "line": line, "column": column,
                "analyzer": "scripted empty diagnostics; native macOS swiftc checked separately"
            })
        );
        if !after.contains(fragment) || !after.contains(caller_frag) {
            failures.push(format!("{name}: unexpected Swift output:\n{after}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
