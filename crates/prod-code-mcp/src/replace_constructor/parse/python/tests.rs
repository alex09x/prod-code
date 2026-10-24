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
fn coverage_python_fields_and_class_boundaries() {
    for (input, expected) in [
        (
            "def __init__(self, x: int = 1, y = None):\n pass",
            vec![("x", "int"), ("y", "Any")],
        ),
        (
            "@dataclass\n\n x: int = 1\n def method():\n bad-name: str\n",
            vec![("x", "int")],
        ),
        ("def __init__(self):\n x: int\n", vec![("x", "int")]),
        ("def __init__(", vec![]),
        ("", vec![]),
    ] {
        let fields = parse_python_fields(input);
        assert_eq!(
            fields
                .iter()
                .map(|f| (f.name.as_str(), f.ty.as_str()))
                .collect::<Vec<_>>(),
            expected
        );
    }
    let text = "# α\r\nclass _Item:\r\n    x: int\r\n\r\n# note\r\nvalue = 1\r\n";
    let decl = parse_python_struct_decl(text, "_Item").unwrap();
    assert_eq!((decl.line, decl.col, decl.is_pub), (2, 7, false));
    assert_eq!(
        &text[decl.decl_start..decl.decl_end],
        "class _Item:\r\n    x: int\r\n\r\n# note\r\n"
    );
    for input in ["class NotItem:", "class ItemOther:", "Item = 1", ""] {
        assert_eq!(
            parse_python_struct_decl(input, "Item")
                .err()
                .unwrap()
                .to_string(),
            "cannot find declaration of class `Item` in Python file"
        );
    }
}
