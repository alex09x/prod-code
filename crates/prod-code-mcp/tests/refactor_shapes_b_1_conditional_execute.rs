/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Replacing a conditional with polymorphism end to end: what is refused before anything is
//! generated, and what validation does to a preview, an apply and a forced apply.

use prod_code_mcp::replace_conditional::replace_conditional_impl;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;

const BIRD: &str = "export function getSpeed(type: string): number {\n    switch (type) {\n        case \"EUROPEAN\":\n            return 10;\n        case \"AFRICAN\":\n            return 8;\n        default:\n            throw new Error(\"Unknown\");\n    }\n}\n";

async fn gateway(diagnostics: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/diagnostic" => diagnostics.clone(),
        _ => Value::Null,
    })
    .await
}

#[allow(clippy::too_many_arguments)]
async fn convert(
    gw: &ScriptedGateway,
    ws: &Workspace,
    name: &str,
    line: u32,
    target: Option<&str>,
    apply: bool,
    force: bool,
) -> anyhow::Result<prod_code_mcp::replace_conditional::types::ReplaceConditionalResult> {
    replace_conditional_impl(
        gw.addr(),
        &ws.root(),
        &ws.path(name),
        line,
        5,
        "Bird",
        "getSpeed",
        &[],
        Some("number"),
        target,
        apply,
        force,
        None,
    )
    .await
}

#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_before_any_code_is_generated() {
    let ws = Workspace::new(&[
        ("bird.ts", BIRD),
        ("plain.ts", "const x = 1;\n"),
        ("bird.txt", BIRD),
    ]);
    let gw = gateway(answers::no_diagnostics()).await;
    let refuse = |name: &'static str, line: u32, target: Option<&'static str>| {
        let (gw, ws) = (&gw, &ws);
        async move {
            convert(gw, ws, name, line, target, false, false)
                .await
                .unwrap_err()
                .to_string()
        }
    };
    assert_eq!(
        refuse("bird.ts", 0, Some("bird")).await,
        "replace_conditional requires one-based line and character coordinates"
    );
    assert_eq!(
        refuse("bird.ts", 99, Some("bird")).await,
        "requested position is outside the document or splits a UTF-16 surrogate pair"
    );
    assert_eq!(
        refuse("gone.ts", 2, Some("bird")).await,
        format!("cannot read file {}", ws.path("gone.ts").display())
    );
    assert_eq!(
        refuse("plain.ts", 1, Some("bird")).await,
        format!(
            "no switch, match, or if-else conditional block found in {} around line 1",
            ws.path("plain.ts").display()
        )
    );
    let missing_target =
        "`target_var` is required; the discriminator is not necessarily the polymorphic receiver";
    assert_eq!(refuse("bird.ts", 2, None).await, missing_target);
    assert_eq!(refuse("bird.ts", 2, Some("  ")).await, missing_target);
    let other = refuse("bird.txt", 2, Some("bird")).await;
    assert!(
        other.starts_with("unsupported language for replace_conditional: "),
        "{other}"
    );
    // A request that is refused changes nothing.
    assert_eq!(ws.read("bird.ts"), BIRD);
}

#[tokio::test]
async fn validation_errors_reject_an_apply_unless_it_is_forced_and_never_a_preview() {
    let ws = Workspace::new(&[("bird.ts", BIRD)]);
    let broken = gateway(answers::error_at(
        2,
        3,
        "TS2304",
        "Cannot find name 'Bird'\nmore",
    ))
    .await;

    let preview = convert(&broken, &ws, "bird.ts", 2, Some("bird"), false, false)
        .await
        .unwrap();
    assert!(!preview.applied && !preview.verified);
    assert_eq!(preview.diagnostics.len(), 1);
    assert!(
        preview.diagnostics[0].starts_with("Cannot find name 'Bird' [TS2304] ("),
        "{:?}",
        preview.diagnostics
    );
    assert_eq!(preview.variants, vec!["European", "African", "Default"]);
    assert_eq!(
        preview.files_modified,
        vec![ws.path("bird.ts").to_string_lossy().into_owned()]
    );
    let header = format!("--- {0}\n+++ {0}\n", ws.path("bird.ts").display());
    assert!(preview.diff.starts_with(&header), "{}", preview.diff);
    assert_eq!(preview.overlays.len(), 1);
    assert_eq!(ws.read("bird.ts"), BIRD);

    let rejected = convert(&broken, &ws, "bird.ts", 2, Some("bird"), true, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        rejected
            .starts_with("refactoring rejected by validation:\nCannot find name 'Bird' [TS2304] ("),
        "{rejected}"
    );
    assert_eq!(ws.read("bird.ts"), BIRD);

    let forced = convert(&broken, &ws, "bird.ts", 2, Some("bird"), true, true)
        .await
        .unwrap();
    assert!(forced.applied);
    assert_eq!(forced.diagnostics.len(), 1);
    assert_eq!(ws.read("bird.ts"), forced.overlays[0].1);
    assert!(ws.read("bird.ts").contains("export interface Bird {"));
}
