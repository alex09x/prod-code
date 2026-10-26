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
use prod_code_mcp::to_method::{to_method_cpp, to_method_swift, to_method_ts};

#[tokio::test]
async fn review4_typescript_conditional_return_type_refuses_atomically() {
    let before = "class C {\n    value = 7;\n    static same(c:C): C extends C ? {value:number} : never { return {value:c.value}; }\n}\nconsole.log(C.same(new C()).value);\n";
    eprintln!("ORIGINAL Node output: {}", run("node", "ts", before));
    if to_method_ts(before, Some("C"), "same").is_ok() {
        let (after, _) = convert(before, "ts", true).await;
        eprintln!("BEFORE:\n{before}AFTER:\n{after}");
        assert_eq!(run("node", "ts", &after), "7\n");
    }
    refuses(before, "ts").await;
    eprintln!("Conditional signature refused before writes, force=false and force=true");
}

#[tokio::test]
async fn review4_python_dedented_literal_content_preserves_exact_docstring() {
    for body in [
        "        \"\"\"first\ncolumn zero content\n        \"\"\"",
        "        r'''first\ncolumn zero content\n        '''\n        return c.value",
        "        '''first\nclass Other:\ndef same(c):\n        '''; return c.value",
    ] {
        let before = format!(
            "class C:\n    value = 7\n    @staticmethod\n    def same(c):\n{body}\nprint(C.same(C()))\nprint(repr(C.same.__doc__))\n"
        );
        let expected = run("python3", "py", &before);
        eprintln!("ORIGINAL Python output: {expected}");
        let (after, _) = convert(&before, "py", true).await;
        eprintln!("BEFORE:\n{before}AFTER:\n{after}");
        let quote = if body.contains("\"\"\"") {
            "\"\"\""
        } else {
            "'''"
        };
        let doc_end = body.rfind(quote).unwrap() + quote.len();
        assert!(
            after.contains(&body[..doc_end]),
            "docstring bytes changed: {after}"
        );
        let actual = run("python3", "py", &after);
        eprintln!("CONVERTED Python output: {actual}");
        assert_eq!(actual, expected);
    }
    refuses(
        "class C:\n    @staticmethod\n    def same(c):\n        \"\"\"unterminated\ncolumn zero content\n",
        "py",
    )
    .await;
}

#[tokio::test]
async fn review4_swift_nested_return_does_not_prove_outer_return() {
    let before = "class C {\n    var value = 7\n    static func same(c: C) -> Int {\n        ({ () -> Int in\n            return c.value\n        })()\n    }\n}\nprint(C.same(c: C()))\n";
    if available("swiftc") {
        eprintln!(
            "ORIGINAL Swift output: {}",
            compiled("swiftc", "swift", before)
        );
    } else {
        eprintln!(
            "Swift compiler unavailable; public API source controls only; no Swift runtime proof"
        );
    }
    if to_method_swift(before, Some("C"), "same").is_ok() {
        let (after, _) = convert(before, "swift", true).await;
        eprintln!("BEFORE:\n{before}AFTER:\n{after}");
        assert!(after.contains("return ({ () -> Int in"), "{after}");
        if available("swiftc") {
            assert_eq!(compiled("swiftc", "swift", &after), "7\n");
        }
    }
    refuses(before, "swift").await;
    eprintln!("Multiline implicit closure expression refused before writes, both force settings");
    // An explicit method return containing the same nested closure remains valid.
    let explicit = before.replace("        ({", "        return ({");
    let (after, _) = convert(&explicit, "swift", true).await;
    assert!(after.contains("return ({ () -> Int in"), "{after}");
    assert!(after.contains("return c.value"), "{after}");
    eprintln!("EXPLICIT RETURN BEFORE:\n{explicit}AFTER:\n{after}");
    if available("swiftc") {
        assert_eq!(compiled("swiftc", "swift", &after), "7\n");
    }
    let one_line = explicit.replace(
        "return ({ () -> Int in\n            return c.value\n        })()",
        "({ return c.value })()",
    );
    let (after, _) = convert(&one_line, "swift", true).await;
    assert!(after.contains("return ({ return c.value })()"), "{after}");
    if available("swiftc") {
        assert_eq!(compiled("swiftc", "swift", &after), "7\n");
    }
}

#[tokio::test]
async fn review4_cpp_function_try_handlers_refuse_atomically() {
    let before = "#include <iostream>\nstruct C {\n    int value = 7;\n    static int same(C& c) try { throw 1; } catch (...) { return c.value; }\n};\nint main() { C value; std::cout << C::same(value) << '\\n'; }\n";
    eprintln!("ORIGINAL C++ output: {}", compiled("c++", "cpp", before));
    if to_method_cpp(before, Some("C"), "same").is_ok() {
        let (after, _) = convert(before, "cpp", true).await;
        eprintln!("BEFORE:\n{before}AFTER:\n{after}");
        assert_eq!(compiled("c++", "cpp", &after), "7\n");
    }
    refuses(before, "cpp").await;
    eprintln!("Function-try-block refused before writes, force=false and force=true");
    let ordinary = before.replace(
        "try { throw 1; } catch (...) { return c.value; }",
        "{ try { throw 1; } catch (...) { return c.value; } }",
    );
    let (after, _) = convert(&ordinary, "cpp", true).await;
    eprintln!("ORDINARY TRY BEFORE:\n{ordinary}AFTER:\n{after}");
    assert_eq!(compiled("c++", "cpp", &after), "7\n");
}
