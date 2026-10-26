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

const MANIFEST: &str = "// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(name: \"App\", targets: [.target(name: \"App\"), .target(name: \"Other\")])\n";
const DECL: &str = "func calculate() -> Int { return 1 }\n";
const CLIENT: &str = "func run() -> Int {\n    return calculate()\n}\n";

#[tokio::test]
async fn recursive_swift_caller_is_not_omitted() {
    let files = [
        ("Package.swift", MANIFEST),
        ("Sources/App/Utils/helper.swift", DECL),
        ("Sources/App/client.swift", CLIENT),
    ];
    for precise in [false, true] {
        let ws = Workspace::new(&files);
        let refs = if precise {
            vec![(
                "Sources/App/client.swift",
                CLIENT.find("calculate").unwrap(),
            )]
        } else {
            vec![]
        };
        let preview = wrap(&ws, files[1].0, "calculate", &refs, false, false)
            .await
            .unwrap();
        eprintln!(
            "swift precise={precise}: propagated={} blocked={:?} unmatched={:?}",
            preview.propagated, preview.blocked, preview.unmatched
        );
        assert_eq!(preview.blocked.len(), 1);
        assert!(
            wrap(&ws, files[1].0, "calculate", &refs, true, false)
                .await
                .is_err()
        );
        unchanged(&ws, &files);
    }
}

#[tokio::test]
async fn recursive_swift_optional_caller_and_unrelated_target() {
    let optional = CLIENT.replace("-> Int", "-> Int?");
    let files = [
        ("Package.swift", MANIFEST),
        ("Sources/App/Utils/helper.swift", DECL),
        ("Sources/App/client.swift", optional.as_str()),
        ("Sources/App/main.swift", "print(run() ?? 0)\n"),
        (
            "Sources/Other/client.swift",
            "func calculate() -> String { return \"other\" }\nfunc run() -> String { return calculate() }\n",
        ),
    ];
    for precise in [false, true] {
        let ws = Workspace::new(&files);
        let refs = if precise {
            vec![(files[2].0, optional.find("calculate").unwrap())]
        } else {
            vec![]
        };
        let result = wrap(&ws, files[1].0, "calculate", &refs, true, false)
            .await
            .unwrap();
        assert_eq!(result.propagated, 1, "{result:?}");
        assert!(result.applied);
        unchanged(&ws, &files[2..]);
    }
}

#[tokio::test]
async fn unknown_swift_ownership_refuses_even_force() {
    let files = [("helper.swift", DECL), ("nested/client.swift", CLIENT)];
    let ws = Workspace::new(&files);
    let result = wrap(&ws, "helper.swift", "calculate", &[], true, true).await;
    assert!(result.is_err(), "{result:?}");
    unchanged(&ws, &files);
}

#[tokio::test]
async fn custom_swift_layout_is_explicit_unknown_without_manifest_execution() {
    let files = [
        (
            "Package.swift",
            "// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(name: \"App\", targets: [.target(name: \"App\", path: \"Custom\")])\n",
        ),
        ("Custom/helper.swift", DECL),
        ("Custom/nested/client.swift", CLIENT),
    ];
    let ws = Workspace::new(&files);
    let preview = wrap(&ws, files[1].0, "calculate", &[], false, false)
        .await
        .unwrap();
    assert_eq!(preview.unmatched.len(), 1, "{preview:?}");
    assert!(preview.unmatched[0].contains("ownership is unknown"));
    assert!(
        wrap(&ws, files[1].0, "calculate", &[], true, true)
            .await
            .is_err()
    );
    unchanged(&ws, &files);
}

#[tokio::test]
async fn java_remains_an_explicit_byte_preserving_refusal() {
    let files = [(
        "Selected.java",
        "class Selected { static int calculate() { return 1; } }\n",
    )];
    let ws = Workspace::new(&files);
    let error = wrap(&ws, files[0].0, "calculate", &[], true, true)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("does not support Java"));
    unchanged(&ws, &files);
}
