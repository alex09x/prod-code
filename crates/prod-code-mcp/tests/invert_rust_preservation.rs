/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "invert_rust_preservation/fixtures.rs"]
mod fixtures;
#[path = "invert_rust_preservation/remaining.rs"]
mod remaining;

use prod_code_mcp::invert_boolean::invert;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::{fs, path::Path, process::Command};

const MANIFEST: &str =
    "[package]\nname = \"preservation\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";

fn reference(file: &Path, text: &str, at: usize) -> serde_json::Value {
    let (line, col) = prod_code_mcp::signature::position_at(text, at).unwrap();
    serde_json::json!({
        "uri": url::Url::from_file_path(file).unwrap().to_string(),
        "range": { "start": { "line": line - 1, "character": col - 1 } }
    })
}

async fn gateway(file: &Path, text: &str) -> ScriptedGateway {
    // Only real, selected references: never the same-named unrelated method or literal text.
    let client = text.find("pub fn callers").unwrap();
    let refs: Vec<_> = text[client..]
        .match_indices("is_ready")
        .map(|(at, _)| reference(file, text, client + at))
        .collect();
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => serde_json::json!(refs),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

fn compile_run(root: &Path, name: &str, generic: bool, phase: &str) -> String {
    let suffix = if generic { "::<Vec<Option<u8>>>" } else { "" };
    let harness = format!(
        "mod selected;\nfn main() {{\nfor n in [-1, 0, 1] {{ println!(\"selected={{}} callers={{:?}}\", selected::{name}{suffix}(n), selected::callers(n)); }}\n}}\n"
    );
    fs::write(root.join("main.rs"), harness).unwrap();
    println!(
        "{phase} selected.rs:\n{}\n{phase} main.rs:\n{}",
        fs::read_to_string(root.join("selected.rs")).unwrap(),
        fs::read_to_string(root.join("main.rs")).unwrap()
    );
    let output = Command::new("rustc")
        .current_dir(root)
        .args(["--edition=2024", "-Awarnings", "main.rs", "-o", "probe"])
        .output()
        .unwrap();
    println!(
        "{phase} rustc --edition=2024 -Awarnings main.rs -o probe: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(root.join("probe")).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    println!("{phase} ./probe: {}\n{stdout}", output.status);
    stdout
}

async fn preserves(label: &str, source: &str, generic: bool, unchanged: &[&str]) {
    let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("selected.rs", source)]);
    let root = ws.root();
    let file = root.join("selected.rs");
    let before = compile_run(&root, "is_ready", generic, &format!("{label} BEFORE"));
    let gw = gateway(&file, source).await;
    let at = source.find("fn is_ready").unwrap() + 3;
    let (line, col) = prod_code_mcp::signature::position_at(source, at).unwrap();
    let plan = invert(
        gw.addr(),
        &root,
        &file,
        line,
        col,
        "is_unready",
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        source,
        "planning writes nothing"
    );
    println!(
        "{label} PLAN unmatched={:?}\n{}",
        plan.unmatched, plan.rewritten[0].1
    );
    assert!(plan.unmatched.is_empty(), "{:?}", plan.unmatched);
    let applied = invert(
        gw.addr(),
        &root,
        &file,
        line,
        col,
        "is_unready",
        true,
        false,
    )
    .await
    .unwrap();
    assert!(applied.applied);
    let transformed = fs::read_to_string(&file).unwrap();
    assert_eq!(transformed, plan.rewritten[0].1);
    // Compile and execute before checking text: canned diagnostics cannot make this pass.
    let after = compile_run(&root, "is_unready", generic, &format!("{label} AFTER"));
    let expected = before
        .lines()
        .map(|line| {
            if let Some(rest) = line.strip_prefix("selected=true ") {
                format!("selected=false {rest}")
            } else if let Some(rest) = line.strip_prefix("selected=false ") {
                format!("selected=true {rest}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert_eq!(
        after, expected,
        "only the selected predicate changes meaning"
    );
    for text in unchanged {
        assert!(
            transformed.contains(text),
            "lost exact bytes: {text:?}\n{transformed}"
        );
    }
}

#[tokio::test]
async fn raw_literals_retain_exact_bytes_and_runtime_output() {
    preserves("RAW", fixtures::RAW, false, &[fixtures::RAW_LITERAL]).await;
}

#[tokio::test]
async fn multiline_nested_functions_closures_and_async_own_their_returns() {
    preserves("NESTED", fixtures::NESTED, false, &[fixtures::HELPER]).await;
}

#[tokio::test]
async fn borrowed_closures_and_struct_fields_own_their_returns() {
    preserves(
        "BORROWED",
        fixtures::BORROWED,
        false,
        &["|| { return n >= 0; }"],
    )
    .await;
}

#[tokio::test]
async fn operator_rhs_returns_still_belong_to_the_selected_function() {
    preserves("OPERATORS", fixtures::OPERATORS, false, &[]).await;
}

#[tokio::test]
async fn turbofish_calls_preserve_types_boundaries_and_external_meaning() {
    preserves(
        "GENERIC",
        fixtures::GENERIC,
        true,
        &["::<Vec<Option<u8>>>", "::<fn() -> bool>"],
    )
    .await;
}

#[tokio::test]
async fn regression_break_value_closures_keep_their_returns() {
    preserves(
        "BREAKS",
        fixtures::BREAKS,
        false,
        &["|| { return n >= 0; }", "|| return n >= 0"],
    )
    .await;
}

#[tokio::test]
async fn regression_match_arm_returns_stop_before_separator() {
    preserves("MATCH", fixtures::MATCH_ARMS, false, &[]).await;
}

#[tokio::test]
async fn remaining_negated_qualified_path_keeps_generic_commas() {
    preserves("QPATH", remaining::QUALIFIED, false, &[]).await;
}

#[tokio::test]
async fn remaining_operator_operand_closures_own_their_returns() {
    preserves(
        "OPERAND",
        remaining::OPERAND,
        false,
        &["|| { return n >= 0; }", "|x: i32| { return x >= 0; }"],
    )
    .await;
}

#[tokio::test]
async fn remaining_function_type_macro_is_not_the_body() {
    preserves("FN_MACRO", remaining::FUNCTION_MACRO, false, &[]).await;
}

#[tokio::test]
async fn remaining_closure_type_macro_is_not_the_body() {
    preserves("CLOSURE_MACRO", remaining::CLOSURE_MACRO, false, &[]).await;
}

#[tokio::test]
async fn remaining_const_generic_values_before_pipes_keep_owned_returns() {
    preserves("GENERIC_PIPE", remaining::GENERIC_PIPE, false, &[]).await;
}

#[tokio::test]
async fn generic_function_values_still_block_apply_even_with_force() {
    let source = format!(
        "{}\npub const POINTER: fn(i32) -> bool = is_ready::<u8>;\n",
        fixtures::GENERIC
    );
    let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("selected.rs", &source)]);
    let file = ws.root().join("selected.rs");
    let gw = gateway(&file, &source).await;
    let at = source.find("fn is_ready").unwrap() + 3;
    let (line, col) = prod_code_mcp::signature::position_at(&source, at).unwrap();
    let plan = invert(
        gw.addr(),
        &ws.root(),
        &file,
        line,
        col,
        "is_unready",
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(plan.unmatched.len(), 1, "{:?}", plan.unmatched);
    for force in [false, true] {
        let error = invert(
            gw.addr(),
            &ws.root(),
            &file,
            line,
            col,
            "is_unready",
            true,
            force,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("nothing was written"), "{error}");
        assert_eq!(fs::read_to_string(&file).unwrap(), source);
    }
}
