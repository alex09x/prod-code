/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::binding::bind_arguments;
use super::super::container::outermost_container;
use super::super::cpp_std::{std_in_cmake, std_in_flags, std_year};
use super::super::params::parse_params;
use super::super::rewrite::{rewritten_call, rewritten_call_with};
use super::super::type_render::{aggregate_text, type_text};
use super::super::types::{Field, Kind, Language};

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn field(name: &str, ty: Option<&str>, default: Option<&str>) -> Field {
    Field {
        name: name.to_string(),
        ty: ty.map(str::to_string),
        default: default.map(str::to_string),
        optional: false,
    }
}

#[test]
fn a_c_parameter_is_named_by_its_declarator() {
    let list = "const char *name, int xs[], char grid[][4], int (*cb)(int), const std::map<int, int> &m, int height = 2, unsigned, ...";
    let (receiver, params) = parse_params(list, Language::Cpp);
    assert_eq!(receiver, None);
    let named: Vec<(&str, &str)> = params
        .iter()
        .map(|p| (p.name.as_str(), p.ty.as_deref().unwrap_or("")))
        .collect();
    assert_eq!(
        named,
        [
            ("name", "const char *name"),
            ("xs", "int *xs"),
            ("grid", "char (*grid)[4]"),
            ("cb", "int (*cb)(int)"),
            ("m", "const std::map<int, int> &m"),
            ("height", "int height"),
            ("", "unsigned"),
            ("", "")
        ]
    );
    assert_eq!(&list[params[3].name_at..params[3].name_at + 2], "cb");
    assert_eq!(params[5].default.as_deref(), Some("2"));
    assert_eq!(params[7].kind, Kind::Variadic);
    assert!(parse_params(" void ", Language::C).1.is_empty());
}

#[test]
fn a_swift_parameter_has_a_label_a_type_and_a_default() {
    let list = "_ label: String, with name: String, x: Int, y: Int = 0, cb: @escaping (Int) -> Void, d: [String: Int], xs: Int...";
    let (_, params) = parse_params(list, Language::Swift);
    let seen: Vec<(Option<&str>, &str, &str)> = params
        .iter()
        .map(|p| {
            (
                p.label.as_deref(),
                p.name.as_str(),
                p.ty.as_deref().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            (None, "label", "String"),
            (Some("with"), "name", "String"),
            (Some("x"), "x", "Int"),
            (Some("y"), "y", "Int"),
            (Some("cb"), "cb", "@escaping (Int) -> Void"),
            (Some("d"), "d", "[String: Int]"),
            (Some("xs"), "xs", "Int...")
        ]
    );
    assert_eq!(&list[params[1].name_at..params[1].name_at + 4], "name");
    assert_eq!(params[3].default.as_deref(), Some("0"));
    assert_eq!(params[6].kind, Kind::Variadic);
}

#[test]
fn a_swift_call_binds_by_label_and_passes_the_literal_under_the_new_one() {
    let (_, params) = parse_params("_ label: String, x: Int, y: Int = 0", Language::Swift);
    let rewrite = |args: &[&str], bundled: &[usize]| {
        let args = strings(args);
        bind_arguments(&args, &params, Language::Swift).map(|bound| {
            rewritten_call(
                &args,
                &bound,
                bundled,
                &params,
                Language::Swift,
                "Point",
                "point",
            )
        })
    };
    assert_eq!(
        rewrite(&["\"b\"", "x: 5", "y: 6"], &[1, 2]).as_deref(),
        Some("\"b\", point: Point(x: 5, y: 6)")
    );
    assert_eq!(
        rewrite(&["\"c\"", "x: 9"], &[1, 2]).as_deref(),
        Some("\"c\", point: Point(x: 9)"),
        "a default the call relied on is the field's default"
    );
    assert_eq!(
        rewrite(&["\"b\"", "x: 5", "y: 6"], &[0, 1]).as_deref(),
        Some("Point(label: \"b\", x: 5), y: 6"),
        "an unlabelled first parameter makes an unlabelled new one"
    );
    assert_eq!(rewrite(&["x: 5"], &[1, 2]), None, "`label` has no default");
    assert_eq!(
        rewrite(&["\"b\"", "y: 6", "x: 5"], &[1, 2]),
        None,
        "labels come in declaration order"
    );
}

#[test]
fn a_cpp_call_may_leave_off_defaulted_trailing_arguments() {
    let (_, mut params) = parse_params("const char *name, int width, int height", Language::Cpp);
    params[2].default = Some("2".to_string());
    let args = strings(&["\"m\"", "7"]);
    let bound = bind_arguments(&args, &params, Language::Cpp).expect("height has a default");
    assert_eq!(
        rewritten_call(
            &args,
            &bound,
            &[1, 2],
            &params,
            Language::Cpp,
            "Size",
            "size"
        ),
        "\"m\", {.width = 7}"
    );
    assert_eq!(
        rewritten_call_with(
            &args,
            &bound,
            &[1, 2],
            &params,
            Language::Cpp,
            "size",
            aggregate_text
        ),
        "\"m\", {7}"
    );
    assert!(bind_arguments(&strings(&["\"m\""]), &params, Language::Cpp).is_none());
    let (_, params) = parse_params("const char *name, int width, int height", Language::C);
    let args = strings(&["\"a\"", "3", "4"]);
    let bound = bind_arguments(&args, &params, Language::C).expect("same arity");
    assert_eq!(
        rewritten_call(&args, &bound, &[1, 2], &params, Language::C, "Size", "size"),
        "\"a\", (struct Size){.width = 3, .height = 4}"
    );
    let (_, printf) = parse_params("const char *fmt, ...", Language::C);
    assert_eq!(
        bind_arguments(&strings(&["f", "1", "2"]), &printf, Language::C),
        Some(vec![Some(0), Some(1), Some(1)])
    );
}

#[test]
fn c_cpp_and_swift_declare_a_struct_their_own_way() {
    let fields = [
        field("width", Some("int width"), None),
        field("height", Some("int height"), Some("2")),
    ];
    assert_eq!(
        type_text(Language::Cpp, "Size", "build", &fields, "    ", false),
        "// The parameters `build` takes together.\nstruct Size {\n    int width;\n    int height = 2;\n};\n"
    );
    assert_eq!(
        type_text(
            Language::C,
            "Size",
            "build",
            &[field("name", Some("const char *name"), None)],
            "\t",
            false
        ),
        "/* The parameters `build` takes together. */\nstruct Size {\n\tconst char *name;\n};\n"
    );
    assert_eq!(
        type_text(
            Language::Swift,
            "Point",
            "draw",
            &[
                field("x", Some("Int"), None),
                field("y", Some("Int"), Some("0"))
            ],
            "    ",
            true
        ),
        "/// The parameters `draw` takes together.\npublic struct Point {\n    let x: Int\n    var y: Int = 0\n}\n"
    );
}

#[test]
fn the_cpp_standard_is_what_the_build_declares() {
    assert_eq!(
        std_in_cmake("set(CMAKE_CXX_STANDARD_REQUIRED ON)\nset(CMAKE_CXX_STANDARD 20)\n"),
        Some(2020)
    );
    assert_eq!(
        std_in_cmake("target_compile_features(po PRIVATE cxx_std_17)\n"),
        Some(2017)
    );
    assert_eq!(
        std_in_cmake("add_compile_options(-std=gnu++2a)\n"),
        Some(2020)
    );
    assert_eq!(std_in_cmake("project(po C)\n"), None);
    assert_eq!(
        std_in_flags("[{\"command\": \"c++ -std=c++1z -o a.o -c a.cpp\"}]"),
        Some(2017)
    );
    assert_eq!(std_in_flags("/std:c++latest"), None);
    assert_eq!(std_year("98"), Some(1998));
    assert_eq!(std_year("23 "), Some(2023));
}

/// clangd's answer for a header with a class in a namespace, cut to what is looked at.
#[test]
fn the_new_type_goes_above_the_outermost_declaration_that_is_not_a_namespace() {
    let symbols = serde_json::json!([{
        "kind": 3, "name": "shapes",
        "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 17, "character": 1 } },
        "children": [
            { "kind": 12, "name": "build",
              "range": { "start": { "line": 7, "character": 0 }, "end": { "line": 7, "character": 69 } } },
            { "kind": 5, "name": "Canvas",
              "range": { "start": { "line": 9, "character": 0 }, "end": { "line": 15, "character": 1 } },
              "children": [ { "kind": 6, "name": "draw",
                "range": { "start": { "line": 11, "character": 4 }, "end": { "line": 11, "character": 66 } } } ] }
        ]
    }]);
    let symbols = symbols.as_array().unwrap();
    assert_eq!(outermost_container(symbols, 11), Some(9));
    assert_eq!(outermost_container(symbols, 7), Some(7));
    assert_eq!(outermost_container(symbols, 2), None);
}
