/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use crate::parameter_object::Language;

#[test]
fn a_wrapper_is_named_either_way_and_nothing_else() {
    assert_eq!(Wrapper::parse("option").unwrap(), Wrapper::Option);
    assert_eq!(Wrapper::parse("Result").unwrap(), Wrapper::Result);
    assert_eq!(Wrapper::parse("promise").unwrap(), Wrapper::Promise);
    assert_eq!(Wrapper::parse("pointer").unwrap(), Wrapper::Pointer);
    assert_eq!(
        Wrapper::parse("Response").unwrap(),
        Wrapper::Custom("Response".into())
    );
    assert!(Wrapper::parse("").is_err());
    assert_eq!(Wrapper::Option.assist_id(), "wrap_return_type_in_option");
    assert_eq!(Wrapper::Result.assist_id(), "wrap_return_type_in_result");
}

#[test]
fn polyglot_declaration_selection_uses_the_requested_overload_line() {
    let source = "function pick(value: string): string { return value; }\n\nfunction pick(value: number): number { return value; }\n";
    let decl = find_polyglot_decl(source, Language::TypeScript, Some("pick"), Some(3)).unwrap();

    assert_eq!(decl.was, "number");
    let open_paren = decl.name_start + source[decl.name_start..decl.close_paren].find('(').unwrap();
    assert_eq!(&source[open_paren + 1..decl.close_paren], "value: number");
}

#[test]
fn polyglot_declaration_body_ends_before_next_function_after_nested_closure() {
    let source = "export function calculate() {\n    const inner = () => { return 2; };\n    return [1];\n}\n\nexport async function run() {\n    return calculate()[0].toString();\n}\n";
    let decl = find_polyglot_decl(source, Language::TypeScript, Some("calculate"), None).unwrap();
    let next_function = source.find("export async function run").unwrap();
    let call_at = source.rfind("calculate").unwrap();

    assert!(decl.body_close < next_function);
    assert!(call_at > decl.body_close);
    assert!(!crate::inline_parameter::is_in_comment(
        source,
        call_at,
        Language::TypeScript
    ));
    assert!(!crate::inline_parameter::is_in_string(
        source,
        call_at,
        Language::TypeScript
    ));
    assert!(!crate::inline_parameter::is_import_or_export_context(
        source,
        call_at,
        Language::TypeScript
    ));
    assert!(crate::parameter_object::call_args_span(source, call_at + "calculate".len()).is_some());
}

#[test]
fn the_declared_return_type_is_found_between_the_arrow_and_the_body() {
    let text = "pub fn plain(a: u32) -> Vec<u32> where u32: Copy {\n    vec![a]\n}\n";
    let close = text.find(')').unwrap();
    let (s, e) = declared_return(text, close).unwrap();
    assert_eq!(&text[s..e], "Vec<u32>");
    let unit = "fn f() {}\n";
    assert!(declared_return(unit, unit.find(')').unwrap()).is_none());
}

#[test]
fn cpp_return_wrapping_keeps_method_qualifiers_outside_the_wrapped_type() {
    let out_of_class = "inline int Widget::load() { return 1; }\n";
    let decl = find_polyglot_decl(out_of_class, Language::Cpp, Some("load"), None).unwrap();
    assert_eq!(decl.was, "int");
    let (wrapped, _) = restructure_declaring_file(
        out_of_class,
        Language::Cpp,
        &decl,
        &Wrapper::Option,
        None,
        None,
    )
    .unwrap();
    assert!(
        wrapped.contains("inline std::optional<int> Widget::load()"),
        "{wrapped}"
    );

    let member = "struct Widget { static inline int load() { return 1; } };\n";
    let decl = find_polyglot_decl(member, Language::Cpp, Some("load"), None).unwrap();
    assert_eq!(decl.was, "int");
    let (wrapped, _) =
        restructure_declaring_file(member, Language::Cpp, &decl, &Wrapper::Option, None, None)
            .unwrap();
    assert!(
        wrapped.contains("static inline std::optional<int> load()"),
        "{wrapped}"
    );
}

#[test]
fn the_caller_is_the_innermost_function_around_the_call() {
    let text = "fn outer() -> Option<u32> {\n    fn inner() -> u32 {\n        plain()\n    }\n    Some(plain()?)\n}\nfn unit() {\n    plain();\n}\n";
    let first = text.find("plain()").unwrap();
    assert_eq!(enclosing_return_type(text, first).as_deref(), Some("u32"));
    let second = text[first + 1..].find("plain()").unwrap() + first + 1;
    assert_eq!(
        enclosing_return_type(text, second).as_deref(),
        Some("Option<u32>")
    );
    let third = text.rfind("plain()").unwrap();
    assert_eq!(enclosing_return_type(text, third).as_deref(), Some("()"));
    assert_eq!(enclosing_return_type("plain()", 0), None);
}

#[test]
fn only_a_matching_wrapper_can_propagate() {
    assert!(propagates("Option<u32>", &Wrapper::Option));
    assert!(propagates("std::option::Option<u32>", &Wrapper::Option));
    assert!(propagates("anyhow::Result<()>", &Wrapper::Result));
    assert!(propagates("Result<u32, String>", &Wrapper::Result));
    assert!(propagates(
        "Response<u32>",
        &Wrapper::Custom("Response".into())
    ));
    assert!(propagates(
        "my_mod::Response<u32>",
        &Wrapper::Custom("Response".into())
    ));
    assert!(!propagates("Result<u32, String>", &Wrapper::Option));
    assert!(!propagates("u32", &Wrapper::Result));
    assert!(!propagates("u32", &Wrapper::Custom("Response".into())));
    assert!(!propagates("()", &Wrapper::Option));
}

fn report() -> WrappedReturn {
    WrappedReturn {
        function: "plain".into(),
        root: "/root".into(),
        file: "src/lib.rs".into(),
        was: "u32".into(),
        now: "Result<u32, String>".into(),
        propagated: 2,
        blocked: vec!["src/app.rs:4:5 the caller returns `u32`: `plain()`".into()],
        unmatched: vec![
            "src/lib.rs:9:5 (a call inside `plain` itself: add `?` there by hand)".into(),
        ],
        rewritten: vec![("/root/src/lib.rs".into(), "fn main() {}\n".into())],
        diagnostics: vec!["mismatched types (src/app.rs:4:5)".into()],
        applied: false,
    }
}

#[test]
fn the_report_names_what_propagates_and_what_needs_a_decision() {
    let text = report().render(10_000);
    assert!(
        text.contains("returned: `u32`") && text.contains("now returns: `Result<u32, String>`"),
        "{text}"
    );
    assert!(text.contains("2 call site(s) propagate"), "{text}");
    assert!(text.contains("does not return a `Result`"), "{text}");
    assert!(text.contains("a call inside `plain` itself"), "{text}");
    assert!(text.contains("the analyzer rejects the result"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
    let mut done = report();
    done.blocked.clear();
    done.unmatched.clear();
    done.diagnostics.clear();
    done.applied = true;
    let text = done.render(10);
    assert!(
        text.contains("0 errors")
            && text.contains("[applied to 1 file(s)]")
            && text.contains("diff truncated"),
        "{text}"
    );
}
