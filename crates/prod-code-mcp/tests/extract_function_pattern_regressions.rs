/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::extract_function::{binding::uncaptured_enclosing_locals, extract_function};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

fn missing(body: &str) -> Vec<String> {
    let original = "fn build(p: usize) {\n    let marker = 0;\n    consume(p);\n}";
    let extracted = format!("fn fun_name() {{ {body} }}");
    uncaptured_enclosing_locals(
        original,
        original.find("consume").unwrap(),
        &extracted,
        None,
    )
}

#[test]
fn extract_function_patterns_declared_inside_are_not_captures() {
    for body in [
        "for p in paths { consume(p); }",
        "for (p, _) in paths { consume(p); }",
        "paths.map(|p| consume(p));",
        "return |p| consume(p);",
        "paths.map(|(p, _): (usize, usize)| { consume(p); });",
        "match value { Some(p) if p > 0 => consume(p), _ => () }",
        "match value { Node { field: p } => { consume(p); } _ => () }",
        "if let Some(p) = value { consume(p); }",
        "if let Some(p) = value && p > 0 { consume(p); }",
        "while let Some(p) = values.pop() { consume(p); }",
        "let p; p = 1; consume(p);",
        "let p: usize; p = 1; consume(p);",
        "let p = 1; consume(p);",
        "for p in paths { println!(\"{p}\"); }",
    ] {
        assert!(
            missing(body).is_empty(),
            "false capture in {body}: {:?}",
            missing(body)
        );
    }
}

#[test]
fn extract_function_pattern_scopes_do_not_hide_real_captures() {
    for body in [
        "for p in paths { consume(p); } consume(p);",
        "paths.map(|p| consume(p)); consume(p);",
        "match value { Some(p) => consume(p), _ => consume(p) }",
        "if let Some(p) = value { consume(p); } else { consume(p); }",
        "for p in p { consume(p); }",
        "if let Some(p) = p { consume(p); }",
        "if let Some(p) = p && p > 0 { consume(p); }",
        "let p = p; consume(p);",
        "for p in paths { consume(p); } println!(\"{p}\");",
    ] {
        assert_eq!(missing(body), vec!["p"], "missed capture in {body}");
    }
}

// The source shape in #1146: a loop binding and a borrowed local reused after selection.
const SOURCE: &str = r#"use std::{path::{Path, PathBuf}, process::Command};
struct RustcInvocation<'a> { paths: &'a [PathBuf], package: &'a Path }
fn build(inv: &RustcInvocation<'_>, clean_crate: &String, cmd: &mut Command) {
    let p = "earlier binding";
    cmd.arg(p);
    for p in inv.paths {
        cmd.arg(p);
    }
    let pkg_dir = inv.package;
    cmd.arg(clean_crate).arg(pkg_dir);
    cmd.current_dir(pkg_dir);
}
"#;

const PROPOSED: &str = r#"use std::{path::{Path, PathBuf}, process::Command};
struct RustcInvocation<'a> { paths: &'a [PathBuf], package: &'a Path }
fn build(inv: &RustcInvocation<'_>, clean_crate: &String, cmd: &mut Command) {
    let p = "earlier binding";
    cmd.arg(p);
    let pkg_dir = fun_name(inv, clean_crate, cmd);
    cmd.current_dir(pkg_dir);
}

fn fun_name(inv: &RustcInvocation<'_>, clean_crate: &String, cmd: &mut Command) -> &Path {
    for p in inv.paths {
        cmd.arg(p);
    }
    let pkg_dir = inv.package;
    cmd.arg(clean_crate).arg(pkg_dir);
    pkg_dir
}
"#;

#[tokio::test]
async fn extract_function_ambiguous_borrowed_return_is_refused_before_writing() {
    let ws = Workspace::new(&[("src/lib.rs", SOURCE)]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let target = file.clone();
    // An empty analyzer report must not allow an invalid lifetime through the wrapper.
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "prodCode/applyAssist" => answers::whole_file(&target, SOURCE, PROPOSED),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let result = extract_function(
        gateway.addr(),
        &root,
        &file,
        (6, 5),
        (11, 1),
        "append_path_remaps",
        false,
        false,
        false,
    )
    .await;
    let error = result
        .expect_err("ambiguous returned borrow must be refused")
        .to_string();
    assert!(error.contains("lifetime"), "{error}");
    assert!(
        error.contains("pkg_dir") || error.contains("borrowed value"),
        "{error}"
    );
    assert!(
        error.contains("input") && error.contains("selection"),
        "{error}"
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), SOURCE);
}

#[tokio::test]
async fn extract_function_loop_local_with_explicit_borrowed_return_is_accepted() {
    let ws = Workspace::new(&[("src/lib.rs", SOURCE)]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let target = file.clone();
    let proposed = PROPOSED.replace(
        "fn fun_name(inv: &RustcInvocation<'_>, clean_crate: &String, cmd: &mut Command) -> &Path",
        "fn fun_name<'a>(inv: &RustcInvocation<'a>, clean_crate: &String, cmd: &mut Command) -> &'a Path",
    );
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "prodCode/applyAssist" => answers::whole_file(&target, SOURCE, &proposed),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let mut result = extract_function(
        gateway.addr(),
        &root,
        &file,
        (6, 5),
        (11, 1),
        "append_path_remaps",
        false,
        false,
        false,
    )
    .await
    .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    result.write(false).unwrap();
    let written = std::fs::read_to_string(file).unwrap();
    assert!(written.contains("fn append_path_remaps<'a>"), "{written}");
    assert!(written.contains("for p in inv.paths"), "{written}");
}

#[tokio::test]
async fn extract_function_control_flow_refusal_keeps_the_construct_and_line() {
    let source = "fn f() { for i in 0..3 { if i == 1 { break; } continue; } }\n";
    let ws = Workspace::new(&[("src/lib.rs", source)]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let reason = "cannot extract function: `break` at line 1, `continue` at line 1 cross the selection boundary; extracting mixed loop exits is not supported. Select the enclosing loop or move the exits outside the selection.";
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "prodCode/applyAssist" => answers::failure(reason),
        _ => serde_json::Value::Null,
    })
    .await;
    let start = (1, source.find("if i").unwrap() as u32 + 1);
    let end = (1, source.rfind(" } }").unwrap() as u32 + 1);
    let error = extract_function(
        gateway.addr(),
        &root,
        &file,
        start,
        end,
        "candidate",
        false,
        false,
        false,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.starts_with("extract function failed:"), "{error}");
    assert!(error.ends_with(reason), "{error}");
    assert!(!error.contains("available:"), "{error}");
    assert_eq!(
        gateway.calls(),
        1,
        "a refusal must stop after the assist request"
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), source);
}
