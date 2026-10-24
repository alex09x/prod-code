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

const SOURCE: &str = "function run(kind: string) {\n    switch (kind) {\n        case 'fast': return 7;\n        default: return 0;\n    }\n}\n";

#[tokio::test]
async fn conditional_forwards_positions_filtered_params_and_diagnostics() {
    for custom in [false, true] {
        for reject in [false, true] {
            let ws = Workspace::new(&[("action.ts", SOURCE)]);
            let root = ws.root();
            let file = ws.path("action.ts");
            let gw = gateway(reject).await;
            let mut args = json!({"path": "action.ts", "base_name": "Action", "method_name": "run", "line": 2, "character": 5, "target_var": "action"});
            if custom {
                args["params"] = json!(["value: number", 9, null, "label: string"]);
                args["return_type"] = json!("number");
                args["verify"] = json!("none");
                args["force"] = json!(true);
            }
            let params = if custom {
                vec!["value: number".into(), "label: string".into()]
            } else {
                vec![]
            };
            let expected = crate::replace_conditional::replace_conditional_impl(
                gw.addr(),
                &root,
                &file,
                2,
                5,
                "Action",
                "run",
                &params,
                custom.then_some("number"),
                Some("action"),
                false,
                custom,
                custom.then_some("none"),
            )
            .await
            .unwrap();
            assert_eq!(expected.base_name, "Action");
            assert_eq!(expected.method_name, "run");
            assert_eq!(expected.variants, ["Fast", "Default"]);
            assert_eq!(expected.applied, false);
            assert_eq!(expected.diagnostics.is_empty(), !reject);
            let response = handle_replace_conditional_with_polymorphism(gw.addr(), &root, &args)
                .await
                .unwrap();
            assert_response(response, expected.render(2048), reject);
            assert_eq!(ws.read("action.ts"), SOURCE);
        }
    }
}

#[tokio::test]
async fn conditional_rejects_invalid_coordinates_and_missing_receiver() {
    let ws = Workspace::new(&[("action.ts", SOURCE), ("unicode.ts", "// 🦀\r\n")]);
    let gw = gateway(false).await;
    for (extra, message) in [
        (
            json!({}),
            "replace_conditional requires one-based line and character coordinates",
        ),
        (
            json!({"line": -1, "character": 1}),
            "replace_conditional requires one-based line and character coordinates",
        ),
        (
            json!({"line": "2", "character": 5}),
            "replace_conditional requires one-based line and character coordinates",
        ),
        (
            json!({"line": 2, "character": null}),
            "replace_conditional requires one-based line and character coordinates",
        ),
        (
            json!({"line": 99, "character": 1}),
            "requested position is outside the document or splits a UTF-16 surrogate pair",
        ),
        (
            json!({"path": "unicode.ts", "line": 1, "character": 5}),
            "requested position is outside the document or splits a UTF-16 surrogate pair",
        ),
        (
            json!({"line": 2, "character": 5}),
            "`target_var` is required; the discriminator is not necessarily the polymorphic receiver",
        ),
        (
            json!({"line": 2, "character": 5, "target_var": "  "}),
            "`target_var` is required; the discriminator is not necessarily the polymorphic receiver",
        ),
    ] {
        let mut args = json!({"path": "action.ts", "base_name": "Action", "method_name": "run"});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            handle_replace_conditional_with_polymorphism(gw.addr(), &ws.root(), &args)
                .await
                .unwrap_err()
                .to_string(),
            message,
            "{args}"
        );
    }
}

#[tokio::test]
async fn conditional_apply_writes_the_exact_preview() {
    let ws = Workspace::new(&[("action.ts", SOURCE)]);
    let root = ws.root();
    let file = ws.path("action.ts");
    let gw = gateway(false).await;
    let expected = crate::replace_conditional::replace_conditional_impl(
        gw.addr(),
        &root,
        &file,
        2,
        5,
        "Action",
        "run",
        &[],
        None,
        Some("action"),
        false,
        false,
        None,
    )
    .await
    .unwrap();
    let args = json!({"path": "action.ts", "base_name": "Action", "method_name": "run", "line": 2, "character": 5, "target_var": "action", "apply": true});
    let response = handle_replace_conditional_with_polymorphism(gw.addr(), &root, &args)
        .await
        .unwrap();
    assert_eq!(response.is_error, false);
    assert_eq!(ws.read("action.ts"), expected.overlays[0].1);
}
