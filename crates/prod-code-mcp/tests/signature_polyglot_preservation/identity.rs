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
use prod_code_mcp::signature::{Modifiers, Param, change_with};
use prod_code_testkit::Workspace;

#[tokio::test]
async fn cpp_preserves_unrelated_prototypes_and_updates_selected_header() {
    let header = "namespace chosen {\nint f(int a, int b);\n}\nnamespace other {\nint f(int a, int b);\nint f(double a, double b);\n}\n";
    let source = "#include \"api.h\"\n#include <cstdio>\nnamespace chosen {\nint f(int a, int b) { return a*10+b; }\n}\nnamespace other {\nint f(int a, int b) { return a*100+b; }\nint f(double a, double b) { return 7; }\n}\nint main() { printf(\"%d %d %s\\n\", chosen::f(1, 2), other::f(3, 4), \"f(int a, int b)\"); }\n";
    let ws = Workspace::new(&[("api.h", header), ("main.cpp", source)]);
    let root = &ws.root();
    evidence(root, "main.cpp", "original");
    evidence(root, "api.h", "original");
    run(root, "c++", &["main.cpp", "-o", "program"]);
    let original = run(root, "./program", &[]);
    let (refs, declarations) = super::semantic::cpp_references(root, source).await;
    let identities = refs
        .iter()
        .chain(&declarations)
        .map(|r| {
            serde_json::json!({
                "uri": r["uri"], "range": {"start": r["range"]["start"]}
            })
        })
        .collect::<Vec<_>>();
    for expected in [
        location(root, "api.h", header, "f(int a, int b)"),
        location(root, "main.cpp", source, "f(int a, int b)"),
        location(root, "main.cpp", source, "f(1, 2)"),
    ] {
        assert!(
            identities.contains(&expected),
            "clangd must identify every selected declaration/call"
        );
    }
    let gw = gateway_with_declarations(refs, declarations).await;
    change_with(
        gw.addr(),
        root,
        &root.join("main.cpp"),
        4,
        5,
        &[Param::Keep("b".into()), Param::Keep("a".into())],
        &Modifiers::default(),
        true,
        false,
    )
    .await
    .unwrap();
    evidence(root, "main.cpp", "generated");
    evidence(root, "api.h", "generated");
    run(root, "c++", &["main.cpp", "-o", "program"]);
    assert_eq!(run(root, "./program", &[]), original);
    assert_eq!(
        std::fs::read_to_string(root.join("api.h")).unwrap(),
        header.replacen("f(int a, int b)", "f(int b, int a)", 1)
    );
}

#[tokio::test]
async fn cpp_selected_header_reconciles_definition_names_without_added_defaults() {
    let header = "namespace chosen {\nint f(int left, double right);\n}\nnamespace other {\nint f(int left, double right);\n}\n";
    let source = "#include \"api.h\"\n#include <cstdio>\nnamespace chosen {\nint f(int a, double b) { return a*10+int(b); }\n}\nnamespace other {\nint f(int left, double right) { return left*100+int(right); }\n}\nint main() { printf(\"%d %d\\n\", chosen::f(1, 2.0), other::f(3, 4.0)); }\n";
    let ws = Workspace::new(&[("api.h", header), ("main.cpp", source)]);
    let root = &ws.root();
    run(root, "c++", &["main.cpp", "-o", "program"]);
    let original = run(root, "./program", &[]);
    let gw = gateway_with_declarations(
        vec![
            location(root, "main.cpp", source, "f(int a, double b)"),
            location(root, "main.cpp", source, "f(1, 2.0)"),
        ],
        vec![location(root, "api.h", header, "f(int left, double right)")],
    )
    .await;
    change_with(
        gw.addr(),
        root,
        &root.join("api.h"),
        2,
        5,
        &[
            Param::Keep("right".into()),
            Param::Keep("left".into()),
            Param::Add {
                name: "offset".into(),
                ty: "int".into(),
                value: "0".into(),
            },
        ],
        &Modifiers::default(),
        true,
        false,
    )
    .await
    .unwrap();
    let generated = std::fs::read_to_string(root.join("main.cpp")).unwrap();
    assert!(generated.contains("int f(double b, int a, int offset)"));
    assert!(!generated.contains("offset ="));
    assert_eq!(
        std::fs::read_to_string(root.join("api.h")).unwrap(),
        header.replacen(
            "int f(int left, double right)",
            "int f(double right, int left, int offset)",
            1
        )
    );
    run(root, "c++", &["main.cpp", "-o", "program"]);
    assert_eq!(run(root, "./program", &[]), original);
}

#[tokio::test]
async fn c_prototype_and_definition_share_identity() {
    let header = "int f(int x, int y);\nint unrelated(int x, int y);\n";
    let source = "#include \"api.h\"\n#include <stdio.h>\nint f(int a, int b) { return a*10+b; }\nint main(void) { printf(\"%d\\n\", f(1, 2)); }\n";
    let ws = Workspace::new(&[("api.h", header), ("main.c", source)]);
    let root = &ws.root();
    run(root, "cc", &["main.c", "-o", "program"]);
    let original = run(root, "./program", &[]);
    let gw = gateway_with_declarations(
        vec![location(root, "main.c", source, "f(1, 2)")],
        vec![location(root, "api.h", header, "f(int x, int y)")],
    )
    .await;
    change_with(
        gw.addr(),
        root,
        &root.join("main.c"),
        3,
        5,
        &[Param::Keep("b".into()), Param::Keep("a".into())],
        &Modifiers::default(),
        true,
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("api.h")).unwrap(),
        header.replacen("f(int x, int y)", "f(int y, int x)", 1)
    );
    run(root, "cc", &["main.c", "-o", "program"]);
    assert_eq!(run(root, "./program", &[]), original);
}
