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
use super::super::container::{dataclass_import, python_import_edit};
use super::super::params::{entries, parse_params};
use super::super::rewrite::rewritten_call;
use super::super::syntax::close_in;
use super::super::type_render::{hover_parameter_type, type_text};
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
fn a_python_list_has_a_receiver_markers_and_defaults_with_commas_in_them() {
    let list =
        "self, name: str, width: int, height: int = 2, *, key=\"a, b\",  # the key\n    **kw";
    let (receiver, params) = parse_params(list, Language::Python);
    assert_eq!(receiver.as_deref(), Some("self"));
    let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["name", "width", "height", "*", "key", "kw"]);
    assert_eq!(params[2].ty.as_deref(), Some("int"));
    assert_eq!(params[2].default.as_deref(), Some("2"));
    assert_eq!(params[3].kind, Kind::Marker);
    assert_eq!(params[4].ty, None);
    assert_eq!(params[4].default.as_deref(), Some("\"a, b\""));
    assert_eq!(params[5].kind, Kind::Keywords);
    assert_eq!(
        params[5].raw, "**kw",
        "the comment before it is not part of it"
    );
}

#[test]
fn a_go_group_gives_every_name_its_type() {
    let (receiver, params) = parse_params(
        "name string, width, height int, rest ...string",
        Language::Go,
    );
    assert_eq!(receiver, None);
    let typed: Vec<(&str, &str, bool)> = params
        .iter()
        .map(|p| {
            (
                p.name.as_str(),
                p.ty.as_deref().unwrap_or(""),
                p.shares_type,
            )
        })
        .collect();
    assert_eq!(
        typed,
        [
            ("name", "string", false),
            ("width", "int", true),
            ("height", "int", false),
            ("rest", "...string", false)
        ]
    );
    assert_eq!(params[3].kind, Kind::Variadic);
}

/// The hovers here are what the TypeScript server and basedpyright answer on a parameter.
#[test]
fn a_hover_gives_a_parameters_type_unless_the_server_does_not_know_it() {
    assert_eq!(
        hover_parameter_type("```typescript\n(parameter) width: number\n```\n", "width").as_deref(),
        Some("number")
    );
    assert_eq!(
        hover_parameter_type("```typescript\n(parameter) text: any\n```\n", "text").as_deref(),
        Some("any")
    );
    assert_eq!(
        hover_parameter_type("```python\n(parameter) width: int\n```", "width").as_deref(),
        Some("int")
    );
    assert_eq!(
        hover_parameter_type("```python\n(parameter) a: Unknown\n```", "a"),
        None
    );
    assert_eq!(
        hover_parameter_type("```go\nvar width int\n```", "width"),
        None
    );
}

#[test]
fn each_language_declares_the_type_its_own_way() {
    let fields = [
        field("width", Some("int"), None),
        field("height", Some("int"), Some("2")),
    ];
    assert_eq!(
        type_text(Language::Python, "Size", "build", &fields, "    ", false),
        "@dataclass\nclass Size:\n    \"\"\"The parameters `build` takes together.\"\"\"\n\n    width: int\n    height: int = 2\n"
    );
    let untyped = [field("a", None, None), field("b", None, Some("4"))];
    assert_eq!(
        type_text(Language::Python, "Pair", "loose", &untyped, "    ", false),
        "class Pair:\n    \"\"\"The parameters `loose` takes together.\"\"\"\n\n    def __init__(self, a, b=4):\n        self.a = a\n        self.b = b\n"
    );
    let out_of_order = [
        field("a", Some("int"), Some("1")),
        field("b", Some("int"), None),
    ];
    assert!(
        type_text(Language::Python, "P", "f", &out_of_order, "    ", false)
            .starts_with("@dataclass(kw_only=True)\n")
    );
    let mut optional = field("y", Some("number"), None);
    optional.optional = true;
    assert_eq!(
        type_text(
            Language::TypeScript,
            "Point",
            "draw",
            &[field("x", Some("number"), None), optional],
            "  ",
            true
        ),
        "/** The parameters `draw` takes together. */\nexport interface Point {\n  x: number;\n  y?: number;\n}\n"
    );
    assert_eq!(
        type_text(
            Language::Go,
            "Size",
            "Build",
            &[
                field("width", Some("int"), None),
                field("h", Some("int"), None)
            ],
            "\t",
            false
        ),
        "// Size holds the parameters Build takes together.\ntype Size struct {\n\twidth int\n\th     int\n}\n"
    );
}

#[test]
fn a_python_call_binds_by_position_and_by_keyword() {
    let (_, params) = parse_params("name: str, width: int, height: int = 2", Language::Python);
    let bundled = [1, 2];
    let rewrite = |args: &[&str]| {
        let args = strings(args);
        bind_arguments(&args, &params, Language::Python).map(|bound| {
            rewritten_call(
                &args,
                &bound,
                &bundled,
                &params,
                Language::Python,
                "Size",
                "size",
            )
        })
    };
    assert_eq!(
        rewrite(&["name", "1", "height=2"]).as_deref(),
        Some("name, Size(width=1, height=2)")
    );
    assert_eq!(
        rewrite(&["name", "1"]).as_deref(),
        Some("name, Size(width=1)"),
        "a default the call relied on is the field's default"
    );
    assert_eq!(
        rewrite(&["name", "height=3", "width=1"]).as_deref(),
        Some("name, size=Size(height=3, width=1)"),
        "after a keyword argument the literal is passed by keyword, its keywords in the \
         order they are evaluated (#436)"
    );
    assert_eq!(
        rewrite(&["width=1", "name=n"]).as_deref(),
        Some("size=Size(width=1), name=n")
    );
    assert_eq!(rewrite(&["name"]), None, "`width` has no default");
    assert_eq!(rewrite(&["*xs"]), None, "a spread cannot be mapped");
    assert_eq!(rewrite(&["a", "b", "c", "d"]), None, "too many");
    assert_eq!(
        rewrite(&["a == b", "1", "2"]).as_deref(),
        Some("a == b, Size(width=1, height=2)"),
        "a comparison is not a keyword argument"
    );
}

#[test]
fn typescript_and_go_calls_bind_by_position_with_the_declared_arity() {
    let (_, params) = parse_params("label: string, x: number, y: number", Language::TypeScript);
    let args = strings(&["\"b\"", "5", "6"]);
    let bound = bind_arguments(&args, &params, Language::TypeScript).expect("same arity");
    assert_eq!(
        rewritten_call(
            &args,
            &bound,
            &[1, 2],
            &params,
            Language::TypeScript,
            "Point",
            "point"
        ),
        "\"b\", { x: 5, y: 6 }"
    );
    assert!(bind_arguments(&strings(&["\"b\"", "5"]), &params, Language::TypeScript).is_none());

    let (_, params) = parse_params("name string, width, height int", Language::Go);
    let args = strings(&["\"a\"", "3", "4"]);
    let bound = bind_arguments(&args, &params, Language::Go).expect("same arity");
    assert_eq!(
        rewritten_call(
            &args,
            &bound,
            &[1, 2],
            &params,
            Language::Go,
            "shapes.Size",
            "size"
        ),
        "\"a\", shapes.Size{width: 3, height: 4}"
    );
}

#[test]
fn strings_and_comments_are_skipped_the_way_each_language_writes_them() {
    // An apostrophe opens a string in these languages, not a lifetime.
    let ts = "f('a, (b', `c, ${d}`, /* e, ) */ g) // h, )\n";
    let close = close_in(ts, 1, Language::TypeScript).expect("it closes");
    assert_eq!(&ts[close..close + 1], ")");
    let args: Vec<&str> = entries(&ts[2..close], Language::TypeScript)
        .into_iter()
        .map(|(_, a)| a)
        .collect();
    assert_eq!(args, ["'a, (b'", "`c, ${d}`", "g"]);
    // `//` divides in Python; `#` comments.
    let py = "f(a // 2, \"\"\"x, )\"\"\", b)  # c, )\n";
    let close = close_in(py, 1, Language::Python).expect("it closes");
    let args: Vec<&str> = entries(&py[2..close], Language::Python)
        .into_iter()
        .map(|(_, a)| a)
        .collect();
    assert_eq!(args, ["a // 2", "\"\"\"x, )\"\"\"", "b"]);
    let go = "f('(', `a, \\`, b)";
    let close = close_in(go, 1, Language::Go).expect("it closes");
    assert_eq!(close, go.len() - 1, "a Go raw string has no escapes");
}

#[test]
fn a_python_import_from_the_declaring_module_gains_the_type() {
    let one_line = "from app.home import build, Canvas\n\nx = 1\n";
    let (at, insert) = python_import_edit(one_line, "home", "Size").expect("an import");
    let mut out = one_line.to_string();
    out.insert_str(at, &insert);
    assert_eq!(out, "from app.home import build, Canvas, Size\n\nx = 1\n");

    let wrapped = "from .home import (\n    build,\n)\n";
    let (at, insert) = python_import_edit(wrapped, "home", "Size").expect("an import");
    let mut out = wrapped.to_string();
    out.insert_str(at, &insert);
    assert_eq!(out, "from .home import (\n    build, Size,\n)\n");

    let (_, insert) =
        python_import_edit("from app.home import Size, build\n", "home", "Size").unwrap();
    assert!(insert.is_empty(), "already imported");
    assert!(python_import_edit("import app.home\n", "home", "Size").is_none());

    assert_eq!(
        dataclass_import("\"\"\"Shapes.\"\"\"\n\nimport math\n\n\ndef f():\n    pass\n"),
        Some((27, "from dataclasses import dataclass\n".to_string()))
    );
    assert_eq!(
        dataclass_import("from dataclasses import dataclass, field\n"),
        None
    );
    assert_eq!(
        dataclass_import("def f():\n    pass\n"),
        Some((0, "from dataclasses import dataclass\n\n".to_string()))
    );
}
