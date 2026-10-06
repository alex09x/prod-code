/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::conversion::{into_call, language_conversion, parse_mismatch, suggest, type_name};
use super::sites::is_candidate;
use super::spans::{declared_type_span, declared_type_span_polyglot, expression_span};
use super::types::{Conversion, Migration, Site};
use crate::parameter_object::Language;
use std::path::PathBuf;

fn span_of(text: &str, name: &str) -> String {
    let at = text.find(name).expect("the name is in the text");
    let (s, e) = declared_type_span(text, at).expect("a declared type");
    text[s..e].to_string()
}

#[test]
fn a_declared_type_is_found_in_every_shape_that_can_have_one() {
    assert_eq!(
        span_of("    pub timeout_secs: u64,\n", "timeout_secs"),
        "u64"
    );
    assert_eq!(
        span_of("    pub map: BTreeMap<String, Vec<u8>>,\n", "map"),
        "BTreeMap<String, Vec<u8>>"
    );
    assert_eq!(span_of("fn f(a: &str, b: u32) {}", "b"), "u32");
    assert_eq!(
        span_of("fn compute(a: &str) -> Result<u32, Error> {\n", "compute"),
        "Result<u32, Error>"
    );
    assert_eq!(span_of("    let x: Vec<u8> = go();\n", "x"), "Vec<u8>");
    // The last field of a struct, written without a trailing comma.
    assert_eq!(span_of("    pub n: usize\n}\n", "n"), "usize");
}

#[test]
fn a_function_without_a_return_type_has_nothing_to_migrate() {
    let text = "fn compute(a: u8) {\n    a;\n}\n";
    let at = text.find("compute").expect("the name");
    assert_eq!(declared_type_span(text, at), None);
}

#[test]
fn a_suggestion_is_made_only_when_the_error_names_both_types() {
    assert_eq!(
        suggest("expected u64, found u32", "u32", "u64").as_deref(),
        Some("the value here is still `u32`; convert it to `u64`")
    );
    assert!(
        suggest("expected u32, found u64", "u32", "u64").is_some_and(|s| s.contains("still wants"))
    );
    // Neither type is ours: no guess.
    assert_eq!(suggest("expected String, found &str", "u32", "u64"), None);
    // The declaration spells a path; the analyzer spells the name that is in scope.
    assert_eq!(
        suggest("expected Duration, found u64", "u64", "std::time::Duration").as_deref(),
        Some("the value here is still `u64`; convert it to `Duration`")
    );
    assert_eq!(
        suggest("cannot find value `n` in this scope", "u32", "u64"),
        None
    );
}

#[test]
fn the_report_calls_the_sites_work_rather_than_failure() {
    let migration = Migration {
        symbol: "timeout_secs".into(),
        root: PathBuf::from("/root"),
        file: "src/lib.rs".into(),
        was: "u64".into(),
        now: "std::time::Duration".into(),
        rewritten: Vec::new(),
        sites: vec![Site {
            file: "src/other.rs".into(),
            line: 12,
            col: 5,
            message: "expected Duration, found u64".into(),
            code: Some("E0308".into()),
            source: "cfg.timeout_secs = 30;".into(),
            suggestion: Some("the value here is still `u64`".into()),
            end: None,
        }],
        in_attributes: 0,
        converted: Vec::new(),
        conversion_note: None,
        transitive_count: 0,
        transitively_migrated: Vec::new(),
        applied: false,
    };
    let text = migration.render(50);
    assert!(text.contains("1 site(s) in 1 file(s)"), "{text}");
    assert!(text.contains("src/other.rs"), "{text}");
    assert!(text.contains("cfg.timeout_secs = 30;"), "{text}");
    assert!(
        text.contains("try: the value here is still `u64`"),
        "{text}"
    );
    assert!(text.contains("they are the migration"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");

    let clean = Migration {
        sites: Vec::new(),
        applied: true,
        ..migration
    };
    let text = clean.render(50);
    assert!(text.contains("nothing else has to change"), "{text}");
    assert!(text.contains("the declaration was written"), "{text}");
}

#[test]
fn a_long_list_is_cut_at_the_budget_and_says_how_many_are_left() {
    let site = |line: u32| Site {
        file: "src/lib.rs".into(),
        line,
        col: 1,
        message: "expected A, found B".into(),
        code: None,
        source: "x".into(),
        suggestion: None,
        end: None,
    };
    let migration = Migration {
        symbol: "f".into(),
        root: PathBuf::from("/root"),
        file: "src/lib.rs".into(),
        was: "A".into(),
        now: "B".into(),
        rewritten: Vec::new(),
        sites: (1..=10).map(site).collect(),
        in_attributes: 0,
        converted: Vec::new(),
        conversion_note: None,
        transitive_count: 0,
        transitively_migrated: Vec::new(),
        applied: false,
    };
    let text = migration.render(3);
    assert!(text.contains("… 7 more site(s)"), "{text}");
}

#[test]
fn into_goes_on_a_path_or_call_and_around_anything_else() {
    assert_eq!(into_call("secs"), "secs.into()");
    assert_eq!(into_call("l.timeout"), "l.timeout.into()");
    assert_eq!(
        into_call("build(1, \"x\").timeout"),
        "build(1, \"x\").timeout.into()"
    );
    assert_eq!(into_call("secs as u32"), "(secs as u32).into()");
    assert_eq!(into_call("l.timeout * 2"), "(l.timeout * 2).into()");
    assert_eq!(into_call("&name"), "(&name).into()");
    assert_eq!(into_call("-1"), "(-1).into()");
}

#[test]
fn a_candidate_is_the_two_types_meeting_on_one_line() {
    let site = |message: &str, code: &str, end: Option<(u32, u32)>| Site {
        file: "src/lib.rs".into(),
        line: 4,
        col: 5,
        message: message.into(),
        code: Some(code.into()),
        source: String::new(),
        suggestion: None,
        end,
    };
    let one_line = Some((4, 9));
    assert!(is_candidate(
        &site("expected u64, found u32", "E0308", one_line),
        "u32",
        "u64"
    ));
    assert!(is_candidate(
        &site("expected u32, found u64", "E0308", one_line),
        "u32",
        "u64"
    ));
    assert!(!is_candidate(
        &site("expected u64, found u16", "E0308", one_line),
        "u32",
        "u64"
    ));
    assert!(!is_candidate(
        &site("expected u64, found u32", "E0277", one_line),
        "u32",
        "u64"
    ));
    assert!(!is_candidate(
        &site("expected u64, found u32", "E0308", Some((6, 2))),
        "u32",
        "u64"
    ));
    assert!(!is_candidate(
        &site("expected u64, found u32", "E0308", None),
        "u32",
        "u64"
    ));
}

#[test]
fn a_converted_migration_lists_what_it_wrote() {
    let migration = Migration {
        symbol: "timeout".into(),
        root: PathBuf::from("/root"),
        file: "src/lib.rs".into(),
        was: "u32".into(),
        now: "u64".into(),
        rewritten: Vec::new(),
        sites: Vec::new(),
        in_attributes: 0,
        converted: vec![Conversion {
            file: "src/lib.rs".into(),
            line: 9,
            was: "secs".into(),
            now: "secs.into()".into(),
        }],
        conversion_note: Some("a note".into()),
        transitive_count: 0,
        transitively_migrated: Vec::new(),
        applied: true,
    };
    let text = migration.render(10);
    assert!(
        text.contains("1 site(s) converted with `.into()`"),
        "{text}"
    );
    assert!(
        text.contains("src/lib.rs:9  `secs` → `secs.into()`"),
        "{text}"
    );
    assert!(text.contains("a note"), "{text}");
    assert!(
        text.contains("the declaration and 1 conversion(s) were written"),
        "{text}"
    );
}

#[test]
fn a_type_is_named_the_way_the_analyzer_names_it() {
    assert_eq!(type_name("std::time::Duration"), "Duration");
    assert_eq!(type_name("Vec<std::string::String>"), "Vec<String>");
    assert_eq!(type_name("Box<str, Global>"), "Box<str>");
    assert_eq!(type_name("std::boxed::Box<str>"), "Box<str>");
    assert_eq!(
        type_name("HashMap<u32, Vec<u8, Global>>"),
        "HashMap<u32, Vec<u8>>"
    );
}

#[test]
fn a_method_name_range_takes_its_arguments_and_a_chain_is_not_cut() {
    let site = |line: u32, col: u32, end_col: u32| Site {
        file: "src/lib.rs".into(),
        line,
        col,
        message: String::new(),
        code: None,
        source: String::new(),
        suggestion: None,
        end: Some((line, end_col)),
    };
    let text = "    name: label.to_string(),\n    x: a.b + c,\n";
    let (s, e) = expression_span(text, &site(1, 17, 26)).expect("a span");
    assert_eq!(&text[s..e], "to_string()");
    assert!(expression_span(text, &site(2, 10, 15)).is_none());
    let (s, e) = expression_span(text, &site(2, 8, 11)).expect("a span");
    assert_eq!(&text[s..e], "a.b");
}

#[test]
fn polyglot_declared_type_span_found_in_all_languages() {
    // TypeScript
    let ts_text = "function compute(val: number): number {\n  const doubled: number = val * 2;\n  return doubled;\n}";
    let val_at = ts_text.find("val").unwrap();
    let (s, e) =
        declared_type_span_polyglot(ts_text, val_at, Language::TypeScript).expect("ts param");
    assert_eq!(&ts_text[s..e], "number");

    let fn_at = ts_text.find("compute").unwrap();
    let (s, e) = declared_type_span_polyglot(ts_text, fn_at, Language::TypeScript).expect("ts ret");
    assert_eq!(&ts_text[s..e], "number");

    let d_at = ts_text.find("doubled").unwrap();
    let (s, e) = declared_type_span_polyglot(ts_text, d_at, Language::TypeScript).expect("ts var");
    assert_eq!(&ts_text[s..e], "number");

    // Python
    let py_text = "def compute(val: int) -> int:\n    doubled: int = val * 2\n    return doubled\n";
    let val_at = py_text.find("val").unwrap();
    let (s, e) = declared_type_span_polyglot(py_text, val_at, Language::Python).expect("py param");
    assert_eq!(&py_text[s..e], "int");

    let fn_at = py_text.find("compute").unwrap();
    let (s, e) = declared_type_span_polyglot(py_text, fn_at, Language::Python).expect("py ret");
    assert_eq!(&py_text[s..e], "int");

    let d_at = py_text.find("doubled").unwrap();
    let (s, e) = declared_type_span_polyglot(py_text, d_at, Language::Python).expect("py var");
    assert_eq!(&py_text[s..e], "int");

    // Go
    let go_text =
        "func compute(val int32) int32 {\n    var doubled int32 = val * 2\n    return doubled\n}\n";
    let val_at = go_text.find("val").unwrap();
    let (s, e) = declared_type_span_polyglot(go_text, val_at, Language::Go).expect("go param");
    assert_eq!(&go_text[s..e], "int32");

    let fn_at = go_text.find("compute").unwrap();
    let (s, e) = declared_type_span_polyglot(go_text, fn_at, Language::Go).expect("go ret");
    assert_eq!(&go_text[s..e], "int32");

    let d_at = go_text.find("doubled").unwrap();
    let (s, e) = declared_type_span_polyglot(go_text, d_at, Language::Go).expect("go var");
    assert_eq!(&go_text[s..e], "int32");

    // Swift
    let sw_text =
        "func compute(val: Int) -> Int {\n    let doubled: Int = val * 2\n    return doubled\n}\n";
    let val_at = sw_text.find("val").unwrap();
    let (s, e) =
        declared_type_span_polyglot(sw_text, val_at, Language::Swift).expect("swift param");
    assert_eq!(&sw_text[s..e], "Int");

    let fn_at = sw_text.find("compute").unwrap();
    let (s, e) = declared_type_span_polyglot(sw_text, fn_at, Language::Swift).expect("swift ret");
    assert_eq!(&sw_text[s..e], "Int");

    let d_at = sw_text.find("doubled").unwrap();
    let (s, e) = declared_type_span_polyglot(sw_text, d_at, Language::Swift).expect("swift var");
    assert_eq!(&sw_text[s..e], "Int");

    // C++
    let cpp_text = "int compute(int val) {\n    int doubled = val * 2;\n    return doubled;\n}\n";
    let val_at = cpp_text.find("val").unwrap();
    let (s, e) = declared_type_span_polyglot(cpp_text, val_at, Language::Cpp).expect("cpp param");
    assert_eq!(&cpp_text[s..e], "int");

    let fn_at = cpp_text.find("compute").unwrap();
    let (s, e) = declared_type_span_polyglot(cpp_text, fn_at, Language::Cpp).expect("cpp ret");
    assert_eq!(&cpp_text[s..e], "int");

    let d_at = cpp_text.find("doubled").unwrap();
    let (s, e) = declared_type_span_polyglot(cpp_text, d_at, Language::Cpp).expect("cpp var");
    assert_eq!(&cpp_text[s..e], "int");
}

#[test]
fn language_conversion_syntax_across_polyglot() {
    assert_eq!(language_conversion("x", "u64", Language::Rust), "x.into()");
    assert_eq!(
        language_conversion("x", "number", Language::TypeScript),
        "Number(x)"
    );
    assert_eq!(
        language_conversion("x", "MyType", Language::TypeScript),
        "MyType(x)"
    );
    assert_eq!(
        language_conversion("x + 1", "MyType", Language::TypeScript),
        "(x + 1 as MyType)"
    );
    assert_eq!(language_conversion("x", "int", Language::Python), "int(x)");
    assert_eq!(language_conversion("x", "int64", Language::Go), "int64(x)");
    assert_eq!(
        language_conversion("x", "Int64", Language::Swift),
        "Int64(x)"
    );
    assert_eq!(
        language_conversion("x", "int64_t", Language::Cpp),
        "static_cast<int64_t>(x)"
    );
}

#[test]
fn parse_mismatch_polyglot_messages() {
    assert_eq!(
        parse_mismatch("Type 'string' is not assignable to type 'number'"),
        Some(("number".to_string(), "string".to_string()))
    );
    assert_eq!(
        parse_mismatch("Expression of type \"str\" cannot be assigned to declared type \"int\""),
        Some(("int".to_string(), "str".to_string()))
    );
    assert_eq!(
        parse_mismatch("cannot use x (variable of type int32) as int64 value"),
        Some(("int64".to_string(), "int32".to_string()))
    );
    assert_eq!(
        parse_mismatch("cannot convert value of type 'Int' to specified type 'Int64'"),
        Some(("Int64".to_string(), "Int".to_string()))
    );
    assert_eq!(
        parse_mismatch("no viable conversion from 'int' to 'double'"),
        Some(("double".to_string(), "int".to_string()))
    );
}
