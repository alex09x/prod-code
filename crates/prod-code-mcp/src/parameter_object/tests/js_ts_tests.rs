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
use super::super::container::in_import;
use super::super::effects::js_constant;
use super::super::params::parse_params;
use super::super::rewrite::{called_name, ident_uses, object_shorthand, rewritten_call};
use super::super::type_render::{js_key, literal_text, parameter_in, type_text};
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
fn a_typescript_list_keeps_optional_marks_defaults_and_nested_type_arguments() {
    let list = "this: Canvas, label: string, x?: number, m: Map<string, number> = new Map(), cb: (a: number, b: number) => void, public readonly id: string, ...rest: number[]";
    let (receiver, params) = parse_params(list, Language::TypeScript);
    assert_eq!(receiver.as_deref(), Some("this: Canvas"));
    let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["label", "x", "m", "cb", "id", "rest"]);
    assert!(params[1].optional);
    assert_eq!(params[1].ty.as_deref(), Some("number"));
    assert_eq!(params[2].ty.as_deref(), Some("Map<string, number>"));
    assert_eq!(params[2].default.as_deref(), Some("new Map()"));
    assert_eq!(
        params[3].ty.as_deref(),
        Some("(a: number, b: number) => void")
    );
    assert_eq!(params[3].default, None, "`=>` is not a default");
    assert_eq!(&list[params[4].name_at..params[4].name_at + 2], "id");
    assert_eq!(params[5].kind, Kind::Variadic);
}

#[test]
fn a_javascript_list_has_no_types_a_pattern_has_no_name_and_rest_is_variadic() {
    let list = "label, { x, y } = {}, width = 2, ...rest";
    let (receiver, params) = parse_params(list, Language::JavaScript);
    assert_eq!(receiver, None, "JavaScript has no `this` parameter");
    let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["label", "", "width", "rest"]);
    assert!(params.iter().all(|p| p.ty.is_none()), "{params:?}");
    assert_eq!(params[1].default.as_deref(), Some("{}"));
    assert_eq!(params[2].default.as_deref(), Some("2"));
    assert_eq!(params[3].kind, Kind::Variadic);
    assert_eq!(&list[params[3].name_at..params[3].name_at + 4], "rest");
}

#[test]
fn a_javascript_call_binds_by_position_and_the_object_carries_constant_defaults() {
    let (_, params) = parse_params("name, width, height = 2", Language::JavaScript);
    let call = |args: &[&str]| {
        let args = strings(args);
        let bound = bind_arguments(&args, &params, Language::JavaScript)?;
        Some(rewritten_call(
            &args,
            &bound,
            &[1, 2],
            &params,
            Language::JavaScript,
            "Size",
            "size",
        ))
    };
    assert_eq!(
        call(&["\"a\"", "3", "4"]).as_deref(),
        Some("\"a\", { width: 3, height: 4 }")
    );
    // Left out or `undefined`, the parameter had its default; the object says so.
    assert_eq!(
        call(&["\"a\"", "3"]).as_deref(),
        Some("\"a\", { width: 3, height: 2 }")
    );
    assert_eq!(
        call(&["\"a\"", "3", "void 0"]).as_deref(),
        Some("\"a\", { width: 3, height: 2 }")
    );
    // `undefined` is a name a scope may give another value; it is passed on as written.
    assert_eq!(
        call(&["\"a\"", "3", "undefined"]).as_deref(),
        Some("\"a\", { width: 3, height: undefined }")
    );
    // A call that stopped before the bundle still passes the object the body reads, with
    // what it left out as own fields.
    assert_eq!(
        call(&["\"a\""]).as_deref(),
        Some("\"a\", { width: void 0, height: 2 }")
    );
    assert_eq!(
        call(&[]).as_deref(),
        Some("void 0, { width: void 0, height: 2 }")
    );
    // An argument past the last parameter is evaluated as before, and still goes nowhere.
    assert_eq!(
        call(&["\"a\"", "3", "4", "log()"]).as_deref(),
        Some("\"a\", { width: 3, height: 4 }, log()")
    );
    assert_eq!(call(&["...pair"]), None, "a spread has no position");

    let (_, params) = parse_params("a, b, ...more", Language::JavaScript);
    let args = strings(&["1", "2", "3", "4"]);
    let bound = bind_arguments(&args, &params, Language::JavaScript).expect("bound");
    assert_eq!(
        rewritten_call(
            &args,
            &bound,
            &[0, 1],
            &params,
            Language::JavaScript,
            "Pair",
            "pair"
        ),
        "{ a: 1, b: 2 }, 3, 4",
        "what the rest parameter took stays after the object"
    );
}

#[test]
fn only_a_constant_javascript_default_moves_to_the_callers() {
    for constant in [
        "2", "-1.5", ".5", "1.", "1.5e-3", "1.e5", "0xff", "1_000", "10n", "\"a, b\"", "'it\\'s'",
        "`plain`", "true", "null", "void 0", "[]", "{}",
    ] {
        assert!(js_constant(constant), "{constant}");
    }
    for expr in [
        "Date.now()",
        "x",
        "-x",
        "\"a\" + \"b\"",
        "`${x}`",
        "[1]",
        "{ a: 1 }",
        "new Map()",
        // A name, which a parameter or a variable can shadow.
        "undefined",
        "void x",
        // A property of a number, `undefined` when there is none.
        "1..missing",
        "1.x",
        "1.e",
        "0xe+1",
        "1.5.x",
        // The closing quote is escaped: the literal does not end there.
        "\"a\\\"",
    ] {
        assert!(!js_constant(expr), "{expr}");
    }
}

#[test]
fn a_typescript_call_may_leave_off_a_defaulted_parameter_and_the_literal_carries_it() {
    let ts = Language::TypeScript;
    let (_, params) = parse_params("text: string, width: number, fill: string = \" \"", ts);
    let call = |args: &[&str]| {
        let args = strings(args);
        let bound = bind_arguments(&args, &params, ts)?;
        Some(rewritten_call(
            &args,
            &bound,
            &[1, 2],
            &params,
            ts,
            "Pad",
            "pad",
        ))
    };
    assert_eq!(
        call(&["\"a\"", "3"]).as_deref(),
        Some("\"a\", { width: 3, fill: \" \" }")
    );
    assert_eq!(
        call(&["\"a\"", "3", "void 0"]).as_deref(),
        Some("\"a\", { width: 3, fill: \" \" }")
    );
    assert_eq!(
        call(&["\"a\"", "3", "\"*\""]).as_deref(),
        Some("\"a\", { width: 3, fill: \"*\" }")
    );
    assert_eq!(call(&["\"a\""]), None, "`width` has no default");
    assert_eq!(call(&["\"a\"", "3", "\"*\"", "4"]), None, "one too many");
    assert_eq!(call(&["\"a\"", "...rest"]), None, "a spread");

    // An optional parameter may be left off too, and a rest one given nothing.
    let (_, params) = parse_params("a: number, b?: number, ...more: number[]", ts);
    assert_eq!(
        bind_arguments(&strings(&["1"]), &params, ts),
        Some(vec![Some(0)])
    );
}

#[test]
fn a_proto_field_is_an_own_property_in_the_literal_and_the_body() {
    assert_eq!(
        literal_text(
            Language::JavaScript,
            "Tag",
            &[
                ("__proto__".to_string(), "p".to_string()),
                ("name".to_string(), "n".to_string())
            ]
        ),
        "{ [\"__proto__\"]: p, name: n }"
    );
    assert_eq!(
        literal_text(
            Language::TypeScript,
            "Tag",
            &[("__proto__".to_string(), "p".to_string())]
        ),
        "{ [\"__proto__\"]: p }"
    );
    assert_eq!(js_key("proto"), "proto");
}

#[test]
fn a_javascript_object_is_written_nowhere_and_its_literal_names_no_type() {
    let fields = [field("width", None, None), field("height", None, Some("2"))];
    let text = type_text(Language::JavaScript, "Size", "build", &fields, "  ", true);
    assert_eq!(
        text,
        "// Size: the plain object `build` takes; nothing is declared for it\n{ width, height = 2 }\n"
    );
    assert_eq!(parameter_in(Language::JavaScript, "size", "Size"), "size");
    assert_eq!(literal_text(Language::JavaScript, "Size", &[]), "{}");
    assert_eq!(
        literal_text(
            Language::JavaScript,
            "Size",
            &[("width".to_string(), "3".to_string())]
        ),
        "{ width: 3 }"
    );
}

#[test]
fn a_shorthand_property_is_told_from_a_block_and_a_jsx_expression() {
    let text = "function f(width) {\n  const o = { width, h: 1 };\n  g({ width });\n  ({ width } = o);\n  if (o) { width }\n  const k = () => { width };\n  const v = <p w={width}>{width}</p>;\n  return { width };\n}\n";
    let from = text.find(')').expect("the list closes");
    let at = |needle: &str| text.find(needle).expect(needle) + needle.find("width").unwrap();
    let shorthand = |needle: &str| object_shorthand(text, from, at(needle), "width".len());
    assert!(shorthand("{ width, h"), "an object literal after `=`");
    assert!(shorthand("g({ width"), "an object literal argument");
    assert!(shorthand("({ width } = o"), "a destructuring assignment");
    assert!(
        shorthand("return { width"),
        "an object literal after `return`"
    );
    assert!(!shorthand("(o) { width"), "a block after `)`");
    assert!(!shorthand("=> { width"), "an arrow's block");
    assert!(!shorthand("w={width"), "a JSX attribute");
    assert!(!shorthand(">{width"), "a JSX child");
}

#[test]
fn a_name_is_found_in_code_and_substitutions_not_in_strings_keys_or_properties() {
    let text =
        "{ const s = `${size} x`; return a.size + size + { size: 1 }.size; /* size */ \"size\"; }";
    let found = ident_uses(text, 0, text.len(), "size");
    let expected = [
        text.find("${size}").unwrap() + 2,
        text.find("+ size +").unwrap() + 2,
    ];
    assert_eq!(found, expected, "{text}");
    assert!(ident_uses(text, 0, text.len(), "siz").is_empty());
}

#[test]
fn a_javascript_call_may_use_the_name_an_import_gave_the_function() {
    let text = "import { build as make } from \"./home\";\nconst { build: other } = require(\"./home\");\nimport dflt from \"./home\";\nconst whole = require(\"./home\");\nmake(1); other(2); dflt(3); whole(4); stranger(5); build(6); builder(7);\n";
    let at = |needle: &str| text.find(needle).expect(needle);
    let js = |needle: &str| called_name(text, at(needle), "build", Language::JavaScript);
    assert_eq!(js("make(1"), Some(4));
    assert_eq!(js("other(2"), Some(5));
    assert_eq!(js("dflt(3"), Some(4));
    assert_eq!(js("whole(4"), Some(5));
    assert_eq!(js("build(6"), Some(5));
    assert_eq!(js("stranger(5"), None, "a name nothing aliases");
    assert_eq!(js("builder(7"), None, "a longer name is another name");
    assert!(in_import(text, at("other }"), Language::JavaScript));
    assert!(!in_import(text, at("make(1"), Language::JavaScript));
}
