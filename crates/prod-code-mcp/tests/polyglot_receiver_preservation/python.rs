/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{convert, run};

#[tokio::test]
async fn repair_python_suite_indent_ignores_leading_comments() {
    for body in [
        "    # c.value is a comment\n\n        # suite comment\n        return c.value",
        "    # c.value is a comment\n        \"\"\"doc\"\"\"\n        return c.value",
        "    # c.value is a comment\n        \"\"\"doc only\"\"\"",
    ] {
        let before = format!(
            "class C:\n    value = 7\n    @staticmethod\n    def same(c, other):\n{body}\nprint(C.same(C(), C()))\nprint(C.same.__doc__)\n"
        );
        let expected = run("python3", "py", &before);
        let (after, _) = convert(&before, "py", true).await;
        eprintln!("BEFORE:\n{before}AFTER:\n{after}");
        assert!(after.contains("\n        c = self\n"), "{after}");
        assert!(after.contains("\n    # c.value is a comment\n"), "{after}");
        if body.contains("suite comment") {
            assert_eq!(
                after,
                "class C:\n    value = 7\n    def same(self, other):\n    # c.value is a comment\n\n        # suite comment\n        c = self\n        return c.value\nprint(C().same(C()))\nprint(C.same.__doc__)\n"
            );
        }
        assert_eq!(run("python3", "py", &after), expected);
    }
}

#[tokio::test]
async fn review_multiline_docstring_with_trailing_statement_preserves_binding_and_doc() {
    let before = "class C:\n    value = 7\n    @staticmethod\n    def same(c, other):\n        \"\"\"doc\n        end\"\"\"; return c.value\nprint(C.same(C(), C()))\nprint(C.same.__doc__)\n";
    let expected = run("python3", "py", before);
    assert_eq!(expected, "7\ndoc\n        end\n");
    let (after, _) = convert(before, "py", true).await;
    eprintln!("BEFORE:\n{before}AFTER:\n{after}");
    assert_eq!(run("python3", "py", &after), expected);
}

#[tokio::test]
async fn same_line_python_direct_identity() {
    let before = "class C:\n    @staticmethod\n    def same(c, other): return c is other\nvalue = C()\nprint(C.same(value, value))\n";
    let (after, _) = convert(before, "py", true).await;
    assert_eq!(run("python3", "py", &after), run("python3", "py", before));
}

#[tokio::test]
async fn python_capture_shadowing_nonlocal_writes_indexing_and_docstring() {
    let before = r#"class C:
    def __init__(self, value):
        self.value = value
    def __getitem__(self, index):
        return self.value
    @staticmethod
    def same(c, other):
        """c.value stays a docstring"""
        def capture():
            return c.value
        def shadow(c):
            return c.value
        def change():
            nonlocal c
            c = other
        first = capture()
        indexed = c[0]
        text = "c.value"
        # c.value stays a comment
        change()
        return first, indexed, capture(), shadow(C(9)), c is other, text
print(C.same(C(3), C(5)))
print(C.same.__doc__)
"#;
    let (after, _) = convert(before, "py", true).await;
    assert!(after.contains("def shadow(c):\n            return c.value"));
    assert!(after.contains("# c.value stays a comment"));
    assert_eq!(run("python3", "py", &after), run("python3", "py", before));
}

#[tokio::test]
async fn python_argument_and_interpolation_keep_the_original_binding() {
    let before = "class C:\n    @staticmethod\n    def same(c, other):\n        def take(value): return value\n        return take(c) is other, f'c is {c is other}'\nvalue = C()\nprint(C.same(value, value))\n";
    let (after, _) = convert(before, "py", false).await;
    assert_eq!(run("python3", "py", &after), run("python3", "py", before));
}

#[tokio::test]
async fn python_comments_on_signature_and_docstring_only_bodies() {
    for body in [
        " # c.value comment\n        return c is other",
        " 'c.value doc'; return c is other",
        "\n        '''c.value\n        docstring only'''",
    ] {
        let before = format!(
            "class C:\n    @staticmethod\n    def same(c, other):{body}\nvalue = C()\nprint(C.same(value, value))\nprint(C.same.__doc__)\n"
        );
        let (after, _) = convert(&before, "py", true).await;
        assert_eq!(run("python3", "py", &after), run("python3", "py", &before));
    }
}
