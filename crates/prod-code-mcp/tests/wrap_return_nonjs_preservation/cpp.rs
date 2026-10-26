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

#[tokio::test]
async fn shared_cpp_include_does_not_own_prototype() {
    let header = "#include <optional>\n#include <string>\nnamespace other {\nint calculate();\n}\n";
    let files = [
        (
            "compute.cpp",
            "#include <optional>\n#include <string>\nint calculate() { return 1; }\n",
        ),
        ("unrelated.hpp", header),
        (
            "other.cpp",
            "#include \"unrelated.hpp\"\nint other::calculate() { return 9; }\n",
        ),
        (
            "main.cpp",
            "#include \"unrelated.hpp\"\nint main() { return other::calculate() == 9 ? 0 : 1; }\n",
        ),
    ];
    for precise in [false, true] {
        let ws = Workspace::new(&files);
        assert!(
            run(
                &ws,
                "c++",
                &[
                    "-std=c++17",
                    "compute.cpp",
                    "other.cpp",
                    "main.cpp",
                    "-o",
                    "before"
                ]
            )
            .status
            .success()
        );
        assert!(run(&ws, "./before", &[]).status.success());
        let refs = if precise {
            vec![("compute.cpp", files[0].1.find("calculate").unwrap())]
        } else {
            vec![]
        };
        let result = wrap(&ws, "compute.cpp", "calculate", &refs, true, false).await;
        eprintln!("cpp precise={precise}: {result:?}");
        let after = std::fs::read_to_string(ws.root().join("unrelated.hpp")).unwrap();
        eprintln!("unrelated.hpp after={after:?}");
        let compiled = run(
            &ws,
            "c++",
            &[
                "-std=c++17",
                "compute.cpp",
                "other.cpp",
                "main.cpp",
                "-o",
                "after",
            ],
        );
        assert_eq!(after, header);
        assert!(compiled.status.success());
        assert!(run(&ws, "./after", &[]).status.success());
    }
}

#[tokio::test]
async fn genuine_cpp_header_preserves_trivia_and_executes() {
    let header = "#pragma once\n#include <optional>\n// keep this declaration\nint /* int: preserve this */\n    calculate();\nint footprint();\n";
    let client = "# include \"api.hpp\"\nstd::optional<int> run() { return calculate(); }\nint main() { return run().value() == 1 ? 0 : 1; }\n";
    let files = [
        (
            "compute.cpp",
            "#include \"api.hpp\"\nint calculate() { return 1; }\n",
        ),
        ("api.hpp", header),
        ("client.cpp", client),
    ];
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
                ("api.hpp", header.find("calculate").unwrap()),
                ("client.cpp", client.find("calculate").unwrap()),
            ]
        } else {
            vec![]
        };
        let result = wrap(&ws, "compute.cpp", "calculate", &refs, true, false)
            .await
            .unwrap();
        assert_eq!(result.propagated, 1, "{result:?}");
        assert_eq!(
            std::fs::read_to_string(ws.root().join("api.hpp")).unwrap(),
            header.replacen("int /*", "std::optional<int> /*", 1)
        );
        unchanged(&ws, &files[2..]);
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
    }
}

#[tokio::test]
async fn uncertain_cpp_prototype_refuses_with_force_and_semantic_references() {
    for precise in [false, true] {
        for force in [false, true] {
            let files = [
                (
                    "compute.cpp",
                    "#include <optional>\nint calculate() { return 1; }\n",
                ),
                (
                    "compute.hpp",
                    "int /* same stem is not ownership */ calculate();\n",
                ),
            ];
            let ws = Workspace::new(&files);
            let refs = if precise {
                vec![("compute.hpp", files[1].1.find("calculate").unwrap())]
            } else {
                vec![]
            };
            let result = wrap(&ws, "compute.cpp", "calculate", &refs, true, force).await;
            assert!(result.is_err(), "{result:?}");
            unchanged(&ws, &files);
        }
    }
}

#[tokio::test]
async fn owned_cpp_prototype_cannot_bypass_semantic_membership() {
    let files = [
        (
            "compute.cpp",
            "#include \"api.hpp\"\nint calculate() { return 1; }\n",
        ),
        ("api.hpp", "#include <optional>\nint calculate();\n"),
    ];
    let ws = Workspace::new(&files);
    let refs = [("compute.cpp", files[0].1.find("calculate").unwrap())];
    let result = wrap(&ws, "compute.cpp", "calculate", &refs, true, true).await;
    assert!(result.is_err(), "{result:?}");
    unchanged(&ws, &files);
}
