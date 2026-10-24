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

#[test]
fn coverage_ts_and_javascript_field_shapes() {
    for (input, expected) in [
        (
            "constructor(public x: number, private x: string, protected readonly y: string, bad-name: boolean) {}",
            vec![("x", "number"), ("y", "string")],
        ),
        (
            "\n// note\n/* note */\nthis.x = 1;\nreturn x;\nmethod() {}\nprivate x: number;\nreadonly y: string;\nx: boolean;\nbad-name: int;\n",
            vec![("x", "number"), ("y", "string")],
        ),
        ("constructor(\nx: number", vec![("x", "number")]),
        ("", vec![]),
    ] {
        assert_eq!(
            parse_ts_fields(input)
                .iter()
                .map(|f| (f.name.as_str(), f.ty.as_str()))
                .collect::<Vec<_>>(),
            expected
        );
    }
    for (input, expected) in [
        (
            "\n// ignored\nx = 1;\nx;\nstatic y;\nmethod() {\n local = 3;\n}\nconstructor(x, y, z = 2, ...rest) {\nthis.x = x;\nthis.z = z;\nthis.rest = rest;\n}\n",
            vec!["x", "y", "z", "rest"],
        ),
        ("constructor(x", vec![]),
        ("constructor(x)", vec![]),
        ("constructor(x) {", vec![]),
        ("constructor({x}, unused) { this.x = x; }", vec![]),
        ("", vec![]),
    ] {
        let fields = parse_javascript_fields(input);
        assert_eq!(
            fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            expected,
            "{input}"
        );
        assert!(fields.iter().all(|f| f.ty.is_empty() && f.vis.is_empty()));
    }
}

#[test]
fn coverage_ts_declaration_boundaries() {
    let text = "// α\r\nconst NotItem = 1;\r\nexport class Item {\r\n x: number;\r\n}\r\n";
    let decl = parse_ts_struct_decl(text, "Item", "typescriptreact").unwrap();
    assert_eq!(
        (decl.name.as_str(), decl.language.as_str(), decl.is_pub),
        ("Item", "typescriptreact", true)
    );
    assert_eq!((decl.line, decl.col), (3, 14));
    assert_eq!(
        &text[decl.decl_start..decl.decl_end],
        "Item {\r\n x: number;\r\n}"
    );
    for text in [
        "class NotItem {}",
        "class ItemOther {}",
        "const Item = 3;",
        "class Item",
        "class Item {",
    ] {
        assert_eq!(
            parse_ts_struct_decl(text, "Item", "typescript")
                .err()
                .unwrap()
                .to_string(),
            "cannot find declaration of class/interface `Item` in TypeScript/JavaScript file"
        );
    }
    let decl = parse_ts_struct_decl("class Item {\nx = 1;\n}", "Item", "javascriptreact").unwrap();
    assert_eq!(decl.fields[0].name, "x");
    assert!(!decl.is_pub);
}
