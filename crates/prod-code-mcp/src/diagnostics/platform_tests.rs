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
use crate::diagnostics::parse::parse_items;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

#[test]
fn runtime_paths_2_platform_exclusions_have_precise_reasons() {
    for (suffix, expected) in [
        (
            "darwin",
            "Darwin / macOS build tags excluded on current host",
        ),
        ("linux", "Linux build tags excluded on current host"),
        ("windows", "Windows build tags excluded on current host"),
        ("freebsd", "BSD build tags excluded on current host"),
        ("openbsd", "BSD build tags excluded on current host"),
        (
            "other",
            "excluded by platform build constraints: NO PACKAGE METADATA",
        ),
    ] {
        let reason = platform_exclusion_reason(
            Path::new(&format!("file_{suffix}.go")),
            &anyhow::anyhow!("NO PACKAGE METADATA\nmore detail"),
        )
        .unwrap();
        assert_eq!(reason, expected);
        let report = platform_excluded_report("file.go", &reason);
        assert_eq!(
            (
                report.file.as_str(),
                report.errors,
                report.warnings,
                report.items.len()
            ),
            ("file.go", 0, 0, 1)
        );
        assert_eq!(
            (
                report.items[0].severity.as_str(),
                report.items[0].code.as_deref(),
                report.items[0].message.as_str(),
                report.items[0].line,
                report.items[0].col
            ),
            ("info", Some("platform-excluded"), expected, 1, 1)
        );
        assert_eq!(report.is_platform_excluded(), Some(expected));
    }
    for message in [
        "no packages found",
        "build constraints exclude all go files",
        "cannot find package",
        "no such package",
    ] {
        assert_eq!(
            platform_exclusion_reason(Path::new("a.go"), &anyhow::anyhow!(message)),
            Some(format!("excluded by platform build constraints: {message}"))
        );
    }
    assert_eq!(
        platform_exclusion_reason(Path::new("a_linux.go"), &anyhow::anyhow!("syntax error")),
        None
    );
}

#[tokio::test]
async fn runtime_paths_2_swift_reconciliation_suppresses_only_resolution_errors() {
    let ws = Workspace::new(&[
        ("Package.swift", "// swift-tools-version: 6.0\n"),
        ("Sources/App/a.swift", "let x = 1\n"),
    ]);
    for exit_code in [0, 1] {
        let gateway = ScriptedGateway::start(move |method, _| {
            if method == "prod-code/shadow-run" {
                serde_json::json!({"exit_code": exit_code})
            } else {
                serde_json::Value::Null
            }
        })
        .await;
        let mut value =
            answers::error_at(1, 1, "resolution", "cannot find type 'NewType' in scope");
        let mut warning = value["items"][0].clone();
        warning["severity"] = serde_json::json!(2);
        warning["message"] = serde_json::json!("unused variable");
        let mut syntax = value["items"][0].clone();
        syntax["message"] = serde_json::json!("expected expression");
        value["items"]
            .as_array_mut()
            .unwrap()
            .extend([warning, syntax]);
        let mut reports = vec![parse_items("Sources/App/a.swift", &value)];
        reconcile_swift_cross_target_diagnostics(
            gateway.addr(),
            &ws.root(),
            &[("Sources/App/a.swift".into(), "let x = NewType()".into())],
            &mut reports,
        )
        .await
        .unwrap();
        assert_eq!(
            (reports[0].errors, reports[0].warnings),
            if exit_code == 0 { (1, 1) } else { (2, 1) }
        );
        assert_eq!(
            reports[0]
                .items
                .iter()
                .map(|i| i.message.as_str())
                .collect::<Vec<_>>(),
            if exit_code == 0 {
                vec!["unused variable", "expected expression"]
            } else {
                vec![
                    "cannot find type 'NewType' in scope",
                    "unused variable",
                    "expected expression",
                ]
            }
        );
    }
}
