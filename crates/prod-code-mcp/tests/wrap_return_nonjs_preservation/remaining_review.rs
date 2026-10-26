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
async fn remaining_review_attribute_unnamed_types() {
    let mut failures = Vec::new();
    for definition in [
        "int calculate([[maybe_unused]] A) { return 1; }",
        "int calculate([[maybe_unused]] A value) { return 1; }",
    ] {
        failures.extend(cpp_pair(
            "#ifndef API_HPP\n#define API_HPP\n#include <optional>\nstruct A {};\nstruct B {};\nint calculate([[maybe_unused]] A);\nint calculate([[maybe_unused]] B);\n#endif\n",
            definition,
            "int calculate([[maybe_unused]] A);",
            "calculate(A{})",
        ).await);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn remaining_review_macro_shadowed_alias() {
    let mut failures = Vec::new();
    // Macros also escape namespace include contexts; they are not C++ declarations.
    for (header, compute) in [
        (
            "#pragma once\n#include <optional>\nusing Number = int;\n#define Number double\nint calculate(Number);\n#undef Number\n",
            "#include \"api.hpp\"\nint calculate(double x) { return static_cast<int>(x); }\nstd::optional<int> run() { return calculate(1.0); }\nint main() { return run().value() == 1 ? 0 : 1; }\n",
        ),
        (
            "#pragma once\nint calculate(Number);\n",
            "#include <optional>\nusing Number = int;\nnamespace hidden {\n#include \"macros.hpp\"\n}\n#include \"api.hpp\"\n#undef Number\nint calculate(double x) { return static_cast<int>(x); }\nstd::optional<int> run() { return calculate(1.0); }\nint main() { return run().value() == 1 ? 0 : 1; }\n",
        ),
    ] {
        let files = [
            ("api.hpp", header),
            ("compute.cpp", compute),
            ("macros.hpp", "#pragma once\n#define Number double\n"),
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
                assert!(
                    run(&ws, "c++", &["-std=c++17", "compute.cpp", "-o", "after"])
                        .status
                        .success()
                );
                assert!(run(&ws, "./after", &[]).status.success());
            } else if applied.is_ok()
                || !preview.unmatched.iter().any(|message| {
                    message.starts_with("api.hpp:") && message.contains("declaration ownership")
                })
            {
                failures.push(format!("{header:?}: {applied:?}; {preview:?}"));
            } else {
                unchanged(&ws, &files);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn confirmed_review_cpp_unnamed_union_overloads() {
    let mut failures = Vec::new();
    for definition in [
        "int calculate(union A) { return 1; }",
        "int calculate(union A value) { return value.x; }",
        "int calculate(A value) { return value.x; }",
    ] {
        failures.extend(
            cpp_pair(
                "#ifndef API_HPP\n#define API_HPP\n#include <optional>\nunion A { int x; };\nunion B { int x; };\nint calculate(union A);\nint calculate(union B);\n#endif\n",
                definition,
                "int calculate(union A);",
                "calculate(A{1})",
            )
            .await,
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn confirmed_review_cpp_suppressed_guard_type_facts() {
    let mut failures = Vec::new();
    let aliases = "#ifndef ALIASES_HPP\n#define ALIASES_HPP\nusing Number = double;\n#endif\n";
    let header = "#pragma once\n#include \"aliases.hpp\"\nnamespace types { using Number = int; }\nusing types::Number;\nint calculate(Number value);\n";
    for (compute, force) in [
        (
            "#include <optional>\n#define ALIASES_HPP\n#include \"aliases.hpp\"\n#include \"api.hpp\"\nint calculate(int x) { return x; }\nstd::optional<int> run() { return calculate(1); }\nint main() { return run().value() == 1 ? 0 : 1; }\n",
            false,
        ),
        (
            "#include <optional>\n#include \"macros.hpp\"\n#include \"api.hpp\"\nint calculate(int x) { return x; }\nstd::optional<int> run() { return calculate(1); }\nint main() { return run().value() == 1 ? 0 : 1; }\n",
            true,
        ),
    ] {
        let files = [
            ("aliases.hpp", aliases),
            ("api.hpp", header),
            ("compute.cpp", compute),
            ("macros.hpp", "#pragma once\n#define ALIASES_HPP\n"),
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
            let applied = wrap(&ws, "compute.cpp", "calculate", &refs, true, force).await;
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
            } else if applied.is_ok()
                || !preview.unmatched.iter().any(|message| {
                    message.starts_with("api.hpp:") && message.contains("declaration ownership")
                })
            {
                let after = run(&ws, "c++", &["-std=c++17", "compute.cpp", "-o", "after"]);
                failures.push(format!(
                    "force={force}: {applied:?}; {preview:?}; compile_exit={:?}",
                    after.status.code()
                ));
            } else {
                unchanged(&ws, &files);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn confirmed_review_swift_combining_mark_unrelated_import() {
    let mut failures = Vec::new();
    let manifest = "// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(name: \"Example\", targets: [.target(name: \"Library\"), .target(name: \"Cafe\\u{301}\"), .target(name: \"Other\", dependencies: [\"Cafe\\u{301}\"])])\n";
    let helper = "func calculate() -> Int { return 1 }\n";
    let caller = "func run() -> Int? {\n    return calculate()\n}\n";
    for import in [
        "import Cafe\u{301}",
        "import Cafe\u{301};",
        "import func Cafe\u{301}.calculate",
    ] {
        let other = format!(
            "{import}\nfunc calculate() -> String {{ return \"other\" }}\nfunc run() -> String {{ return calculate() }}\n"
        );
        let files = [
            ("Package.swift", manifest),
            ("Sources/Library/Utils/helper.swift", helper),
            ("Sources/Library/client.swift", caller),
            ("Sources/Library/main.swift", "print(run() ?? 0)\n"),
            ("Sources/Other/client.swift", other.as_str()),
        ];
        for precise in [false, true] {
            let ws = Workspace::new(&files);
            let refs = if precise {
                vec![(files[2].0, caller.find("calculate").unwrap())]
            } else {
                vec![]
            };
            let preview = wrap(&ws, files[1].0, "calculate", &refs, false, false)
                .await
                .unwrap();
            let applied = wrap(&ws, files[1].0, "calculate", &refs, true, false).await;
            let actual = std::fs::read_to_string(ws.root().join(files[1].0)).unwrap();
            if !applied
                .as_ref()
                .is_ok_and(|r| r.applied && r.propagated == 1)
                || !preview.unmatched.is_empty()
                || !preview.blocked.is_empty()
                || actual != "func calculate() -> Int? { return 1 }\n"
            {
                if applied.is_err() {
                    unchanged(&ws, &files);
                }
                failures.push(format!(
                    "{import} precise={precise}: {applied:?}; preview={preview:?}"
                ));
            } else {
                unchanged(&ws, &files[2..]);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
