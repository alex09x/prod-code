/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};

#[tokio::test]
async fn shadow_applies_winning_in_memory_proposal() {
    let original = "pub fn sample() {}\n";
    let proposed = "pub fn changed() {}\n";
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname=\"sample\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        ),
        ("src/lib.rs", original),
    ]);
    let spec = json!({"ram":true,"hypotheses":[{
        "name":"proposal","edits":[{"path":"src/lib.rs","new_text":proposed}]
    }]});
    ws.write("spec.json", &spec.to_string());
    let gateway = ScriptedGateway::start(|method, params| {
        if method == "prod-code/shadow-run" {
            assert_eq!(params["command"], json!(["sample-check"]));
            json!({"exit_code":0})
        } else {
            Value::Null
        }
    })
    .await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("-r")
        .arg(gateway.addr().to_string())
        .args(["shadow-run", "spec.json", "--apply", "--", "sample-check"])
        .env("HOME", ws.root())
        .current_dir(ws.root())
        .output()
        .await
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(ws.read("src/lib.rs"), proposed);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("proposal"), "{text}");
}
