/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The tool and the arguments each class-level refactoring command sends: how the first
//! argument is read (a name, a file, a file with `--line`) and which optional parts are sent.

use super::*;
use crate::commands::dispatch::tests::{on_big_stack, parse};
use serde_json::{Value, json};

fn call(args: &[&str]) -> (&'static str, Value) {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    on_big_stack(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        tool_call(parse(&args)).expect("the command builds a tool call")
    })
}

fn check(tool: &str, cases: Vec<(Vec<&str>, Value)>) {
    for (argv, args) in cases {
        assert_eq!(call(&argv), (tool, args), "{argv:?}");
    }
}

#[test]
fn generify_reads_its_first_argument_as_name_or_file() {
    let base = |extra: Value| {
        let mut args = json!({
            "param": "x", "bound": "", "type_param": "T", "apply": false, "force": false,
        });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    };
    check(
        "code_generify",
        vec![
            (
                vec!["generify", "scale", "--param", "x"],
                base(json!({"symbol": "scale"})),
            ),
            (
                vec!["generify", "Shape::scale", "--param", "x"],
                base(json!({"symbol": "Shape::scale"})),
            ),
            (
                vec!["generify", "src/a.rs", "--param", "x"],
                base(json!({"path": "src/a.rs"})),
            ),
            (
                vec![
                    "generify",
                    "src/a.rs",
                    "--param",
                    "x",
                    "--function",
                    "scale",
                ],
                base(json!({"path": "src/a.rs", "symbol": "scale"})),
            ),
            (
                vec![
                    "generify",
                    "src/a.rs",
                    "--param",
                    "x",
                    "--line",
                    "3",
                    "--character",
                    "8",
                ],
                base(json!({"path": "src/a.rs", "line": 3, "character": 8})),
            ),
            (
                vec![
                    "generify",
                    "src/a.rs",
                    "--param",
                    "x",
                    "--line",
                    "3",
                    "--function",
                    "scale",
                ],
                base(json!({"path": "src/a.rs", "line": 3, "character": 1, "symbol": "scale"})),
            ),
            (
                vec!["generify", "scale", "--param", "x", "--path", "src/a.rs"],
                base(json!({"path": "src/a.rs", "symbol": "scale"})),
            ),
            (
                vec![
                    "generify",
                    "scale",
                    "--param",
                    "x",
                    "--path",
                    "src/a.rs",
                    "--function",
                    "other",
                    "--bound",
                    "AsRef<[u32]>",
                    "--as",
                    "U",
                    "--apply",
                    "--force",
                ],
                json!({
                    "param": "x", "bound": "AsRef<[u32]>", "type_param": "U", "apply": true,
                    "force": true, "path": "src/a.rs", "symbol": "other",
                }),
            ),
        ],
    );
}

#[test]
fn invert_boolean_tells_a_file_from_a_name() {
    let args = |extra: Value| {
        let mut args = json!({"new_name": "is_busy", "apply": false, "force": false});
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    };
    check(
        "code_invert_boolean",
        vec![
            (
                vec!["invert-boolean", "is_idle", "--to", "is_busy"],
                args(json!({"symbol": "is_idle"})),
            ),
            (
                vec!["invert-boolean", "src/a.rs", "--to", "is_busy"],
                args(json!({"path": "src/a.rs"})),
            ),
            // A name that is an existing path is a file, extension or not.
            (
                vec!["invert-boolean", ".", "--to", "is_busy"],
                args(json!({"path": "."})),
            ),
            (
                vec![
                    "invert-boolean",
                    "src/a.rs",
                    "--to",
                    "is_busy",
                    "--line",
                    "4",
                ],
                args(json!({"path": "src/a.rs", "line": 4, "character": 1})),
            ),
            (
                vec![
                    "invert-boolean",
                    "is_idle",
                    "--to",
                    "is_busy",
                    "--path",
                    "src/a.rs",
                    "--function",
                    "poll",
                    "--verify",
                    "compile",
                    "--apply",
                    "--force",
                ],
                json!({
                    "new_name": "is_busy", "apply": true, "force": true, "symbol": "is_idle",
                    "path": "src/a.rs", "function": "poll", "verify": "compile",
                }),
            ),
        ],
    );
}

#[test]
fn convert_to_method_and_make_static_share_one_reading_of_their_target() {
    for (command, tool) in [
        ("convert-to-method", "code_convert_to_method"),
        ("make-static", "code_make_static"),
    ] {
        let args = |extra: Value| {
            let mut args = json!({"apply": false, "force": false});
            for (key, value) in extra.as_object().unwrap() {
                args[key] = value.clone();
            }
            args
        };
        check(
            tool,
            vec![
                (
                    vec![command, "Point::dist"],
                    args(json!({"symbol": "Point::dist"})),
                ),
                (vec![command, "src/p.rs"], args(json!({"path": "src/p.rs"}))),
                (
                    vec![command, "src/p.rs", "--line", "2", "--character", "5"],
                    args(json!({"path": "src/p.rs", "line": 2, "character": 5})),
                ),
                // `--method` with no file: the first argument is where it lives.
                (
                    vec![command, "Point", "--method", "dist"],
                    args(json!({"path": "Point", "method": "dist"})),
                ),
                // `--method` with `--path`: the first argument is the class.
                (
                    vec![command, "Point", "--method", "dist", "--path", "src/p.rs"],
                    args(json!({"class_name": "Point", "path": "src/p.rs", "method": "dist"})),
                ),
                (
                    vec![
                        command,
                        "Point::dist",
                        "--class",
                        "Point",
                        "--verify",
                        "compile",
                        "--apply",
                        "--force",
                    ],
                    json!({
                        "apply": true, "force": true, "symbol": "Point::dist",
                        "class_name": "Point", "verify": "compile",
                    }),
                ),
            ],
        );
    }
}

#[test]
fn inline_parameter_sends_only_what_it_was_given() {
    check(
        "code_inline_parameter",
        vec![
            (
                vec!["inline-parameter", "src/a.rs"],
                json!({"path": "src/a.rs", "apply": false, "force": false}),
            ),
            (
                vec![
                    "inline-parameter",
                    "src/a.rs",
                    "3",
                    "7",
                    "--function",
                    "scale",
                    "--param",
                    "factor",
                    "--verify",
                    "compile",
                    "--apply",
                    "--force",
                ],
                json!({
                    "path": "src/a.rs", "apply": true, "force": true, "line": 3, "character": 7,
                    "function": "scale", "parameter": "factor", "verify": "compile",
                }),
            ),
        ],
    );
}

#[test]
fn extract_delegate_sends_its_field_lists_and_names() {
    check(
        "code_extract_delegate",
        vec![
            (
                vec![
                    "extract-delegate",
                    "src/a.rs",
                    "--fields",
                    "a,b",
                    "--name",
                    "Helper",
                    "--field",
                    "helper",
                ],
                json!({
                    "path": "src/a.rs", "fields": ["a", "b"], "methods": [""],
                    "name": "Helper", "field": "helper", "apply": false, "force": false,
                }),
            ),
            (
                vec![
                    "extract-delegate",
                    "src/a.rs",
                    "2",
                    "1",
                    "--symbol",
                    "Config",
                    "--fields",
                    "a",
                    "--methods",
                    "get,set",
                    "--name",
                    "Helper",
                    "--field",
                    "helper",
                    "--verify",
                    "compile",
                    "--apply",
                    "--force",
                ],
                json!({
                    "path": "src/a.rs", "fields": ["a"], "methods": ["get", "set"],
                    "name": "Helper", "field": "helper", "apply": true, "force": true,
                    "line": 2, "character": 1, "symbol": "Config", "verify": "compile",
                }),
            ),
        ],
    );
}

#[test]
fn extract_trait_migrates_callers_unless_told_not_to() {
    check(
        "code_extract_trait",
        vec![
            (
                vec![
                    "extract-trait",
                    "src/a.rs",
                    "4",
                    "1",
                    "--methods",
                    "a,b",
                    "--name",
                    "Shape",
                ],
                json!({
                    "path": "src/a.rs", "line": 4, "character": 1, "methods": ["a", "b"],
                    "name": "Shape", "migrate_callers": true, "apply": false, "force": false,
                }),
            ),
            (
                vec![
                    "extract-trait",
                    "src/a.rs",
                    "4",
                    "1",
                    "--methods",
                    "a",
                    "--name",
                    "Shape",
                    "--no-migrate-callers",
                    "--apply",
                    "--force",
                ],
                json!({
                    "path": "src/a.rs", "line": 4, "character": 1, "methods": ["a"],
                    "name": "Shape", "migrate_callers": false, "apply": true, "force": true,
                }),
            ),
        ],
    );
}

#[test]
fn loop_to_iterator_sends_a_position_or_a_symbol() {
    check(
        "code_loop_to_iterator",
        vec![
            (
                vec!["loop-to-iterator", "src/a.rs"],
                json!({"path": "src/a.rs", "apply": false, "force": false}),
            ),
            (
                vec![
                    "loop-to-iterator",
                    "src/a.rs",
                    "9",
                    "5",
                    "--symbol",
                    "total",
                    "--apply",
                    "--force",
                ],
                json!({
                    "path": "src/a.rs", "apply": true, "force": true, "line": 9, "character": 5,
                    "symbol": "total",
                }),
            ),
        ],
    );
}

#[test]
fn wrap_return_reads_its_first_argument_like_generify_and_adds_the_envelope() {
    let args = |extra: Value| {
        let mut args = json!({"wrapper": "option", "apply": false, "force": false});
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    };
    check(
        "code_wrap_return",
        vec![
            (
                vec!["wrap-return", "find", "--wrapper", "option"],
                args(json!({"symbol": "find"})),
            ),
            (
                vec!["wrap-return", "src/a.rs", "--wrapper", "option"],
                args(json!({"path": "src/a.rs"})),
            ),
            (
                vec![
                    "wrap-return",
                    "src/a.rs",
                    "--wrapper",
                    "option",
                    "--function",
                    "find",
                ],
                args(json!({"path": "src/a.rs", "symbol": "find"})),
            ),
            (
                vec![
                    "wrap-return",
                    "src/a.rs",
                    "--wrapper",
                    "option",
                    "--line",
                    "6",
                    "--character",
                    "4",
                    "--function",
                    "find",
                ],
                args(json!({"path": "src/a.rs", "line": 6, "character": 4, "symbol": "find"})),
            ),
            (
                vec![
                    "wrap-return",
                    "find",
                    "--wrapper",
                    "option",
                    "--path",
                    "src/a.rs",
                ],
                args(json!({"path": "src/a.rs", "symbol": "find"})),
            ),
            (
                vec![
                    "wrap-return",
                    "find",
                    "--wrapper",
                    "result",
                    "--path",
                    "src/a.rs",
                    "--function",
                    "lookup",
                    "--constructor",
                    "Ok",
                    "--error",
                    "anyhow::Error",
                    "--verify",
                    "compile",
                    "--apply",
                    "--force",
                ],
                json!({
                    "wrapper": "result", "apply": true, "force": true, "constructor": "Ok",
                    "path": "src/a.rs", "symbol": "lookup", "error": "anyhow::Error",
                    "verify": "compile",
                }),
            ),
        ],
    );
}
