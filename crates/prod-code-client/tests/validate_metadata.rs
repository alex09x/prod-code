//! Non-source proposal files get an actionable refusal before the analyzer is asked.
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

#[tokio::test]
async fn metadata_validation_names_the_limit_without_sending_it_to_a_language_server() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"metadata\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("Cargo.lock", "version = 4\n"),
        ("CHANGELOG.md", "# Changes\n"),
        ("src/lib.rs", "pub fn answer() {}\n"),
    ]);
    let gw = ScriptedGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    for file in ["Cargo.toml", "Cargo.lock", "CHANGELOG.md"] {
        let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
            .arg("--remote")
            .arg(gw.addr().to_string())
            .args(["validate", file, "--from", file, "--json"])
            .env("PROD_CODE_REMOTE", gw.addr().to_string())
            .current_dir(ws.root())
            .output()
            .await
            .unwrap();
        let shown = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{file}: {shown}");
        assert!(
            shown.contains(file) && shown.contains("not supported") && shown.contains("shadow-run"),
            "{shown}"
        );
    }
    for (tool, args) in [
        (
            "code_validate_edit",
            json!({"path":"Cargo.lock","new_text":"version = 4\n"}),
        ),
        (
            "code_validate_edits",
            json!({"edits":[{"path":"src/lib.rs","new_text":"pub fn changed() {}\n"},{"path":"CHANGELOG.md","new_text":"# Proposed changes\n"}]}),
        ),
    ] {
        let err = prod_code_mcp::tools::execute_tool(gw.addr(), &ws.root(), tool, args)
            .await
            .expect_err("unsupported metadata is not certified");
        let shown = format!("{err:#}");
        assert!(
            shown.contains("not supported") && shown.contains("shadow-run"),
            "{shown}"
        );
    }
    assert_eq!(
        gw.calls(),
        0,
        "no unsupported or partially supported batch is sent to the server"
    );
    assert_eq!(ws.read("Cargo.lock"), "version = 4\n");
    assert_eq!(ws.read("CHANGELOG.md"), "# Changes\n");
    assert_eq!(ws.read("src/lib.rs"), "pub fn answer() {}\n");
}
