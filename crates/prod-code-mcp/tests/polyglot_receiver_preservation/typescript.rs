/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{convert, refuses, run};

#[tokio::test]
async fn review6_typescript_qualified_extends_return_type_converts() {
    for ty in [
        "typeof schema.extends",
        "typeof schema. /* property */ extends",
    ] {
        let before = format!(
            "const schema = {{ extends: 7 }};\nclass C {{\n    value = 7;\n    static same(c: C): {ty} {{ return c.value; }}\n}}\nconsole.log(C.same(new C()));\n"
        );
        eprintln!("BEFORE:\n{before}");
        let expected = run("node", "ts", &before);
        eprintln!("ORIGINAL Node output: {expected}");
        let (after, _) = convert(&before, "ts", true).await;
        eprintln!("AFTER:\n{after}");
        assert!(after.contains(ty), "{after}");
        let actual = run("node", "ts", &after);
        eprintln!("CONVERTED Node output: {actual}");
        assert_eq!(actual, expected);
    }
}

#[tokio::test]
async fn review_hoisted_promoted_function_binding_refuses_before_writes() {
    let before = "class C {\n    static same(c, other) { function c() { return 7; } return c(); }\n}\nconsole.log(C.same(new C(), new C()));\n";
    assert_eq!(run("node", "js", before), "7\n");
    if prod_code_mcp::to_method::to_method_ts(before, Some("C"), "same").is_ok() {
        let (after, _) = convert(before, "js", true).await;
        eprintln!("BEFORE:\n{before}AFTER:\n{after}");
        assert_eq!(run("node", "js", &after), "7\n");
    }
    refuses(before, "js").await;
}

#[tokio::test]
async fn same_line_direct_binding_survives_public_apply() {
    let before = "class C {\n    static same(c, other){return c===other;}\n}\nconst value = new C();\nconsole.log(C.same(value, value));\n";
    let expected = run("node", "js", before);
    let (after, _) = convert(before, "js", true).await;
    eprintln!("BEFORE:\n{before}AFTER:\n{after}");
    assert_eq!(run("node", "js", &after), expected);
}

#[tokio::test]
async fn typed_same_line_direct_binding_is_preserved_in_preview() {
    let before = "class C {\n    static same(c:C, other:C){return c===other;}\n}\n";
    let (after, _) = convert(before, "ts", false).await;
    assert!(
        after.contains("return this===other") || after.contains("var c: C = this;"),
        "{after}"
    );
}

#[tokio::test]
async fn direct_index_argument_shorthand_and_unrelated_member_reads() {
    let before = r#"class C {
    value = 7;
    static same(c, other) {
        const take = value => value.value;
        const object = {c: other};
        // c.value } must remain literal comment text
        const text = "c.value }";
        return [c === other, c["value"], take(c), {c}.c.value, object.c.value, text, `value ${c.value}`];
    }
}
console.log(JSON.stringify(C.same(new C(), new C())));
"#;
    let (after, _) = convert(before, "js", true).await;
    assert!(after.contains("// c.value } must remain literal comment text"));
    assert!(after.contains("\"c.value }\""));
    assert!(after.contains("object.c.value"));
    assert_eq!(run("node", "js", &after), run("node", "js", before));
}

#[tokio::test]
async fn nested_captures_shadowing_and_parameter_writes_keep_binding_identity() {
    let before = r#"class C {
    constructor(value) { this.value = value; }
    static same(c, other) {
        function capture() { return c.value; }
        const shadow = (c) => c.value;
        let block;
        { let c = {value: 11}; block = c.value; }
        const first = capture();
        c = other;
        return [first, capture(), shadow({value: 13}), block, c === other];
    }
}
console.log(JSON.stringify(C.same(new C(3), new C(5))));
"#;
    let (after, _) = convert(before, "js", true).await;
    assert!(after.contains("function capture() { return c.value; }"));
    assert!(after.contains("(c) => c.value"));
    assert_eq!(run("node", "js", &after), "[3,5,13,11,true]\n");
    assert_eq!(run("node", "js", &after), run("node", "js", before));
}

#[tokio::test]
async fn escaping_closure_keeps_the_promoted_binding() {
    let before = "class C {\n    static same(c, other) { return function() { return c === other; }; }\n}\nconst value = new C();\nconsole.log(C.same(value, value)());\n";
    let (after, _) = convert(before, "js", true).await;
    assert_eq!(run("node", "js", &after), run("node", "js", before));
}
