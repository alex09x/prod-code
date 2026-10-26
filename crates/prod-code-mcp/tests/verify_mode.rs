/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A `verify` mode other than `compile` is refused before anything is asked or written: a
//! typo used to read as "no compiler gate" and let an apply through unchecked (#1453).

use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};

const LIB: &str = "pub fn enabled() -> bool {\n    true\n}\n";

#[tokio::test]
async fn an_unknown_verify_mode_is_refused_before_planning() {
    for mode in ["compiler", "Compile", "cargo", ""] {
        let ws = Workspace::new(&[("src/lib.rs", LIB)]);
        let gw = ScriptedGateway::start(|_, _| Value::Null).await;
        let args = json!({
            "path": "src/lib.rs", "line": 1, "character": 8, "new_name": "disabled",
            "apply": true, "verify": mode
        });
        let err = execute_tool(gw.addr(), &ws.root(), "code_invert_boolean", args)
            .await
            .expect_err("an unknown verify mode is refused");
        let text = format!("{err:#}");
        assert!(
            text.contains("is not a verification mode"),
            "{mode:?}: {text}"
        );
        assert!(text.contains("Nothing was planned or written"), "{text}");
        assert_eq!(gw.calls(), 0, "{mode:?}: the analyzer is not asked");
        assert_eq!(ws.read("src/lib.rs"), LIB, "{mode:?}: nothing is written");
    }
}

#[tokio::test]
async fn compile_absent_and_the_fixture_boolean_still_pass_the_check() {
    let ws = Workspace::new(&[("src/lib.rs", LIB)]);
    let gw = ScriptedGateway::start(|_, _| Value::Null).await;
    for args in [
        json!({ "path": "src/lib.rs", "line": 1, "character": 8, "new_name": "disabled", "verify": "compile" }),
        json!({ "path": "src/lib.rs", "line": 1, "character": 8, "new_name": "disabled" }),
    ] {
        // Whatever the scripted gateway makes of it, the mode itself is not what refuses it.
        if let Err(err) = execute_tool(gw.addr(), &ws.root(), "code_invert_boolean", args).await {
            assert!(
                !format!("{err:#}").contains("is not a verification mode"),
                "{err:#}"
            );
        }
    }
    let fixture = json!({ "path": "src/lib.rs", "symbol": "enabled", "verify": false });
    if let Err(err) = execute_tool(gw.addr(), &ws.root(), "code_generate_fixture", fixture).await {
        assert!(
            !format!("{err:#}").contains("is not a verification mode"),
            "{err:#}"
        );
    }
}
