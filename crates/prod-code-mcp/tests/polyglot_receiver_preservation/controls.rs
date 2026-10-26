/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{convert, refuses};
use prod_code_mcp::to_method::{
    to_method_cpp, to_method_go, to_method_py, to_method_swift, to_method_ts,
};

#[test]
fn existing_property_access_spelling_and_counts_stay_supported() {
    let ts = to_method_ts(
        "class C {\n    static same(c:C, n:number) {\n        return c.value + n;\n    }\n}\n",
        Some("C"),
        "same",
    )
    .unwrap();
    assert!(ts.4.contains("return this.value + n;"));
    assert_eq!(ts.5, 1);
    let py = to_method_py(
        "class C:\n    @staticmethod\n    def same(c, n):\n        return c.value + n\n",
        Some("C"),
        "same",
    )
    .unwrap();
    assert!(py.4.contains("return self.value + n"));
    assert_eq!(py.5, 1);
    let cpp = to_method_cpp(
        "struct C {\n    static int same(C& c, int n) { return c.value + n; }\n};\n",
        Some("C"),
        "same",
    )
    .unwrap();
    assert!(cpp.4.contains("return this->value + n;"));
    assert_eq!(cpp.5, 1);
    let swift = to_method_swift(
        "class C {\n    static func same(c:C, n:Int) -> Int { return c.value + n }\n}\n",
        Some("C"),
        "same",
    )
    .unwrap();
    assert!(swift.4.contains("return self.value + n"));
    assert_eq!(swift.5, 1);
}

#[tokio::test]
async fn complete_receivers_and_literal_call_text_are_preserved() {
    let before = "class C {\n    static same(c, other) { return c === other; }\n}\nconst text = \"C.same(c, other)\";\n// C.same(c, other)\nC.same(flag ? left : right, other);\nC.same(values[index], other);\nC.same(make(), other);\n";
    let (after, result) = convert(before, "js", false).await;
    assert!(after.contains("\"C.same(c, other)\""));
    assert!(after.contains("// C.same(c, other)"));
    assert!(after.contains("(flag ? left : right).same(other)"));
    assert!(after.contains("values[index].same(other)"));
    assert!(after.contains("make().same(other)"));
    assert_eq!(result.rewritten_calls, 3);
}

#[tokio::test]
async fn unsupported_binding_and_receiver_semantics_refuse_even_with_force_before_writes() {
    for source in [
        "class C {\n static same(c, other) { return this.value + c.value; }\n}\n",
        "class C {\n static same(c, other) { return function() { return this.value + c.value; }; }\n}\n",
        "class C {\n static same(c, other) { return arguments[0]; }\n}\n",
        "class C {\n static same(c = new C(), other) { return c; }\n}\n",
        "class C {\n static same({value}, other) { return value; }\n}\n",
        "class C {\n static same(c, other = c) { return other; }\n}\n",
    ] {
        refuses(source, "js").await;
    }
    for source in [
        "class C:\n    @staticmethod\n    def same(c, other): return self, c\n",
        "class C:\n    @staticmethod\n    def same(c, self): return c\n",
        "class C:\n    @staticmethod\n    def same(c, other): return locals()\n",
    ] {
        refuses(source, "py").await;
    }
    for source in [
        "class C {\n static func same(c: inout C, other: C) -> C { return c }\n}\n",
        "class C {\n static func same(c: C, other: C) -> C { return self }\n}\n",
    ] {
        refuses(source, "swift").await;
    }
    for source in [
        "struct C {\n static int same(C&& c, C other) { return c.value; }\n};\n",
        "struct C {\n static int same(C& c, C other) { return this->value; }\n};\n",
    ] {
        refuses(source, "cpp").await;
    }
}

#[test]
fn unchanged_go_receiver_binding_control() {
    let before = "package main\n\ntype C struct { value int }\n\nfunc same(c *C, other *C) bool {\n    return c == other\n}\n\nfunc main() {\n    c := &C{}\n    _ = same(c, c)\n}\n";
    let result = to_method_go(before, Some("C"), "same").unwrap();
    assert!(
        result.4.contains("func (c *C) same(other *C) bool"),
        "{}",
        result.4
    );
    assert!(result.4.contains("return c == other"));
    assert!(result.4.contains("c.same(c)"));
}

#[tokio::test]
async fn unsupported_cpp_raw_literal_in_another_file_refuses_before_any_write() {
    use prod_code_mcp::to_method::convert_to_method_polyglot;
    use prod_code_testkit::Workspace;

    let header = "struct C {\n static int same(C& c, int n) { return c.value + n; }\n};\n";
    let caller = "const char* text = R\"tag(\" C::same(c, 1) \" )tag\";\n";
    let ws = Workspace::new(&[("case.h", header), ("caller.cpp", caller)]);
    let path = ws.root().join("case.h");
    let error = convert_to_method_polyglot(
        "127.0.0.1:1".parse().unwrap(),
        &ws.root(),
        &path,
        Some("C"),
        "same",
        true,
        true,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("unsupported C++ raw string"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), header);
    assert_eq!(
        std::fs::read_to_string(ws.root().join("caller.cpp")).unwrap(),
        caller
    );
}
