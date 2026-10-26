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

fn native(ws: &Workspace, selected: &str, binary: &str, flags: &[&str]) -> bool {
    static IDENTITY: std::sync::Once = std::sync::Once::new();
    IDENTITY.call_once(|| {
        assert!(run(ws, "uname", &["-sm"]).status.success());
        assert!(run(ws, "c++", &["--version"]).status.success());
    });
    let mut args = vec!["-std=c++17"];
    args.extend_from_slice(flags);
    args.extend([selected, "-o", binary]);
    let compiled = run(ws, "c++", &args).status.success();
    compiled && run(ws, &format!("./{binary}"), &[]).status.success()
}

#[tokio::test]
async fn prototype_review_selected_source() {
    let source = "#include <optional>\nint calculate(int value);\nint calculate(double value);\nnamespace other { int calculate(int value); }\nint calculate(int x) { return x; }\nstd::optional<int> run() { return calculate(1); }\nint main() { return run().value() == 1 ? 0 : 1; }\n";
    let mut failures = Vec::new();
    for membership in ["empty", "precise", "missing"] {
        for force in [false, true] {
            let files = [("compute.cpp", source)];
            let ws = Workspace::new(&files);
            assert!(native(&ws, "compute.cpp", "before", &[]));
            let mut refs = vec![];
            if membership != "empty" {
                refs.push(("compute.cpp", source.rfind("calculate").unwrap()));
            }
            if membership == "precise" {
                refs.push(("compute.cpp", source.find("calculate").unwrap()));
            }
            let preview = wrap(&ws, "compute.cpp", "calculate", &refs, false, force)
                .await
                .unwrap();
            unchanged(&ws, &files);
            let result = wrap(&ws, "compute.cpp", "calculate", &refs, true, force).await;
            if membership == "missing" {
                assert!(result.is_err());
                assert!(
                    preview
                        .unmatched
                        .iter()
                        .any(|s| s.contains("missing from semantic references"))
                );
                unchanged(&ws, &files);
            } else if let Ok(result) = result {
                assert!(result.applied);
                let after = std::fs::read_to_string(ws.root().join("compute.cpp")).unwrap();
                assert!(after.contains("std::optional<int> calculate(int value);"));
                assert!(after.contains("int calculate(double value);"));
                assert!(after.contains("namespace other { int calculate(int value); }"));
                assert!(native(&ws, "compute.cpp", "after", &[]));
            } else {
                failures.push(format!("{membership} force={force}: {result:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn prototype_review_configured_include() {
    let header = "#pragma once\nint calculate(int value);\nint calculate(double value);\nnamespace other { int calculate(int value); }\n";
    let source = "#include <optional>\n#include \"api.hpp\"\nint calculate(int x) { return x; }\nstd::optional<int> run() { return calculate(1); }\nint main() { return run().value() == 1 ? 0 : 1; }\n";
    let mut failures = Vec::new();
    for config in ["flags", "database", "command", "angle"] {
        for precise in [false, true] {
            for force in [false, true] {
                let selected = if config == "angle" {
                    source.replace("\"api.hpp\"", "<api.hpp>")
                } else {
                    source.to_string()
                };
                let files = [
                    ("src/compute.cpp", selected.as_str()),
                    ("include/api.hpp", header),
                    ("loose/api.hpp", "#pragma once\nint unrelated();\n"),
                ];
                let ws = Workspace::new(&files);
                if matches!(config, "flags" | "angle") {
                    std::fs::write(
                        ws.root().join("compile_flags.txt"),
                        "-std=c++17\n-Iinclude\n",
                    )
                    .unwrap();
                } else {
                    let entry = if config == "database" {
                        serde_json::json!({"directory": ws.root(), "file": "src/compute.cpp", "arguments": ["c++", "-I", "include", "src/compute.cpp"]})
                    } else {
                        serde_json::json!({"directory": ws.root(), "file": "src/compute.cpp", "command": "c++ -I'include' src/compute.cpp"})
                    };
                    // An unrelated TU's first search path must never own our header.
                    let decoy = serde_json::json!({"directory": ws.root(), "file": "src/other.cpp", "arguments": ["c++", "-Iloose", "src/other.cpp"]});
                    std::fs::write(
                        ws.root().join("compile_commands.json"),
                        serde_json::json!([decoy, entry]).to_string(),
                    )
                    .unwrap();
                }
                let context = ["compile_flags.txt", "compile_commands.json"]
                    .into_iter()
                    .filter_map(|path| {
                        std::fs::read_to_string(ws.root().join(path))
                            .ok()
                            .map(|content| (path, content))
                    })
                    .collect::<std::collections::BTreeMap<_, _>>();
                eprintln!(
                    "CPP_CONTEXT {}",
                    serde_json::to_string(&context)
                        .unwrap()
                        .replace(&ws.root().display().to_string(), "<fixture>")
                );
                assert!(native(&ws, "src/compute.cpp", "before", &["-Iinclude"]));
                let refs = if precise {
                    vec![
                        ("include/api.hpp", header.find("calculate").unwrap()),
                        ("src/compute.cpp", selected.rfind("calculate").unwrap()),
                    ]
                } else {
                    vec![]
                };
                let preview = wrap(&ws, "src/compute.cpp", "calculate", &refs, false, force)
                    .await
                    .unwrap();
                unchanged(&ws, &files);
                let result = wrap(&ws, "src/compute.cpp", "calculate", &refs, true, force).await;
                if let Ok(result) = result {
                    assert!(result.applied);
                    assert!(preview.unmatched.is_empty(), "{preview:?}");
                    assert_eq!(
                        std::fs::read_to_string(ws.root().join("include/api.hpp")).unwrap(),
                        header.replacen("int calculate(int", "std::optional<int> calculate(int", 1)
                    );
                    unchanged(&ws, &[("loose/api.hpp", files[2].1)]);
                    assert!(native(&ws, "src/compute.cpp", "after", &["-Iinclude"]));
                } else {
                    failures.push(format!(
                        "{config} precise={precise} force={force}: {result:?}"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn prototype_review_attributes() {
    let mut failures = Vec::new();
    for attributes in [
        "[[deprecated(\"legacy\")]]",
        "[[nodiscard]]",
        "[[deprecated(\"legacy ]] ; \\\" message\")]] /* keep */ [[nodiscard]]",
        "[[deprecated(R\"tag(legacy ]] \"quoted\"; // message)tag\")]]",
    ] {
        for precise in [false, true] {
            for force in [false, true] {
                let header = format!(
                    "#pragma once\n#include <optional>\n{attributes} int /* result */ calculate();\n"
                );
                let source = "#include \"api.hpp\"\nint calculate() { return 1; }\nstd::optional<int> run() { return calculate(); }\nint main() { return run().value() == 1 ? 0 : 1; }\n";
                let files = [("api.hpp", header.as_str()), ("compute.cpp", source)];
                let ws = Workspace::new(&files);
                assert!(native(&ws, "compute.cpp", "before", &[]));
                let refs = if precise {
                    vec![
                        ("api.hpp", header.find("calculate").unwrap()),
                        ("compute.cpp", source.rfind("calculate").unwrap()),
                    ]
                } else {
                    vec![]
                };
                let preview = wrap(&ws, "compute.cpp", "calculate", &refs, false, force)
                    .await
                    .unwrap();
                unchanged(&ws, &files);
                let result = wrap(&ws, "compute.cpp", "calculate", &refs, true, force).await;
                if let Ok(result) = result {
                    assert!(result.applied);
                    let actual = std::fs::read_to_string(ws.root().join("api.hpp")).unwrap();
                    let expected =
                        header.replace("int /* result */", "std::optional<int> /* result */");
                    let compiled = native(&ws, "compute.cpp", "after", &[]);
                    if actual != expected || !compiled || !preview.unmatched.is_empty() {
                        failures.push(format!("{attributes} precise={precise} force={force}: {actual:?}; compiled={compiled}; {preview:?}"));
                    }
                } else {
                    failures.push(format!(
                        "{attributes} precise={precise} force={force}: {result:?}"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn prototype_review_literal_return() {
    let header =
        "#pragma once\n#include <optional>\n[[nodiscard]] decltype(sizeof(\"x\")) calculate();\n";
    let source = "#include \"api.hpp\"\ndecltype(sizeof(\"x\")) calculate() { return 1; }\nstd::optional<decltype(sizeof(\"x\"))> run() { return calculate(); }\nint main() { return run().value() == 1 ? 0 : 1; }\n";
    let mut failures = Vec::new();
    for precise in [false, true] {
        for force in [false, true] {
            let files = [("api.hpp", header), ("compute.cpp", source)];
            let ws = Workspace::new(&files);
            assert!(native(&ws, "compute.cpp", "before", &[]));
            let refs = if precise {
                vec![
                    ("api.hpp", header.find("calculate").unwrap()),
                    ("compute.cpp", source.rfind("calculate").unwrap()),
                ]
            } else {
                vec![]
            };
            let preview = wrap(&ws, "compute.cpp", "calculate", &refs, false, force)
                .await
                .unwrap();
            unchanged(&ws, &files);
            let applied = wrap(&ws, "compute.cpp", "calculate", &refs, true, force).await;
            if let Ok(result) = applied {
                assert!(result.applied);
                let compiled = native(&ws, "compute.cpp", "after", &[]);
                let expected = header.replace(
                    "decltype(sizeof(\"x\")) calculate",
                    "std::optional<decltype(sizeof(\"x\"))> calculate",
                );
                let actual = std::fs::read_to_string(ws.root().join("api.hpp")).unwrap();
                if !compiled || actual != expected || !preview.unmatched.is_empty() {
                    failures.push(format!("precise={precise} force={force}: {actual:?}; compiled={compiled}; {preview:?}"));
                }
            } else {
                failures.push(format!("precise={precise} force={force}: {applied:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
