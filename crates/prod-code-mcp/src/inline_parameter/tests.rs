/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;

use super::decl::format_binding;
use super::syntax::{is_caller_independent, keyword_arg, swift_label};
use super::types::InlinedParameter;

#[test]
fn only_a_value_that_means_the_same_in_the_body_is_inlined() {
    for ok in [
        "10",
        "-3",
        "2.5",
        "1_000u64",
        "true",
        "false",
        "True",
        "False",
        "None",
        "nil",
        "null",
        "nullptr",
        "undefined",
        "\"x\"",
        "b\"raw\"",
        "'c'",
        "LIMIT",
        "Mode::Fast",
        "crate::limits::MAX",
        "Config.MAX",
        "Math.PI",
        "Default",
    ] {
        assert!(is_caller_independent(ok), "{ok}");
    }
    for no in [
        "limit",
        "v + 1",
        "f()",
        "self.max",
        "this.max",
        "&x",
        "LIMIT + 1",
        "format!(\"{x}\")",
        "\"{x}\"",
        "Mode::from(x)",
        "self.MAX",
        "this.LIMIT",
    ] {
        assert!(!is_caller_independent(no), "{no}");
    }
}

#[test]
fn the_report_names_the_value_and_the_calls() {
    let done = InlinedParameter {
        function: "clamp".into(),
        parameter: "max".into(),
        value: "LIMIT".into(),
        root: "/nonexistent".into(),
        file: "src/lib.rs".into(),
        rewritten_calls: 2,
        unmatched: vec![
            "src/lib.rs:9:5 (the function used as a value: it would change type)".into(),
        ],
        rewritten: vec![],
        diagnostics: vec![],
        applied: false,
    };
    let text = done.render(1000);
    assert!(text.contains("every call passes `LIMIT`"), "{text}");
    assert!(text.contains("2 call(s) lose the argument"), "{text}");
    assert!(
        text.contains("nothing is written while any remains"),
        "{text}"
    );
    assert!(text.contains("nothing was written"), "{text}");
}

#[test]
fn polyglot_format_binding_generates_correct_syntax() {
    assert_eq!(
        format_binding("max", Some("number"), "100", Language::TypeScript),
        "const max: number = 100;"
    );
    assert_eq!(
        format_binding("max", None, "100", Language::JavaScript),
        "const max = 100;"
    );
    assert_eq!(
        format_binding("max", Some("int"), "100", Language::Python),
        "max = 100"
    );
    assert_eq!(
        format_binding("max", Some("int"), "100", Language::Cpp),
        "const int max = 100;"
    );
    assert_eq!(
        format_binding("max", Some("Int"), "100", Language::Swift),
        "let max: Int = 100"
    );
    assert_eq!(
        format_binding("max", Some("int"), "100", Language::Go),
        "const max = 100"
    );
    assert_eq!(
        format_binding("ptr", None, "&MyStruct{}", Language::Go),
        "var ptr = &MyStruct{}"
    );
}

#[test]
fn polyglot_keyword_arg_and_swift_label_parse_correctly() {
    assert_eq!(keyword_arg("max=100"), Some(("max", "100")));
    assert_eq!(keyword_arg("max = 100"), Some(("max", "100")));
    assert_eq!(keyword_arg("a == b"), None);
    assert_eq!(keyword_arg("100"), None);

    assert_eq!(swift_label("max: 100"), Some(("max", "100")));
    assert_eq!(swift_label("label: val"), Some(("label", "val")));
    assert_eq!(swift_label("100"), None);
}
