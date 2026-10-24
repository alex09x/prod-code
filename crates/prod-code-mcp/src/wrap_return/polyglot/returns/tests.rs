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
fn coverage_return_expression_matrix() {
    use Language::*;
    use Wrapper::*;
    for (lang, body, wrapper, expected) in [
        (Go, "return\n", Result, "return nil\n"),
        (Go, "return 1, true\n", Result, "return 1, true, nil\n"),
        (Swift, "return\n", Result, "return .success(())\n"),
        (Swift, "return 7\n", Result, "return .success(7)\n"),
        (Python, "    return\n", Result, "    return Ok(None)\n"),
        (Python, "    return 'α'\r\n", Result, "    return Ok('α')\n"),
        (
            TypeScript,
            "return;\n",
            Result,
            "return { ok: true, value: undefined };\n",
        ),
        (
            JavaScript,
            "return 7;\n",
            Result,
            "return { ok: true, value: 7 };\n",
        ),
        (
            Go,
            "return nil\nreturn &value\nreturn value\n",
            Pointer,
            "return nil\nreturn &value\nreturn &value\n",
        ),
        (Go, "return\n", Option, "return\n"),
        (Cpp, "return 7;\n", Result, "return 7;\n"),
        (
            Python,
            "    return x\n",
            Custom("pkg.Box[T]".into()),
            "    return Box(x)\n",
        ),
        (
            TypeScript,
            "const s = 'return x';\nreturn x;\n",
            Custom("Box<T>".into()),
            "const s = 'return x';\nreturn new Box(x);\n",
        ),
    ] {
        assert_eq!(
            rewrite_body_returns(body, lang, &wrapper, None, "int"),
            expected,
            "{body:?}"
        );
    }
    let body = "    def nested():\n        return 1\n\n    # note\n    async def inner():\n        return 2\n    return 3\n";
    assert_eq!(
        python_owned_return_offsets(body),
        vec![body.rfind("return").unwrap()]
    );
    assert_eq!(
        rewrite_body_returns(body, Python, &Result, None, "int"),
        "    def nested():\n        return 1\n\n    # note\n    async def inner():\n        return 2\n    return Ok(3)\n"
    );
}
