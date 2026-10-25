/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `inline_parameter_polyglot` on the shapes the happy path does not reach: references that are
//! not calls, calls in the declaring file, and the other languages' declarations.

#[allow(dead_code)]
#[path = "coverage_w2_mcp_runtime_inline_parameter/support.rs"]
mod support;

use prod_code_testkit::{Workspace, answers};
use support::*;

#[tokio::test]
async fn references_that_are_not_calls_are_reported_and_block_the_write() {
    let client = "import { clamp } from \"./math\";\nexport const f = clamp;\nexport const v = clamp(1, 5);\n";
    let ws = Workspace::new(&[
        ("math.ts", MATH_TS),
        ("client.ts", client),
        ("notes.md", "clamp(1, 5)\n"),
    ]);
    let client_file = ws.path("client.ts");
    let gw = gateway(
        vec![
            reference_at(&client_file, client, "clamp", 1),
            reference_at(&client_file, client, "clamp", 2),
            location(&ws.path("notes.md"), 0, 0),
            location(&ws.path("math.ts"), 0, 0),
        ],
        answers::no_diagnostics(),
    )
    .await;
    let mut request = Request::new("math.ts", "clamp", "max");
    let res = request.run(gw.addr(), &ws).await.unwrap();
    assert_eq!(res.value, "5");
    assert_eq!(res.rewritten_calls, 1);
    let mut unmatched = res.unmatched.clone();
    unmatched.sort();
    assert_eq!(
        unmatched,
        [
            "client.ts:2:18 (the function used as a value: it would change type)",
            "math.ts:1:1: analyzer reference could not be rewritten safely",
            "notes.md:1:1: analyzer reference could not be rewritten safely",
        ]
    );
    let text = res.render(10_000);
    assert!(text.contains("not a call with this argument (3)"), "{text}");

    request.apply = true;
    let err = request.run(gw.addr(), &ws).await.unwrap_err();
    assert!(
        err.to_string().starts_with(
            "3 reference(s) to `clamp` are not a call passing `max`; nothing was written:\n  "
        ),
        "{err}"
    );
    assert_eq!(ws.read("client.ts"), client);
}

#[tokio::test]
async fn an_export_that_only_names_the_function_is_not_a_call_but_an_exported_call_is() {
    let client = "import { clamp } from \"./math\";\nexport { clamp };\nexport default clamp;\nexport const v = clamp(1, 5);\n";
    let ws = Workspace::new(&[("math.ts", MATH_TS), ("client.ts", client)]);
    let client_file = ws.path("client.ts");
    let gw = gateway(
        (1..4)
            .map(|n| reference_at(&client_file, client, "clamp", n))
            .collect(),
        answers::no_diagnostics(),
    )
    .await;
    let mut request = Request::new("math.ts", "clamp", "max");
    request.apply = true;
    let res = request.run(gw.addr(), &ws).await.unwrap();
    assert_eq!((res.value.as_str(), res.rewritten_calls), ("5", 1));
    assert!(res.unmatched.is_empty(), "{:?}", res.unmatched);
    assert_eq!(
        ws.read("client.ts"),
        "import { clamp } from \"./math\";\nexport { clamp };\nexport default clamp;\nexport const v = clamp(1);\n"
    );
}

#[tokio::test]
async fn calls_in_the_declaring_file_are_rewritten_and_a_self_call_is_refused() {
    let source = "export function clamp(x: number, max: number): number {\n    return Math.min(x, max);\n}\nexport const a = clamp(5, 9);\nexport function again(x: number): number {\n    return clamp(x, 9);\n}\n";
    let ws = Workspace::new(&[("math.ts", source)]);
    let file = ws.path("math.ts");
    let gw = gateway(
        vec![
            reference_at(&file, source, "clamp", 1),
            reference_at(&file, source, "clamp", 2),
        ],
        answers::no_diagnostics(),
    )
    .await;
    let mut request = Request::new("math.ts", "clamp", "max");
    request.apply = true;
    let res = request.run(gw.addr(), &ws).await.unwrap();
    assert!(res.applied && res.unmatched.is_empty());
    assert_eq!(
        ws.read("math.ts"),
        "export function clamp(x: number): number {\n    const max: number = 9;\n    return Math.min(x, max);\n}\nexport const a = clamp(5);\nexport function again(x: number): number {\n    return clamp(x);\n}\n"
    );

    let source = "export function fact(n: number, base: number): number {\n    return n <= 1 ? base : n * fact(n - 1, base);\n}\nexport const r = fact(5, 1);\n";
    let ws = Workspace::new(&[("fact.ts", source)]);
    let file = ws.path("fact.ts");
    let gw = gateway(
        vec![
            reference_at(&file, source, "fact", 1),
            reference_at(&file, source, "fact", 2),
        ],
        answers::no_diagnostics(),
    )
    .await;
    let res = Request::new("fact.ts", "fact", "base")
        .run(gw.addr(), &ws)
        .await
        .unwrap();
    assert_eq!(
        res.unmatched,
        ["fact.ts:2:32 (a call inside `fact` itself passes its own `base`)"]
    );
    assert_eq!(res.rewritten_calls, 1);
}

#[tokio::test]
async fn a_file_that_is_not_text_is_left_alone() {
    let ws = ts_workspace();
    std::fs::write(ws.path("blob.ts"), b"\xff\xfe clamp(1, 2)").unwrap();
    let gw = gateway(client_calls(&ws), answers::no_diagnostics()).await;
    let res = Request::new("math.ts", "clamp", "max")
        .run(gw.addr(), &ws)
        .await
        .unwrap();
    assert_eq!(res.rewritten_calls, 2);
    assert!(
        res.rewritten
            .iter()
            .all(|(path, _)| !path.ends_with("blob.ts"))
    );
    assert!(res.unmatched.is_empty(), "{:?}", res.unmatched);
}

#[tokio::test]
async fn go_parameters_that_share_a_type_keep_it_and_python_receivers_stay() {
    let go = "package m\n\nfunc Clamp(x, max int) int {\n\tif x > max {\n\t\treturn max\n\t}\n\treturn x\n}\n";
    let go_client = "package m\n\nfunc Run() int {\n\treturn Clamp(3, 8)\n}\n";
    let ws = Workspace::new(&[("m.go", go), ("run.go", go_client)]);
    let gw = gateway(
        vec![reference_at(&ws.path("run.go"), go_client, "Clamp", 0)],
        answers::no_diagnostics(),
    )
    .await;
    let mut request = Request::new("m.go", "Clamp", "max");
    request.apply = true;
    let res = request.run(gw.addr(), &ws).await.unwrap();
    assert_eq!(res.value, "8");
    assert_eq!(
        ws.read("m.go"),
        "package m\n\nfunc Clamp(x int) int {\n\tconst max = 8\n\tif x > max {\n\t\treturn max\n\t}\n\treturn x\n}\n"
    );
    assert_eq!(
        ws.read("run.go"),
        "package m\n\nfunc Run() int {\n\treturn Clamp(3)\n}\n"
    );

    let py = "class Scaler:\n    def scale(self, x, factor):\n        return x * factor\n";
    let py_client = "s = Scaler()\nr1 = s.scale(3, 2)\nr2 = Scaler.scale(s, 4, factor=2)\n";
    let ws = Workspace::new(&[("scaler.py", py), ("client.py", py_client)]);
    let gw = gateway(
        vec![
            reference_at(&ws.path("client.py"), py_client, "scale", 0),
            reference_at(&ws.path("client.py"), py_client, "scale", 1),
        ],
        answers::no_diagnostics(),
    )
    .await;
    let mut request = Request::new("scaler.py", "scale", "factor");
    request.apply = true;
    let res = request.run(gw.addr(), &ws).await.unwrap();
    assert_eq!((res.value.as_str(), res.rewritten_calls), ("2", 2));
    assert_eq!(
        ws.read("scaler.py"),
        "class Scaler:\n    def scale(self, x):\n        factor = 2\n        return x * factor\n"
    );
    assert_eq!(
        ws.read("client.py"),
        "s = Scaler()\nr1 = s.scale(3)\nr2 = Scaler.scale(s, 4)\n"
    );
}

#[tokio::test]
async fn swift_labels_and_cpp_prototypes_are_followed() {
    let swift = "func clamp(x: Int, limit max: Int) -> Int {\n    return min(x, max)\n}\n";
    let swift_client = "let a = clamp(x: 1, limit: 5)\nlet b = clamp(x: 2, max: 5)\n";
    let ws = Workspace::new(&[("m.swift", swift), ("c.swift", swift_client)]);
    let gw = gateway(
        vec![
            reference_at(&ws.path("c.swift"), swift_client, "clamp", 0),
            reference_at(&ws.path("c.swift"), swift_client, "clamp", 1),
        ],
        answers::no_diagnostics(),
    )
    .await;
    let mut request = Request::new("m.swift", "clamp", "max");
    request.apply = true;
    let res = request.run(gw.addr(), &ws).await.unwrap();
    assert_eq!(res.value, "5");
    assert_eq!(
        ws.read("m.swift"),
        "func clamp(x: Int) -> Int {\n    let max: Int = 5\n    return min(x, max)\n}\n"
    );
    assert_eq!(
        ws.read("c.swift"),
        "let a = clamp(x: 1)\nlet b = clamp(x: 2)\n"
    );

    // A prototype after the definition loses the parameter with it.
    let cpp = "int clamp(int x, int max) {\n    return x > max ? max : x;\n}\nint clamp(int x, int max);\nint run() { return clamp(1, 7); }\n";
    let ws = Workspace::new(&[("m.cpp", cpp)]);
    let gw = gateway(
        vec![reference_at(&ws.path("m.cpp"), cpp, "clamp", 2)],
        answers::no_diagnostics(),
    )
    .await;
    let mut request = Request::new("m.cpp", "clamp", "max");
    request.apply = true;
    request.run(gw.addr(), &ws).await.unwrap();
    assert_eq!(
        ws.read("m.cpp"),
        "int clamp(int x) {\n    const int max = 7;\n    return x > max ? max : x;\n}\nint clamp(int x);\nint run() { return clamp(1); }\n"
    );
}
