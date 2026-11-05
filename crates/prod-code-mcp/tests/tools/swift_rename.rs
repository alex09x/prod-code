/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A Swift rename that leaves a call in another file says so instead of only reporting success
//! (#1953).

use super::*;

const MODEL: &str = "final class SetupModel {\n    func checkSelectedFolder() {}\n}\n";
const STEP: &str = "struct SetupPermissionsStep {\n    let model: SetupModel\n    func run() {\n        model.checkSelectedFolder()\n    }\n}\n";

async fn rename(ws: &Workspace) -> String {
    let model = ws.path("Sources/SetupModel.swift");
    let renamed = MODEL.replace("checkSelectedFolder", "checkFolderAccess");
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/rename" => answers::whole_file(&model, MODEL, &renamed),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_rename",
        serde_json::json!({
            "path": "Sources/SetupModel.swift", "line": 2, "character": 11,
            "new_name": "checkFolderAccess"
        }),
    )
    .await
    .expect("the rename runs");
    let text = text_of(&result);
    assert!(!result.is_error, "{text}");
    text
}

#[tokio::test]
async fn a_call_the_index_did_not_know_is_named_in_the_report() {
    let ws = workspace();
    write(&ws, "Sources/SetupModel.swift", MODEL);
    write(&ws, "Sources/SetupPermissionsStep.swift", STEP);
    write(&ws, "Sources/Unrelated.swift", "struct Unrelated {}\n");
    commit(&ws);

    let text = rename(&ws).await;
    assert!(text.contains("renamed to `checkFolderAccess`"), "{text}");
    assert!(
        text.contains("warning: `checkSelectedFolder` is still written in 1"),
        "{text}"
    );
    assert!(
        text.contains("Sources/SetupPermissionsStep.swift:4"),
        "the file and line of the call: {text}"
    );
    assert!(!text.contains("Unrelated.swift"), "{text}");
}

#[tokio::test]
async fn a_rename_that_reached_every_file_has_no_warning() {
    let ws = workspace();
    write(&ws, "Sources/SetupModel.swift", MODEL);
    write(&ws, "Sources/Unrelated.swift", "struct Unrelated {}\n");
    commit(&ws);

    let text = rename(&ws).await;
    assert!(text.contains("renamed to `checkFolderAccess`"), "{text}");
    assert!(!text.contains("warning:"), "{text}");
}
