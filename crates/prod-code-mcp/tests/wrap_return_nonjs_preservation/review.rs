/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{recovered::cpp_pair, support::*};
use prod_code_testkit::Workspace;

#[tokio::test]
async fn review_unnamed_extended_integer() {
    let failures = cpp_pair(
        "#pragma once\n#include <optional>\nint calculate(unsigned x);\nint calculate(unsigned __int128);\n",
        "int calculate(unsigned x) { return x; }",
        "int calculate(unsigned x);",
        "calculate(1u)",
    ).await;
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn review_alias_prototype() {
    let mut failures = Vec::new();
    for (parameter, alias) in [
        ("Number value", "using Number = int;\n"),
        ("signed int", ""),
        ("const int value", ""),
    ] {
        let selected = format!("int calculate({parameter});");
        let header = format!(
            "#pragma once\n#include <optional>\n{alias}{selected}\nint calculate(double);\n"
        );
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
async fn review_comment_after_name() {
    let failures = cpp_pair(
        "#ifndef API_HPP\n#define API_HPP\n#include <optional>\nint calculate /* keep */ (int value);\n#endif\n",
        "int calculate(int x) { return x; }",
        "int calculate /* keep */ (int value);",
        "calculate(1)",
    ).await;
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn review_unresolved_alias_requires_identity() {
    let header = "#pragma once\n#include <optional>\nusing Number = decltype(1);\nint calculate(Number value);\n";
    let compute = "#include \"api.hpp\"\nint calculate(int x) { return x; }\nstd::optional<int> run() { return calculate(1); }\nint main() { return run().value() == 1 ? 0 : 1; }\n";
    let files = [("api.hpp", header), ("compute.cpp", compute)];
    for precise in [false, true] {
        let ws = Workspace::new(&files);
        assert!(
            run(&ws, "c++", &["-std=c++17", "compute.cpp", "-o", "before"])
                .status
                .success()
        );
        assert!(run(&ws, "./before", &[]).status.success());
        let refs = if precise {
            vec![
                ("api.hpp", header.find("calculate").unwrap()),
                ("compute.cpp", compute.rfind("calculate").unwrap()),
            ]
        } else {
            vec![]
        };
        let preview = wrap(&ws, "compute.cpp", "calculate", &refs, false, false)
            .await
            .unwrap();
        let applied = wrap(&ws, "compute.cpp", "calculate", &refs, true, true).await;
        if precise {
            assert!(applied.unwrap().applied);
            assert!(preview.unmatched.is_empty());
            assert!(
                run(&ws, "c++", &["-std=c++17", "compute.cpp", "-o", "after"])
                    .status
                    .success()
            );
            assert!(run(&ws, "./after", &[]).status.success());
        } else {
            assert!(
                preview
                    .unmatched
                    .iter()
                    .any(|message| message.starts_with("api.hpp:")
                        && message.contains("declaration ownership")),
                "{preview:?}"
            );
            assert!(applied.is_err());
            unchanged(&ws, &files);
        }
    }
}

#[tokio::test]
async fn review_commented_void() {
    let failures = cpp_pair(
        "#ifndef API_HPP\n#define API_HPP\n#include <optional>\nint calculate(/* keep */ void);\n#endif\n",
        "int calculate() { return 1; }",
        "int calculate(/* keep */ void);",
        "calculate()",
    ).await;
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn review_include_alias_context() {
    let header = "#pragma once\nint calculate(Number value);\n";
    let mut failures = Vec::new();
    for (context, other) in [
        (
            "namespace unrelated {\n#include \"other.hpp\"\n}\n",
            "using Number = double;\n",
        ),
        (
            "namespace unrelated {\n#include \"other.hpp\"\n}\n",
            "#include \"detail.hpp\"\n",
        ),
        (
            "#if 0\n#include \"other.hpp\"\n#endif\n",
            "using Number = double;\n",
        ),
    ] {
        let compute = format!(
            "#include <optional>\ntypedef int Number;\n{context}#include \"api.hpp\"\nint calculate(int x) {{ return x; }}\nstd::optional<int> run() {{ return calculate(1); }}\nint main() {{ return run().value() == 1 ? 0 : 1; }}\n"
        );
        let files = [
            ("api.hpp", header),
            ("other.hpp", other),
            ("detail.hpp", "using Number = double;\n"),
            ("compute.cpp", compute.as_str()),
        ];
        for precise in [false, true] {
            let ws = Workspace::new(&files);
            assert!(
                run(&ws, "c++", &["-std=c++17", "compute.cpp", "-o", "before"])
                    .status
                    .success()
            );
            assert!(run(&ws, "./before", &[]).status.success());
            let refs = if precise {
                vec![
                    ("api.hpp", header.find("calculate").unwrap()),
                    ("compute.cpp", compute.rfind("calculate").unwrap()),
                ]
            } else {
                vec![]
            };
            let preview = wrap(&ws, "compute.cpp", "calculate", &refs, false, false)
                .await
                .unwrap();
            let applied = wrap(&ws, "compute.cpp", "calculate", &refs, true, true).await;
            let expected = header.replace("int calculate", "std::optional<int> calculate");
            let actual = std::fs::read_to_string(ws.root().join("api.hpp")).unwrap();
            if !applied.as_ref().is_ok_and(|result| result.applied)
                || actual != expected
                || !preview.unmatched.is_empty()
            {
                failures.push(format!(
                    "precise={precise}: {applied:?}; preview={preview:?}; header={actual:?}"
                ));
                continue;
            }
            assert_eq!(
                std::fs::read_to_string(ws.root().join("other.hpp")).unwrap(),
                files[1].1
            );
            assert!(
                run(&ws, "c++", &["-std=c++17", "compute.cpp", "-o", "after"])
                    .status
                    .success()
            );
            assert!(run(&ws, "./after", &[]).status.success());
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn review_unicode_swift_imports() {
    for import in [
        "import Café",
        "import Café;",
        "import func Café.calculate",
        "import Cafe\u{301};",
    ] {
        let module = if import.contains("Cafe\u{301}") {
            "Cafe\u{301}"
        } else {
            "Café"
        };
        let selected = format!("Sources/{module}/helper.swift");
        let manifest = format!(
            "// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(name: \"Example\", targets: [.target(name: \"{module}\"), .target(name: \"App\", dependencies: [\"{module}\"])])\n"
        );
        let client = format!("{import}\nfunc run() -> Int {{ return calculate() }}\n");
        let files = [
            ("Package.swift", manifest.as_str()),
            (
                selected.as_str(),
                "public func calculate() -> Int { return 1 }\n",
            ),
            ("Sources/App/client.swift", client.as_str()),
        ];
        for precise in [false, true] {
            let ws = Workspace::new(&files);
            let refs = if precise {
                vec![(files[2].0, client.rfind("calculate()").unwrap())]
            } else {
                vec![]
            };
            let preview = wrap(&ws, files[1].0, "calculate", &refs, false, false)
                .await
                .unwrap();
            let result = wrap(&ws, files[1].0, "calculate", &refs, true, !precise).await;
            assert!(
                !preview.blocked.is_empty() || !preview.unmatched.is_empty(),
                "{import} precise={precise}: {preview:?}"
            );
            assert!(result.is_err(), "{import} precise={precise}: {result:?}");
            unchanged(&ws, &files);
        }
    }
}
