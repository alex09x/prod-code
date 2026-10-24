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
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::json;

#[tokio::test]
async fn compiler_verdict_controls_writes_and_force_overrides_rejection() {
    for (analyzer_clean, apply, force, exit, passed, written) in [
        (false, true, false, 0, false, false),
        (true, false, false, 0, true, false),
        (true, true, false, 0, true, true),
        (true, true, false, 1, false, false),
        (true, false, false, 1, false, false),
        (false, true, true, 1, false, true),
    ] {
        let ws = Workspace::new(&[("sample.rs", "fn old() {}\n")]);
        let gw = ScriptedGateway::start(move |method, params| {
            if method == "prod-code/shadow-run" {
                assert_eq!(
                    params["command"],
                    json!([
                        "cargo",
                        "check",
                        "--workspace",
                        "--all-targets",
                        "--message-format=short"
                    ])
                );
                json!({"exit_code":exit})
            } else {
                json!(null)
            }
        })
        .await;
        let gate = compile_gate(
            gw.addr(),
            &ws.root(),
            &[(
                ws.path("sample.rs").to_string_lossy().into_owned(),
                "fn new() {}\n".into(),
            )],
            analyzer_clean,
            apply,
            force,
        )
        .await
        .unwrap();
        assert_eq!((gate.passed, gate.applied), (passed, written));
        let expected = if !analyzer_clean && !force {
            "\nthe compiler was not asked: the analyzer already rejects the result\n".to_string()
        } else if exit == 0 {
            "\nthe compiler accepts the result too: `cargo check` in a shadow of the workspace, 10 ms\n".to_string()
        } else {
            format!(
                "\nthe compiler rejects the result (`cargo check` in a shadow of the workspace, 10 ms):\n  (it failed without an error line this could read; see the build)\n{}",
                if apply && !force {
                    "\nnothing was written: the compiler rejects it. Pass `force: true` to write it anyway.\n"
                } else {
                    ""
                }
            )
        };
        assert_eq!(gate.text, expected);
        assert_eq!(
            ws.read("sample.rs"),
            if written {
                "fn new() {}\n"
            } else {
                "fn old() {}\n"
            }
        );
    }
}

#[test]
fn incomplete_plan_is_refused_only_when_writing() {
    assert_eq!(refuse_incomplete(false, &["first".into()]).unwrap(), ());
    assert_eq!(refuse_incomplete(true, &[]).unwrap(), ());
    assert_eq!(
        refuse_incomplete(true, &["first".into(), "second".into()])
            .unwrap_err()
            .to_string(),
        "2 reference(s) were not rewritten; nothing was written:\n  first\n  second"
    );
}

#[test]
fn path_resolution_preserves_existing_absolute_and_missing_paths() {
    let ws = Workspace::new(&[("src/sample.rs", "")]);
    assert_eq!(
        resolve_file_path(&ws.root(), "src/sample.rs"),
        ws.path("src/sample.rs")
    );
    assert_eq!(
        resolve_file_path(&ws.root(), ws.path("src/sample.rs").to_str().unwrap()),
        ws.path("src/sample.rs")
    );
    assert_eq!(
        resolve_file_path(&ws.root(), "/missing/sample.rs"),
        std::path::PathBuf::from("/missing/sample.rs")
    );
    ws.write("missing/sample.rs", "");
    assert_eq!(
        resolve_file_path(&ws.root(), "/missing/sample.rs"),
        ws.path("missing/sample.rs")
    );
}
