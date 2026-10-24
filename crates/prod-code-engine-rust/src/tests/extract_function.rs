/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::RustEngine;
use std::{path::Path, process::Command};

struct Case {
    name: &'static str,
    body: &'static str,
    output: &'static str,
    refusal: Option<(&'static str, &'static str)>,
}

fn run(root: &Path, source: &str) -> String {
    let file = root.join("run.rs");
    let binary = root.join("run");
    std::fs::write(&file, source).unwrap();
    let compiled = Command::new("rustc")
        .args(["--edition=2021", "-Awarnings"])
        .arg(&file)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}\n{source}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let result = Command::new(binary).output().unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn extract_function_loop_control_flow_table() {
    const MIXED: &str = "cross the selection boundary; extracting mixed loop exits is not supported. Select the enclosing loop or move the exits outside the selection.";
    const OUTER: &str = "targets an outer loop or labelled block outside the selection; rust-analyzer cannot preserve this labelled exit. Select its target together with the body or move the exit outside the selection.";
    let cases = [
        Case {
            name: "break",
            body: "if i == 3 { break; }\n        total += i;",
            output: "Ok(3)\n",
            refusal: None,
        },
        Case {
            name: "continue",
            body: "if i == 3 { continue; }\n        total += i;",
            output: "Ok(7)\n",
            refusal: None,
        },
        Case {
            name: "break_continue",
            body: "if i == 1 { continue; }\n        if i == 3 { break; }\n        total += i;",
            output: "Ok(2)\n",
            refusal: Some(("`continue` at line 5, `break` at line 6", MIXED)),
        },
        Case {
            name: "let_else_break",
            body: "let Some(value) = (if i == 3 { None } else { Some(i) }) else { break; };\n        total += value;",
            output: "Ok(3)\n",
            refusal: None,
        },
        Case {
            name: "try_break",
            body: "if i == 3 { break; }\n        total += step(i)?;",
            output: "Err(99)\n",
            refusal: Some(("`break` at line 5, `?` at line 6", MIXED)),
        },
        Case {
            name: "outer_label",
            body: "for _ in 0..1 {\n            if i == 3 { break 'outer; }\n        }\n        total += i;",
            output: "Ok(3)\n",
            refusal: Some(("`break 'outer` at line 6", OUTER)),
        },
        Case {
            name: "outer_continue",
            body: "for _ in 0..1 {\n            if i == 3 { continue 'outer; }\n        }\n        total += i;",
            output: "Ok(7)\n",
            refusal: Some(("`continue 'outer` at line 6", OUTER)),
        },
        Case {
            name: "return",
            body: "if i == 3 { return Ok(total + 10); }\n        total += i;",
            output: "Ok(13)\n",
            refusal: None,
        },
        Case {
            name: "reported",
            body: "let Some(value) = (if i == 4 { None } else { Some(i) }) else { break; };\n        if value == 1 { continue; }\n        total += step(value)?;",
            output: "Err(99)\n",
            refusal: Some((
                "`break` at line 5, `continue` at line 6, `?` at line 7",
                MIXED,
            )),
        },
        Case {
            name: "continue_try",
            body: "if i == 1 { continue; }\n        total += step(i)?;",
            output: "Err(99)\n",
            refusal: Some(("`continue` at line 5, `?` at line 6", MIXED)),
        },
        Case {
            name: "return_break",
            body: "if i == 1 { return Ok(total); }\n        if i == 3 { break; }\n        total += i;",
            output: "Ok(0)\n",
            refusal: Some(("`return` at line 5, `break` at line 6", MIXED)),
        },
        Case {
            name: "label_same_loop",
            body: "if i == 3 { break 'outer; }\n        total += i;",
            output: "Ok(3)\n",
            refusal: None,
        },
        Case {
            name: "local_loop",
            body: "for j in 0..3 { if j == 0 { continue; } if j == 2 { break; } total += j; }\n        if i == 3 { break; }",
            output: "Ok(4)\n",
            refusal: None,
        },
        Case {
            name: "local_closure_try_return",
            body: "let value = (|| -> Result<i32, i32> { if i == 1 { return Ok(10); } Ok(step(i)?) })().unwrap_or(0);\n        if i == 3 { break; }\n        total += value;",
            output: "Ok(10)\n",
            refusal: Some((
                "`return` at line 5",
                "belongs to a closure inside the selection; rust-analyzer cannot preserve this nested return while transporting loop exits. Move the nested callable outside the selection or select the enclosing loop.",
            )),
        },
        Case {
            name: "local_async_try_return",
            body: "let _future = async { if i == 1 { return Ok::<i32, i32>(10); } Ok(step(i)?) };\n        if i == 3 { break; }\n        total += i;",
            output: "Ok(3)\n",
            refusal: Some((
                "`return` at line 5",
                "belongs to an async or generator block inside the selection; rust-analyzer cannot preserve this nested return while transporting loop exits. Move the nested callable outside the selection or select the enclosing loop.",
            )),
        },
        Case {
            name: "local_closure_try",
            body: "let value = (|| -> Result<i32, i32> { Ok(step(i)?) })().unwrap_or(0);\n        if i == 3 { break; }\n        total += value;",
            output: "Ok(1)\n",
            refusal: None,
        },
        Case {
            name: "local_label",
            body: "'inner: for j in 0..3 { if j == 2 { break 'inner; } total += j; }\n        if i == 3 { break; }",
            output: "Ok(4)\n",
            refusal: None,
        },
        Case {
            name: "comments_and_strings",
            body: "let _message = \"continue return ?\"; // continue; return; ?\n        if i == 3 { break; }\n        total += i;",
            output: "Ok(3)\n",
            refusal: None,
        },
    ];
    let mut failures = Vec::new();
    let temp = tempfile::tempdir().unwrap();
    super::configure_test_workspace(temp.path());
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir(temp.path().join("src")).unwrap();
    let mut engine = None;
    for case in cases {
        let source = format!(
            "fn step(i: i32) -> Result<i32, i32> {{ if i == 2 {{ Err(99) }} else {{ Ok(i) }} }}\nfn calculate() -> Result<i32, i32> {{\n    let mut total = 0;\n    'outer: for i in 0..5 {{\n        {}\n    }}\n    Ok(total)\n}}\nfn main() {{ println!(\"{{:?}}\", calculate()); }}\n",
            case.body
        );
        assert_eq!(
            run(temp.path(), &source),
            case.output,
            "{} before",
            case.name
        );
        let file = temp.path().join("src/main.rs");
        std::fs::write(&file, &source).unwrap();
        let engine = engine.get_or_insert_with(|| RustEngine::load(temp.path()).unwrap());
        engine.apply_file_change(&file, source.clone()).unwrap();
        let end_line = 5 + case.body.lines().count() as u32;
        let result = engine
            .apply_assist(&file, 5, 9, Some((end_line, 1)), "extract_function", None)
            .unwrap();
        eprintln!(
            "{}: {}",
            case.name,
            result.as_ref().map(|_| "extracted").unwrap_or_else(|e| e)
        );
        match (case.refusal, result) {
            (None, Ok(outcome)) => {
                assert_eq!(outcome.files.len(), 1);
                assert_eq!(
                    run(temp.path(), &outcome.files[0].new_text),
                    case.output,
                    "{} after",
                    case.name
                );
            }
            (Some((constructs, reason)), Err(error))
                if error == format!("cannot extract function: {constructs} {reason}") =>
            {
                assert_eq!(
                    run(temp.path(), &source),
                    case.output,
                    "{} refused",
                    case.name
                );
            }
            (expected, actual) => failures.push(format!(
                "{}: expected {expected:?}, got {actual:?}",
                case.name
            )),
        }
        assert_eq!(std::fs::read_to_string(file).unwrap(), source);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
