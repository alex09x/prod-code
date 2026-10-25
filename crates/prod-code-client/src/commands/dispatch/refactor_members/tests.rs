/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The tool and the arguments each member-level refactoring command sends, for every flag
//! the command line takes, and the refusals made before anything is sent.

use super::*;
use crate::commands::dispatch::tests::{on_big_stack, parse};
use serde_json::{Value, json};

fn build(args: &[&str]) -> Result<(Runner, &'static str, Value)> {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    on_big_stack(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        tool_call(parse(&args))
    })
}

fn call(args: &[&str]) -> (Runner, &'static str, Value) {
    build(args).expect("the command builds a tool call")
}

fn refusal(args: &[&str]) -> String {
    build(args).expect_err("the command is refused").to_string()
}

#[test]
fn extract_and_introduce_send_the_selection_through_run_tool() {
    let cases: Vec<(Vec<&str>, &str, Value)> = vec![
        (
            vec![
                "extract-function",
                "src/a.rs",
                "3",
                "5",
                "--to",
                "4:9",
                "--name",
                "helper",
            ],
            "code_extract_function",
            json!({
                "path": "src/a.rs", "line": 3, "character": 5, "end_line": 4,
                "end_character": 9, "name": "helper", "duplicates": true,
                "parameterize": false, "other_files": false, "apply": false, "force": false,
            }),
        ),
        (
            vec![
                "extract-function",
                "src/a.rs",
                "3",
                "5",
                "--to",
                "4:9",
                "--name",
                "helper",
                "--no-duplicates",
                "--parameterize",
                "--other-files",
                "--verify",
                "compile",
                "--apply",
                "--force",
            ],
            "code_extract_function",
            json!({
                "path": "src/a.rs", "line": 3, "character": 5, "end_line": 4,
                "end_character": 9, "name": "helper", "duplicates": false,
                "parameterize": true, "other_files": true, "apply": true, "force": true,
                "verify": "compile",
            }),
        ),
        (
            vec![
                "introduce-variable",
                "src/a.rs",
                "7",
                "2",
                "--to",
                "7:19",
                "--name",
                "total",
                "--apply",
            ],
            "code_introduce_variable",
            json!({
                "path": "src/a.rs", "line": 7, "character": 2, "end_line": 7,
                "end_character": 19, "name": "total", "apply": true, "force": false,
            }),
        ),
    ];
    for (argv, tool, args) in cases {
        let got = call(&argv);
        assert_eq!(got, (Runner::Tool, tool, args), "{argv:?}");
    }
}

#[test]
fn selection_end_must_be_line_colon_col() {
    for to in ["12", "4:x", "a:4", ":", "4:9:1", " 4 : 9"] {
        for command in ["extract-function", "introduce-variable"] {
            let argv = [command, "src/a.rs", "1", "1", "--to", to, "--name", "n"];
            assert_eq!(
                refusal(&argv),
                "--to takes LINE:COL",
                "{command} --to {to:?}"
            );
        }
    }
}

#[test]
fn constructor_replacements_name_the_factory_or_builder() {
    let cases: Vec<(Vec<&str>, &str, Value)> = vec![
        (
            vec![
                "replace-constructor-with-factory",
                "src/p.rs",
                "--type-name",
                "Point",
            ],
            "code_replace_constructor_with_factory",
            json!({"path": "src/p.rs", "type_name": "Point", "apply": false, "force": false}),
        ),
        (
            vec![
                "replace-constructor-with-factory",
                "src/p.rs",
                "--type-name",
                "Point",
                "--name",
                "origin",
                "--verify",
                "compile",
                "--apply",
                "--force",
            ],
            "code_replace_constructor_with_factory",
            json!({
                "path": "src/p.rs", "type_name": "Point", "apply": true, "force": true,
                "factory_name": "origin", "verify": "compile",
            }),
        ),
        (
            vec![
                "replace-constructor-with-builder",
                "src/p.rs",
                "--type-name",
                "Point",
            ],
            "code_replace_constructor_with_builder",
            json!({"path": "src/p.rs", "type_name": "Point", "apply": false, "force": false}),
        ),
        (
            vec![
                "replace-constructor-with-builder",
                "src/p.rs",
                "--type-name",
                "Point",
                "--name",
                "PointMaker",
                "--verify",
                "compile",
                "--apply",
            ],
            "code_replace_constructor_with_builder",
            json!({
                "path": "src/p.rs", "type_name": "Point", "apply": true, "force": false,
                "builder_name": "PointMaker", "verify": "compile",
            }),
        ),
    ];
    for (argv, tool, args) in cases {
        assert_eq!(call(&argv), (Runner::Exec, tool, args), "{argv:?}");
    }
}

#[test]
fn pull_up_and_push_down_carry_their_member_lists() {
    let cases: Vec<(Vec<&str>, &str, Value)> = vec![
        (
            vec!["pull-up", "src/s.rs", "--class", "Sub", "--members", "a,b"],
            "code_pull_up",
            json!({
                "path": "src/s.rs", "class_name": "Sub", "members": ["a", "b"],
                "clean_siblings": true, "apply": false, "force": false,
            }),
        ),
        (
            vec![
                "pull-up",
                "src/s.rs",
                "--class",
                "Sub",
                "--members",
                "a",
                "--target-class",
                "Base",
                "--no-clean-siblings",
                "--verify",
                "compile",
                "--apply",
                "--force",
            ],
            "code_pull_up",
            json!({
                "path": "src/s.rs", "class_name": "Sub", "members": ["a"],
                "clean_siblings": false, "apply": true, "force": true,
                "target_class": "Base", "verify": "compile",
            }),
        ),
        (
            vec![
                "push-down",
                "src/s.rs",
                "--class",
                "Base",
                "--members",
                "a,b",
            ],
            "code_push_down",
            json!({
                "path": "src/s.rs", "class_name": "Base", "members": ["a", "b"],
                "apply": false, "force": false,
            }),
        ),
        (
            vec![
                "push-down",
                "src/s.rs",
                "--class",
                "Base",
                "--members",
                "a",
                "--target-classes",
                "X,Y",
                "--verify",
                "compile",
                "--apply",
            ],
            "code_push_down",
            json!({
                "path": "src/s.rs", "class_name": "Base", "members": ["a"],
                "apply": true, "force": false, "target_classes": ["X", "Y"],
                "verify": "compile",
            }),
        ),
    ];
    for (argv, tool, args) in cases {
        assert_eq!(call(&argv), (Runner::Exec, tool, args), "{argv:?}");
    }
}

#[test]
fn inheritance_and_conditional_replacements_send_optional_parts_only_when_given() {
    let cases: Vec<(Vec<&str>, &str, Value)> = vec![
        (
            vec!["replace-inheritance", "src/d.rs", "--sub-type", "Dog"],
            "code_replace_inheritance_with_delegation",
            json!({"path": "src/d.rs", "sub_type": "Dog", "apply": false, "force": false}),
        ),
        (
            vec![
                "replace-inheritance",
                "src/d.rs",
                "--class",
                "Dog",
                "--base-type",
                "Animal",
                "--field-name",
                "animal",
                "--methods",
                "eat,sleep",
                "--verify",
                "compile",
                "--apply",
                "--force",
            ],
            "code_replace_inheritance_with_delegation",
            json!({
                "path": "src/d.rs", "sub_type": "Dog", "apply": true, "force": true,
                "base_type": "Animal", "field_name": "animal",
                "methods": ["eat", "sleep"], "verify": "compile",
            }),
        ),
        (
            vec![
                "replace-conditional",
                "src/k.rs",
                "--line",
                "5",
                "--character",
                "3",
                "--base",
                "Shape",
                "--method",
                "area",
            ],
            "code_replace_conditional_with_polymorphism",
            json!({
                "path": "src/k.rs", "base_name": "Shape", "method_name": "area", "line": 5,
                "character": 3, "apply": false, "force": false,
            }),
        ),
        (
            vec![
                "replace-conditional",
                "src/k.rs",
                "--line",
                "5",
                "--character",
                "3",
                "--base-name",
                "Shape",
                "--method-name",
                "area",
                "--params",
                "scale: f64,unit: u8",
                "--return-type",
                "f64",
                "--target-var",
                "shape",
                "--verify",
                "compile",
                "--apply",
                "--force",
            ],
            "code_replace_conditional_with_polymorphism",
            json!({
                "path": "src/k.rs", "base_name": "Shape", "method_name": "area", "line": 5,
                "character": 3, "apply": true, "force": true,
                "params": ["scale: f64", "unit: u8"], "return_type": "f64",
                "target_var": "shape", "verify": "compile",
            }),
        ),
    ];
    for (argv, tool, args) in cases {
        assert_eq!(call(&argv), (Runner::Exec, tool, args), "{argv:?}");
    }
}

#[test]
fn conditional_replacement_needs_one_based_coordinates() {
    for (line, character) in [("0", "3"), ("5", "0"), ("0", "0")] {
        let argv = [
            "replace-conditional",
            "src/k.rs",
            "--line",
            line,
            "--character",
            character,
            "--base",
            "Shape",
            "--method",
            "area",
        ];
        assert_eq!(
            refusal(&argv),
            "line and character must be one-based coordinates",
            "line {line}, character {character}"
        );
    }
}

#[test]
fn extract_interface_positions_are_one_based_and_required_for_rust() {
    let base = ["extract-interface", "--symbol", "Repo", "--name", "Store"];
    let with = |file: &'static str, extra: &[&'static str]| {
        let mut argv = vec![base[0], file];
        argv.extend_from_slice(&base[1..]);
        argv.extend_from_slice(extra);
        argv
    };
    let refusals = [
        (
            with("src/r.rs", &[]),
            "Rust extract-interface requires one-based --line and --character positions",
        ),
        (
            with("src/r.rs", &["--line", "2"]),
            "Rust extract-interface requires one-based --line and --character positions",
        ),
        (
            with("src/r.rs", &["--line", "0", "--character", "1"]),
            "Rust extract-interface requires one-based --line and --character positions",
        ),
        (
            with("src/r.rs", &["--line", "1", "--character", "0"]),
            "Rust extract-interface requires one-based --line and --character positions",
        ),
        (
            with("src/r.ts", &["--line", "0"]),
            "line must be a one-based coordinate",
        ),
        (
            with("src/r.ts", &["--character", "0"]),
            "character must be a one-based coordinate",
        ),
    ];
    for (argv, message) in refusals {
        assert_eq!(refusal(&argv), message, "{argv:?}");
    }

    assert_eq!(
        call(&with("src/r.ts", &[])),
        (
            Runner::Exec,
            "code_extract_interface",
            json!({
                "path": "src/r.ts", "symbol": "Repo", "interface_name": "Store",
                "migrate_callers": true, "apply": false, "force": false,
            })
        )
    );
    assert_eq!(
        call(&with(
            "src/r.rs",
            &[
                "--line",
                "4",
                "--character",
                "2",
                "--methods",
                "get,put",
                "--no-migrate-callers",
                "--verify",
                "compile",
                "--apply",
                "--force",
            ]
        )),
        (
            Runner::Exec,
            "code_extract_interface",
            json!({
                "path": "src/r.rs", "symbol": "Repo", "interface_name": "Store",
                "migrate_callers": false, "apply": true, "force": true, "line": 4,
                "character": 2, "methods": ["get", "put"], "verify": "compile",
            })
        )
    );
}

#[test]
fn extract_field_sends_only_the_selection_given() {
    let cases: Vec<(Vec<&str>, Value)> = vec![
        (
            vec![
                "extract-field",
                "src/f.rs",
                "--name",
                "cache",
                "--expression",
                "Vec::new()",
            ],
            json!({
                "path": "src/f.rs", "name": "cache", "replace_all": false, "apply": false,
                "force": false, "expression": "Vec::new()",
            }),
        ),
        (
            vec![
                "extract-field",
                "src/f.rs",
                "6",
                "9",
                "--to",
                " 6 : 20 ",
                "--name",
                "cache",
                "--type",
                "Vec<u8>",
                "--init",
                "Vec::new()",
                "--replace-all",
                "--verify",
                "compile",
                "--apply",
                "--force",
            ],
            json!({
                "path": "src/f.rs", "name": "cache", "replace_all": true, "apply": true,
                "force": true, "line": 6, "character": 9, "end_line": 6, "end_character": 20,
                "type": "Vec<u8>", "init": "Vec::new()", "verify": "compile",
            }),
        ),
        (
            vec![
                "extract-field",
                "src/f.rs",
                "--name",
                "cache",
                "--range",
                "3:1-3:12",
            ],
            json!({
                "path": "src/f.rs", "name": "cache", "replace_all": false, "apply": false,
                "force": false, "range": "3:1-3:12",
            }),
        ),
    ];
    for (argv, args) in cases {
        assert_eq!(
            call(&argv),
            (Runner::Exec, "code_extract_field", args),
            "{argv:?}"
        );
    }
    for to in ["42", "42:x", "x:1"] {
        let argv = [
            "extract-field",
            "src/f.rs",
            "1",
            "1",
            "--to",
            to,
            "--name",
            "n",
        ];
        assert_eq!(
            refusal(&argv),
            "--to takes LINE:COL, for example --to 42:31",
            "--to {to:?}"
        );
    }
}
