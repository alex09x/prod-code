/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `inline_parameter_polyglot`: how the parameter is chosen, what the preview and the write
//! refuse, and what the rewritten files look like.

#[allow(dead_code)]
#[path = "coverage_w2_mcp_runtime_inline_parameter/support.rs"]
mod support;

use prod_code_testkit::{Workspace, answers};
use support::*;

#[tokio::test]
async fn the_request_is_refused_before_any_analyzer_call_when_it_names_nothing_usable() {
    let ws = Workspace::new(&[
        ("math.ts", MATH_TS),
        ("notes.txt", "clamp(1, 2)\n"),
        ("big.ts", "export const b = 1;\n"),
    ]);
    let mut request = Request::new("notes.txt", "clamp", "max");
    let err = request.run(dead_end(), &ws).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "unsupported language for {}",
            ws.path("notes.txt").display()
        )
    );

    request.file = "absent.ts";
    let err = request.run(dead_end(), &ws).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("cannot read {}", ws.path("absent.ts").display())
    );

    // Neither a function nor a line that sits near a declaration.
    request.file = "big.ts";
    request.function = None;
    request.line = Some(1);
    let err = request.run(dead_end(), &ws).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "could not determine function name to inline parameter from"
    );

    request.file = "math.ts";
    request.function = Some("missing");
    let err = request.run(dead_end(), &ws).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "could not find declaration for function `missing`"
    );
}

#[tokio::test]
async fn the_parameter_is_chosen_by_name_by_position_or_as_the_only_one() {
    let ws = ts_workspace();
    let gw = gateway(client_calls(&ws), answers::no_diagnostics()).await;

    let mut request = Request::new("math.ts", "clamp", "nope");
    let err = request.run(gw.addr(), &ws).await.unwrap_err();
    assert_eq!(err.to_string(), "parameter `nope` not found in `clamp`");

    request.param = None;
    let err = request.run(gw.addr(), &ws).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "multiple parameters in `clamp`; specify which parameter to inline using `parameter`"
    );

    // A position outside the file, and one that is not on a parameter's name.
    request.line = Some(99);
    request.character = Some(1);
    let err = request.run(gw.addr(), &ws).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "the parameter position is not inside the file"
    );
    request.line = Some(1);
    request.character = Some(1);
    let err = request.run(gw.addr(), &ws).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "the selected position is not inside a parameter name"
    );

    // The function named by the line, the parameter by the column under the cursor.
    request.function = None;
    request.character = Some(MATH_TS.find("max").unwrap() as u32 + 2);
    let res = request.run(gw.addr(), &ws).await.unwrap();
    assert_eq!(
        (res.function.as_str(), res.parameter.as_str()),
        ("clamp", "max")
    );
    assert_eq!(res.value, "100");
    assert_eq!(res.rewritten_calls, 2);
    assert!(!res.applied && res.unmatched.is_empty() && res.diagnostics.is_empty());
    assert_eq!(
        rewritten(&res, "math.ts"),
        "export function clamp(x: number): number {\n    const max: number = 100;\n    return Math.min(x, max);\n}\n"
    );
    assert_eq!(
        rewritten(&res, "client.ts"),
        CLIENT_TS
            .replace("clamp(10, 100)", "clamp(10)")
            .replace("clamp(20, 100)", "clamp(20)")
    );
    // A preview writes nothing.
    assert_eq!(ws.read("math.ts"), MATH_TS);
    assert_eq!(ws.read("client.ts"), CLIENT_TS);
    let text = res.render(10_000);
    assert!(
        text.contains("nothing was written; pass `apply: true`"),
        "{text}"
    );
    assert!(
        text.contains("the analyzer accepts the result: 0 errors"),
        "{text}"
    );

    // The only parameter needs no name.
    let ws = Workspace::new(&[
        (
            "scale.ts",
            "export function scale(factor: number) {\n    return 4 * factor;\n}\n",
        ),
        ("use.ts", "scale(2);\nscale(2);\n"),
    ]);
    let use_ts = ws.path("use.ts");
    let source = "scale(2);\nscale(2);\n";
    let gw = gateway(
        vec![
            reference_at(&use_ts, source, "scale", 0),
            reference_at(&use_ts, source, "scale", 1),
        ],
        answers::no_diagnostics(),
    )
    .await;
    let mut request = Request::new("scale.ts", "scale", "factor");
    request.param = None;
    request.apply = true;
    let res = request.run(gw.addr(), &ws).await.unwrap();
    assert_eq!(res.value, "2");
    assert!(res.applied);
    assert_eq!(
        ws.read("scale.ts"),
        "export function scale() {\n    const factor: number = 2;\n    return 4 * factor;\n}\n"
    );
    assert_eq!(ws.read("use.ts"), "scale();\nscale();\n");
}

#[tokio::test]
async fn nothing_is_inlined_when_no_call_passes_a_value() {
    let ws = Workspace::new(&[("math.ts", MATH_TS)]);
    let gw = gateway(vec![], answers::no_diagnostics()).await;
    let err = Request::new("math.ts", "clamp", "max")
        .run(gw.addr(), &ws)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "no call passes a value for `max`, so there is none to inline"
    );
}

#[tokio::test]
async fn an_analyzer_error_blocks_the_write_unless_forced() {
    let ws = ts_workspace();
    let diagnostics = answers::error_at(2, 12, "TS2304", "Cannot find name 'max'.\nsecond line");
    let gw = gateway(client_calls(&ws), diagnostics).await;
    let mut request = Request::new("math.ts", "clamp", "max");
    request.apply = true;

    // Only the first line of the message is kept. An error past the end of its line (client.ts
    // has an empty line 2) still counts: an analyzer error is never dropped for its position.
    let err = request.run(gw.addr(), &ws).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "the change does not compile (2 error(s)); nothing was written. Pass `force: true` to write it anyway:\n  Cannot find name 'max'. [TS2304] (client.ts:2:12)\n  Cannot find name 'max'. [TS2304] (math.ts:2:12)"
    );
    assert_eq!(ws.read("math.ts"), MATH_TS);
    assert_eq!(ws.read("client.ts"), CLIENT_TS);

    request.force = true;
    let res = request.run(gw.addr(), &ws).await.unwrap();
    assert!(res.applied);
    assert_eq!(
        res.diagnostics,
        [
            "Cannot find name 'max'. [TS2304] (client.ts:2:12)",
            "Cannot find name 'max'. [TS2304] (math.ts:2:12)"
        ]
    );
    assert!(
        res.render(10_000)
            .contains("the analyzer rejects the result:")
    );
    assert_eq!(
        ws.read("math.ts"),
        "export function clamp(x: number): number {\n    const max: number = 100;\n    return Math.min(x, max);\n}\n"
    );
}
