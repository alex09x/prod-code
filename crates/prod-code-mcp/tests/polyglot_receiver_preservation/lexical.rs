/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{available, compiled, convert, refuses, run};
use prod_code_mcp::make_static::Language;
use prod_code_mcp::to_method::convert_to_method_polyglot;
use prod_code_mcp::to_method::polyglot::rewrite_static_calls_in_code;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

#[tokio::test]
async fn review3_cpp_block_comments_do_not_nest() {
    let before = "#include <iostream>\nstruct C {\n    int value;\n    static int same(C& c) { /* a literal /* marker */ return c.value; }\n};\nint main() { C c{7}; /* another /* marker */ std::cout << C::same(c) << '\\n'; }\n";
    eprintln!("BEFORE:\n{before}");
    let expected = available("c++").then(|| compiled("c++", "cpp", before));
    let (after, _) = convert(before, "cpp", true).await;
    eprintln!("AFTER:\n{after}");
    assert!(after.contains("/* a literal /* marker */ return c.value;"));
    assert!(after.contains("C& c = *this;"));
    assert!(after.contains("std::cout << c.same()"));
    if let Some(expected) = expected {
        assert_eq!(expected, "7\n");
        assert_eq!(compiled("c++", "cpp", &after), expected);
        eprintln!("C++ BEFORE/AFTER OUTPUT: {expected:?}");
    }
}

#[tokio::test]
async fn review3_cpp_ordinary_strings_and_commented_raw_markers() {
    let before = "#include <iostream>\nstruct C {\n    static const char* same(C& c) { return \"ERROR\"; }\n};\n// R\"tag( is only comment text, as is C::same(c)\nint main() { C c; std::cout << C::same(c) << '\\n'; }\n";
    eprintln!("BEFORE:\n{before}");
    let expected = available("c++").then(|| compiled("c++", "cpp", before));
    let (after, _) = convert(before, "cpp", true).await;
    eprintln!("AFTER:\n{after}");
    assert!(after.contains("return \"ERROR\";"));
    assert!(after.contains("// R\"tag( is only comment text, as is C::same(c)"));
    if let Some(expected) = expected {
        assert_eq!(expected, "ERROR\n");
        assert_eq!(compiled("c++", "cpp", &after), expected);
        eprintln!("C++ BEFORE/AFTER OUTPUT: {expected:?}");
    }
}

#[test]
fn review3_cpp_standalone_rewriter_handles_ordinary_r_suffix() {
    let before = "const char* error = \"ERROR\"; auto value = C::same(c);\n";
    let (after, calls) = rewrite_static_calls_in_code(before, "same", "C", Language::Cpp);
    eprintln!("BEFORE:\n{before}AFTER:\n{after}");
    assert_eq!(calls, 1);
    assert_eq!(
        after,
        "const char* error = \"ERROR\"; auto value = c.same();\n"
    );
}

#[tokio::test]
async fn review3_python_prefixed_docstrings_keep_their_position() {
    for doc in [
        "r'doc'",
        "u'doc'",
        "R\"doc\"",
        "U\"doc\"",
        "r'''doc'''",
        "u\"\"\"doc\"\"\"",
    ] {
        for layout in [
            format!(" {doc}; return c is other"),
            format!("\n        {doc}\n        return c is other"),
        ] {
            let before = format!(
                "class C:\n    @staticmethod\n    def same(c, other):{layout}\nvalue = C()\nprint(C.same(value, value))\nprint(C.same.__doc__)\n"
            );
            eprintln!("BEFORE:\n{before}");
            let expected = run("python3", "py", &before);
            assert_eq!(expected, "True\ndoc\n");
            let (after, _) = convert(&before, "py", true).await;
            eprintln!("AFTER:\n{after}");
            assert!(
                after.find(doc).unwrap() < after.find("c = self").unwrap(),
                "{after}"
            );
            assert_eq!(run("python3", "py", &after), expected);
            eprintln!("PYTHON BEFORE/AFTER OUTPUT: {expected:?}");
        }
    }
}

#[tokio::test]
async fn review3_cpp_caller_file_does_not_mistake_error_for_raw_string() {
    let header = "struct C {\n    int value;\n    static int same(C& c) { return c.value; }\n};\n";
    let caller = "#include \"model.hpp\"\n#include <iostream>\nint main() { C c{7}; std::cout << \"ERROR\" << ':' << C::same(c) << '\\n'; }\n";
    let ws = Workspace::new(&[("model.hpp", header), ("caller.cpp", caller)]);
    let gateway = ScriptedGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    eprintln!("BEFORE HEADER:\n{header}BEFORE CALLER:\n{caller}");
    let before = format!("{header}{}", caller.replace("#include \"model.hpp\"\n", ""));
    let expected = available("c++").then(|| compiled("c++", "cpp", &before));
    let result = convert_to_method_polyglot(
        gateway.addr(),
        &ws.root(),
        &ws.root().join("model.hpp"),
        Some("C"),
        "same",
        true,
        false,
    )
    .await
    .unwrap();
    assert_eq!(result.rewritten_calls, 1);
    let header = std::fs::read_to_string(ws.root().join("model.hpp")).unwrap();
    let caller = std::fs::read_to_string(ws.root().join("caller.cpp")).unwrap();
    eprintln!("AFTER HEADER:\n{header}AFTER CALLER:\n{caller}");
    assert!(caller.contains("\"ERROR\" << ':' << c.same()"));
    if let Some(expected) = expected {
        assert_eq!(expected, "ERROR:7\n");
        let after = format!("{header}{}", caller.replace("#include \"model.hpp\"\n", ""));
        assert_eq!(compiled("c++", "cpp", &after), expected);
        eprintln!("C++ BEFORE/AFTER OUTPUT: {expected:?}");
    }
}

#[tokio::test]
async fn review3_cpp_actual_raw_tokens_still_refuse_without_writes() {
    for prefix in ["R", "u8R", "uR", "UR", "LR"] {
        let raw = format!("{prefix}\"tag(c.value /* }} \\\" )tag\"");
        let source = format!(
            "struct C {{\n    int value;\n    static int same(C& c) {{ auto text = {raw}; return c.value; }}\n}};\n"
        );
        refuses(&source, "cpp").await;
        let source = format!("auto text = {raw}; auto value = C::same(c);\n");
        assert_eq!(
            rewrite_static_calls_in_code(&source, "same", "C", Language::Cpp),
            (source, 0)
        );
        let header =
            "struct C {\n    int value;\n    static int same(C& c) { return c.value; }\n};\n";
        let caller = format!("auto text = {raw}; auto value = C::same(c);\n");
        let ws = Workspace::new(&[("model.hpp", header), ("caller.cpp", &caller)]);
        for force in [false, true] {
            let error = convert_to_method_polyglot(
                "127.0.0.1:1".parse().unwrap(),
                &ws.root(),
                &ws.root().join("model.hpp"),
                Some("C"),
                "same",
                true,
                force,
            )
            .await
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("unsupported C++ raw string caller")
            );
            assert_eq!(
                std::fs::read_to_string(ws.root().join("model.hpp")).unwrap(),
                header
            );
            assert_eq!(
                std::fs::read_to_string(ws.root().join("caller.cpp")).unwrap(),
                caller
            );
        }
    }
}
