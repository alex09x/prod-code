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
async fn final_review_void_alias_arity() {
    let mut failures = Vec::new();
    for alias in ["using Nothing = void;", "typedef void Nothing;"] {
        let header = format!(
            "#ifndef API_HPP\n#define API_HPP\n#include <optional>\n{alias}\nint calculate(Nothing);\nint calculate(double);\n#endif\n"
        );
        failures.extend(
            cpp_pair(
                &header,
                "int calculate() { return 1; }",
                "int calculate(Nothing);",
                "calculate()",
            )
            .await,
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn final_review_macro_arity_requires_identity() {
    let header = "#pragma once\n#include <optional>\n#define PARAMETERS int x, int y\nint calculate(PARAMETERS);\n";
    let compute = "#include \"api.hpp\"\nint calculate(int x, int y) { return x + y; }\nstd::optional<int> run() { return calculate(0, 1); }\nint main() { return run().value() == 1 ? 0 : 1; }\n";
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
        let result = wrap(&ws, "compute.cpp", "calculate", &refs, true, true).await;
        if precise {
            assert!(result.unwrap().applied);
            assert!(preview.unmatched.is_empty());
            assert_eq!(
                std::fs::read_to_string(ws.root().join("api.hpp")).unwrap(),
                header.replace("int calculate", "std::optional<int> calculate")
            );
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
            assert!(result.is_err(), "{result:?}");
            unchanged(&ws, &files);
        }
    }
}

#[tokio::test]
async fn final_review_unnamed_complex_overloads() {
    let mut failures = Vec::new();
    for specifier in ["_Complex", "__complex__"] {
        let header = format!(
            "#pragma once\n#include <optional>\nint calculate(double x);\nint calculate(double {specifier});\n"
        );
        failures.extend(
            cpp_pair(
                &header,
                "int calculate(double x) { return static_cast<int>(x); }",
                "int calculate(double x);",
                "calculate(1.0)",
            )
            .await,
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn final_review_mixed_include_context_alias() {
    let mut failures = Vec::new();
    for aliases in [
        "#pragma once\nusing Number = double;\n",
        "#ifndef ALIASES_HPP\n#define ALIASES_HPP\nusing Number = double;\n#endif\n",
    ] {
        let header = "#pragma once\n#include \"aliases.hpp\"\nnamespace types { using Number = int; }\nusing types::Number;\nint calculate(Number value);\n";
        let compute = "#include <optional>\nnamespace hidden {\n#include \"aliases.hpp\"\n}\n#include \"api.hpp\"\nint calculate(int x) { return x; }\nstd::optional<int> run() { return calculate(1); }\nint main() { return run().value() == 1 ? 0 : 1; }\n";
        let files = [
            ("aliases.hpp", aliases),
            ("api.hpp", header),
            ("compute.cpp", compute),
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
            if precise {
                assert!(applied.unwrap().applied);
                assert!(preview.unmatched.is_empty(), "{preview:?}");
                assert_eq!(
                    std::fs::read_to_string(ws.root().join("api.hpp")).unwrap(),
                    header.replace("int calculate", "std::optional<int> calculate")
                );
                assert_eq!(
                    std::fs::read_to_string(ws.root().join("aliases.hpp")).unwrap(),
                    aliases
                );
                assert!(
                    run(&ws, "c++", &["-std=c++17", "compute.cpp", "-o", "after"])
                        .status
                        .success()
                );
                assert!(run(&ws, "./after", &[]).status.success());
            } else {
                if applied.is_ok()
                    || !preview.unmatched.iter().any(|message| {
                        message.starts_with("api.hpp:") && message.contains("declaration ownership")
                    })
                {
                    failures.push(format!("{aliases:?}: {applied:?}; {preview:?}"));
                }
                // Record unsafe public-apply bytes before asserting preservation.
                if applied.is_err() {
                    unchanged(&ws, &files);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
