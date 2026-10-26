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

fn record(name: &str, extension: &str, before: &str, after: &str) {
    eprintln!(
        "REPAIR_SOURCE {}",
        serde_json::json!({"name": name, "extension": extension, "before": before, "after": after})
    );
}

#[tokio::test]
async fn review18_typescript_keyof_object_boundary() {
    let before = "class C {\n    value: \"value\" = \"value\";\n    static same(c: C): keyof { value: number } { return c.value; }\n}\nconsole.log(C.same(new C()));\n";
    let (after, _) = convert(before, "ts", true).await;
    record("keyof", "ts", before, &after);
    assert!(
        after.contains(": keyof { value: number } { return this.value; }"),
        "{after}"
    );
    assert_eq!(run("node", "ts", &after), run("node", "ts", before));
}

#[tokio::test]
async fn review18_typescript_callable_boundary() {
    let before = "class C {\n    value = () => 7;\n    static same(c: C): () => number { return c.value; }\n}\nconsole.log(C.same(new C())());\n";
    eprintln!("ORIGINAL callable:\n{before}");
    let (after, _) = convert(before, "ts", true).await;
    record("callable", "ts", before, &after);
    assert_eq!(run("node", "ts", &after), run("node", "ts", before));
}

#[tokio::test]
async fn review18_python_logical_suite_boundary() {
    let before = "class C:\n    @staticmethod\n    def same(c):\n        return (\nc\n        )\nvalue = C()\nprint(C.same(value) is value)\n";
    let (after, _) = convert(before, "py", true).await;
    record("continued", "py", before, &after);
    assert_eq!(run("python3", "py", &after), run("python3", "py", before));
}

#[tokio::test]
async fn review18_swift_regex_executable_mask() {
    let before = "class C {\n    var value = 7\n    static func same(c: C) -> Regex<Substring> { return #/c.value/# }\n}\nlet pattern = #/C.same(c: C())/#\nprint(\"c.value\".wholeMatch(of: C.same(c: C())) != nil)\nprint(\"C.same(c: C())\".wholeMatch(of: pattern) != nil)\n";
    let (after, _) = convert(before, "swift", true).await;
    record("regex_receiver", "swift", before, &after);
    assert!(after.contains("return #/c.value/#"), "{after}");
    assert!(
        after.contains("let pattern = #/C.same(c: C())/#"),
        "{after}"
    );
    assert!(after.contains("C().same()"), "{after}");
}

#[tokio::test]
async fn review18_typescript_complete_result_annotations() {
    for (ty, value, call) in [
        ("{ value: number }", "{ value: 7 }", ".value"),
        (
            "() => { value: number }",
            "() => ({ value: 7 })",
            "().value",
        ),
        (
            "Array<() => { value: number }>",
            "[() => ({ value: 7 })]",
            "[0]().value",
        ),
        (
            "{ read: () => number; nested: { value: number } }",
            "{ read: () => 7, nested: { value: 7 } }",
            ".read()",
        ),
        (
            "readonly [number, { value: number }]",
            "[7, { value: 7 }] as const",
            "[0]",
        ),
        ("'>' /* > < is comment text */", "'>' as const", ""),
    ] {
        let before = format!(
            "class C {{\n    value = {value};\n    static same(c: C): {ty} {{ return c.value; }}\n}}\nconsole.log(C.same(new C()){call});\n"
        );
        let (after, _) = convert(&before, "ts", true).await;
        record(ty, "ts", &before, &after);
        assert!(
            after.contains(&format!(": {ty} {{ return this.value; }}")),
            "{after}"
        );
        assert_eq!(run("node", "ts", &after), run("node", "ts", &before));
    }
}

#[tokio::test]
async fn review18_python_continuations_cover_class_and_selected_suite() {
    for body in [
        "        return [\nc\n        ][0]",
        "        return {\n'value': c\n        }['value']",
        "        return \\\nc",
        "        text = '''first\ncolumn zero\n        '''\n        return (\nc\n        )",
    ] {
        let before = format!(
            "class C:\n    values = (\n0,\n    )\n    @staticmethod\n    def same(c):\n{body}\n    def untouched(self):\n        return 9\nvalue = C()\nprint(C.same(value) is value)\nprint(value.untouched())\n"
        );
        let (after, _) = convert(&before, "py", true).await;
        record("logical_controls", "py", &before, &after);
        assert!(after.contains("    def untouched(self):\n        return 9"));
        assert_eq!(run("python3", "py", &after), run("python3", "py", &before));
    }
    super::support::refuses(
        "class C:\n    @staticmethod\n    def same(c):\n        return (\nc\n",
        "py",
    )
    .await;
}

#[tokio::test]
async fn review19_typescript_numeric_literal_results() {
    for literal in [
        "1.5", "-1.5", ".5", "-.5", "1.5e-3", "1E+3", "0xff", "1_000.5", "12n",
    ] {
        let before = format!(
            "class C {{\n    value: {literal} = {literal};\n    static same(c: C): {literal} {{ return c.value; }}\n}}\nconsole.log(String(C.same(new C())));\n"
        );
        let (after, _) = convert(&before, "ts", true).await;
        record(literal, "ts", &before, &after);
        assert!(
            after.contains(&format!(": {literal} {{ return this.value; }}")),
            "{after}"
        );
        assert_eq!(run("node", "ts", &after), run("node", "ts", &before));
    }
    let before = "declare namespace Names { type Value1 = { c: number }; }\nclass C {\n    value = { c: 7 };\n    static same(c: C): Names.Value1 { return c.value; }\n}\nconsole.log(C.same(new C()).c);\n";
    let (after, _) = convert(before, "ts", true).await;
    record("qualified_numeric_identifier", "ts", before, &after);
    assert!(
        after.contains(": Names.Value1 { return this.value; }"),
        "{after}"
    );
    assert_eq!(run("node", "ts", &after), run("node", "ts", before));
}

#[tokio::test]
async fn review19_typescript_predicate_and_assertion_results() {
    for (annotation, body) in [
        ("value is number", "return typeof value === 'number';"),
        (
            "asserts value is number",
            "if (typeof value !== 'number') throw Error('number');",
        ),
        ("asserts value", "if (!value) throw Error('truthy');"),
    ] {
        let before = format!(
            "class C {{\n    static same(c: C, value: unknown): {annotation} {{ {body} }}\n}}\nconsole.log(C.same(new C(), 7));\n"
        );
        let (after, _) = convert(&before, "ts", true).await;
        record(annotation, "ts", &before, &after);
        assert!(
            after.contains(&format!("same(value: unknown): {annotation}")),
            "{after}"
        );
        assert_eq!(run("node", "ts", &after), run("node", "ts", &before));
    }
    for annotation in ["c is C", "asserts c is C", "asserts c"] {
        super::support::refuses(
            &format!(
                "class C {{\n    static same(c: C): {annotation} {{ throw Error('stop'); }}\n}}\n"
            ),
            "ts",
        )
        .await;
    }
}

#[tokio::test]
async fn review19_typescript_contextual_predicate_names_are_types() {
    for (declaration, annotation) in [
        ("type asserts = number;", "asserts"),
        (
            "declare namespace asserts { type c = number; }",
            "asserts.c",
        ),
        ("declare namespace c { type is = number; }", "c.is"),
    ] {
        let before = format!(
            "{declaration}\nclass C {{\n    value = 7;\n    static same(c: C): {annotation} {{ return c.value; }}\n}}\nconsole.log(C.same(new C()));\n"
        );
        let (after, _) = convert(&before, "ts", true).await;
        record(annotation, "ts", &before, &after);
        assert!(
            after.contains(&format!(": {annotation} {{ return this.value; }}")),
            "{after}"
        );
        assert_eq!(run("node", "ts", &after), run("node", "ts", &before));
    }
}

#[tokio::test]
async fn review20_typescript_escaped_result_identifiers() {
    for (declaration, annotation) in [
        ("type V = number;", r"\u0056"),
        ("type Value = number;", r"V\u0061lue"),
        ("type V = number;", r"\u{56}"),
        (
            "declare namespace Names { type V = number; }",
            r"Names.\u0056",
        ),
    ] {
        let before = format!(
            "{declaration}\nclass C {{\n    value = 4;\n    static same(c: C): {annotation} {{ return c.value; }}\n}}\nconsole.log(C.same(new C()));\n"
        );
        record(annotation, "ts", &before, "conversion pending");
        assert_eq!(run("node", "ts", &before), "4\n");
        let (after, _) = convert(&before, "ts", true).await;
        record(annotation, "ts", &before, &after);
        assert!(
            after.contains(&format!(": {annotation} {{ return this.value; }}")),
            "{after}"
        );
        assert_eq!(run("node", "ts", &after), "4\n");
    }
    for annotation in [r"\u0063 is C", r"asserts \u{63} is C", r"asserts \u0063"] {
        super::support::refuses(
            &format!(
                "class C {{\n    static same(c: C): {annotation} {{ throw Error('stop'); }}\n}}\n"
            ),
            "ts",
        )
        .await;
    }
}

#[tokio::test]
async fn review21_typescript_abstract_type_name_and_constructor_modifier() {
    for (declaration, annotation, value, call, expected) in [
        ("type abstract = number;", "abstract", "7", "", "7\n"),
        ("type abstract<T> = T;", "abstract<number>", "7", "", "7\n"),
        (
            "declare namespace abstract { type Value = number; }",
            "abstract.Value",
            "7",
            "",
            "7\n",
        ),
        (
            "",
            "abstract new () => { value: number }",
            "class { value = 7; }",
            ".name === 'value'",
            "true\n",
        ),
    ] {
        let before = format!(
            "{declaration}\nclass C {{\n    value = {value};\n    static same(c: C): {annotation} {{ return c.value; }}\n}}\nconsole.log(C.same(new C()){call});\n"
        );
        record(annotation, "ts", &before, "conversion pending");
        assert_eq!(run("node", "ts", &before), expected);
        let (after, _) = convert(&before, "ts", true).await;
        record(annotation, "ts", &before, &after);
        assert!(
            after.contains(&format!(": {annotation} {{ return this.value; }}")),
            "{after}"
        );
        assert_eq!(run("node", "ts", &after), expected);
    }
}

#[tokio::test]
async fn review22_typescript_nested_template_result_annotation() {
    for (annotation, value) in [
        ("`${`x`}`", "x"),
        (r#"`a${`b${"x"}`}c`"#, "abxc"),
        (r#"`${{ value: "x" }["value"]}`"#, "x"),
        (r#"`${/* } ` */ `x`}`"#, "x"),
        (r#"`${"'"}${`x`}`"#, "'x"),
        (r#"`\`${`x`}\``"#, "`x`"),
    ] {
        let before = format!(
            "class C {{\n    value: {annotation} = \"{value}\";\n    static same(c: C): {annotation} {{ return c.value; }}\n}}\nconsole.log(C.same(new C()));\n"
        );
        record(annotation, "ts", &before, "conversion pending");
        let expected = format!("{value}\n");
        assert_eq!(run("node", "ts", &before), expected);
        let (after, _) = convert(&before, "ts", true).await;
        record(annotation, "ts", &before, &after);
        assert!(
            after.contains(&format!(": {annotation} {{ return this.value; }}")),
            "{after}"
        );
        assert_eq!(run("node", "ts", &after), expected);
    }
}
