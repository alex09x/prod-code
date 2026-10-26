/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_testkit::Workspace;

pub(super) async fn cpp_pair(
    header: &str,
    definition: &str,
    selected: &str,
    call: &str,
) -> Vec<String> {
    let compute = format!("#include \"api.hpp\"\n{definition}\n");
    let client = format!(
        "#include \"api.hpp\"\nstd::optional<int> run() {{ return {call}; }}\nint main() {{ return run().value() == 1 ? 0 : 1; }}\n"
    );
    let files = [
        ("compute.cpp", compute.as_str()),
        ("api.hpp", header),
        (
            "types.hpp",
            "#pragma once\nusing longint = double;\ntemplate<int N> struct Tag {};\n",
        ),
        ("client.cpp", client.as_str()),
    ];
    let mut failures = Vec::new();
    for precise in [false, true] {
        let ws = Workspace::new(&files);
        assert!(
            run(
                &ws,
                "c++",
                &["-std=c++17", "compute.cpp", "client.cpp", "-o", "before"]
            )
            .status
            .success()
        );
        assert!(run(&ws, "./before", &[]).status.success());
        let refs = if precise {
            vec![
                (
                    "api.hpp",
                    header.find(selected).unwrap() + selected.find("calculate").unwrap(),
                ),
                ("client.cpp", client.find("calculate").unwrap()),
            ]
        } else {
            vec![]
        };
        let preview = wrap(&ws, "compute.cpp", "calculate", &refs, false, false)
            .await
            .unwrap();
        let applied = wrap(&ws, "compute.cpp", "calculate", &refs, true, false).await;
        if let Ok(result) = &applied {
            let expected = header.replacen(
                selected,
                &selected.replacen("int calculate", "std::optional<int> calculate", 1),
                1,
            );
            let actual = std::fs::read_to_string(ws.root().join("api.hpp")).unwrap();
            if actual != expected {
                failures.push(format!(
                    "precise={precise}: unexpected header {actual:?}; expected {expected:?}"
                ));
                continue;
            }
            assert!(result.applied);
            assert!(
                run(
                    &ws,
                    "c++",
                    &["-std=c++17", "compute.cpp", "client.cpp", "-o", "after"]
                )
                .status
                .success()
            );
            assert!(run(&ws, "./after", &[]).status.success());
        } else {
            unchanged(&ws, &files);
        }
        if applied.is_err() || !preview.unmatched.is_empty() {
            failures.push(format!(
                "precise={precise}: {applied:?}; preview={preview:?}"
            ));
        }
    }
    failures
}

#[tokio::test]
async fn recovered_cpp_parameter_names() {
    let mut failures = Vec::new();
    for parameter in ["int value", "int"] {
        let selected = format!("int calculate({parameter});");
        let header = format!("#pragma once\n#include <optional>\n{selected}\n");
        failures.extend(
            cpp_pair(
                &header,
                "int calculate(int x) { return x; }",
                &selected,
                "calculate(1)",
            )
            .await,
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn recovered_cpp_include_guard() {
    let failures = cpp_pair("#ifndef API_HPP\n#define API_HPP\n#include <optional>\nint calculate();\n#if 0\nint ignored();\n#endif\n#endif\n",
        "int calculate() { return 1; }", "int calculate();", "calculate()").await;
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn recovered_cpp_token_and_literal_identity() {
    let mut failures = Vec::new();
    for (selected, other, definition, call) in [
        (
            "int calculate(long int x);",
            "int calculate(longint x);",
            "int calculate(long int x) { return static_cast<int>(x); }",
            "calculate(1L)",
        ),
        (
            "int calculate(Tag<sizeof(\"a\")> x);",
            "int calculate(Tag<sizeof(\"long\")> x);",
            "int calculate(Tag<sizeof(\"a\")> x) { return 1; }",
            "calculate(Tag<sizeof(\"a\")>{})",
        ),
    ] {
        let header = format!(
            "#pragma once\n#include <optional>\n#include \"types.hpp\"\n{selected}\n{other}\n"
        );
        failures.extend(cpp_pair(&header, definition, selected, call).await);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn recovered_cpp_unrelated_overload() {
    let failures = cpp_pair(
        "#pragma once\n#include <optional>\nint calculate(int x);\nint calculate(double x);\n",
        "int calculate(int x) { return x; }",
        "int calculate(int x);",
        "calculate(1)",
    )
    .await;
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn recovered_python_comment_backslash() {
    let client = "from db import find_user # \\\nprint(find_user() + 1)\n";
    let files = [
        ("db.py", "def find_user() -> int:\n    return 1\n"),
        ("client.py", client),
    ];
    let mut failures = Vec::new();
    for precise in [false, true] {
        let ws = Workspace::new(&files);
        assert!(run(&ws, "python3", &["client.py"]).status.success());
        let refs = if precise {
            vec![("client.py", client.rfind("find_user").unwrap())]
        } else {
            vec![]
        };
        let preview = wrap(&ws, "db.py", "find_user", &refs, false, false)
            .await
            .unwrap();
        if preview.blocked.len() != 1 {
            failures.push(format!("precise={precise}: {preview:?}"));
        }
        let result = wrap(&ws, "db.py", "find_user", &refs, true, false).await;
        if result.is_ok() {
            failures.push(format!("unsafe apply precise={precise}: {result:?}"));
        }
        if result.is_err() {
            unchanged(&ws, &files);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn recovered_swift_dependent_imports() {
    let mut failures = Vec::new();
    for import in [
        "import Library;",
        "import func Library.calculate",
        "import /* keep */ func Library.calculate;",
    ] {
        let client = format!("{import}\nfunc run() -> Int {{ return calculate() }}\n");
        let files = [
            (
                "Package.swift",
                "// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(name: \"Example\", targets: [.target(name: \"Library\"), .target(name: \"App\", dependencies: [\"Library\"])])\n",
            ),
            (
                "Sources/Library/helper.swift",
                "public func calculate() -> Int { return 1 }\n",
            ),
            ("Sources/App/client.swift", client.as_str()),
        ];
        for precise in [false, true] {
            let ws = Workspace::new(&files);
            let refs = if precise {
                vec![(files[2].0, client.find("calculate()").unwrap())]
            } else {
                vec![]
            };
            let preview = wrap(&ws, files[1].0, "calculate", &refs, false, false)
                .await
                .unwrap();
            if preview.blocked.is_empty() && preview.unmatched.is_empty() {
                failures.push(format!("{import} precise={precise}: {preview:?}"));
            }
            let result = wrap(&ws, files[1].0, "calculate", &refs, true, false).await;
            if result.is_ok() {
                failures.push(format!("unsafe apply {import} precise={precise}"));
            }
            if result.is_err() {
                unchanged(&ws, &files);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
