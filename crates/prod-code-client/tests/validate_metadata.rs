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

#[tokio::test]
async fn json_files_such_as_package_json_are_syntax_validated_without_lsp_error() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn answer() {}\n")]);
    let gw = ScriptedGateway::start(|_, _| serde_json::Value::Null).await;

    // 1. Valid package.json proposal succeeds with 0 errors (#733)
    let valid_pkg = r#"{
  "name": "prod-browser",
  "version": "1.0.0"
}"#;
    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_validate_edit",
        json!({
            "path": "package.json",
            "new_text": valid_pkg
        }),
    )
    .await
    .expect("validation succeeds on valid JSON");
    assert!(!res.is_error, "clean JSON should have 0 errors: {res:?}");
    let text = match &res.content[0] {
        prod_code_mcp::protocol::McpContentItem::Text { text } => text,
    };
    assert!(text.contains("package.json: 0 error(s), 0 warning(s)"), "{text}");

    // 2. Malformed package.json proposal reports syntax error at line and column
    let malformed_pkg = "{\n  \"name\": \"prod-browser\",\n}";
    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_validate_edit",
        json!({
            "path": "package.json",
            "new_text": malformed_pkg
        }),
    )
    .await
    .expect("validation reports syntax error without crashing");
    assert!(res.is_error, "malformed JSON must fail validation");
    let text = match &res.content[0] {
        prod_code_mcp::protocol::McpContentItem::Text { text } => text,
    };
    assert!(text.contains("1 error(s)"), "{text}");
    assert!(text.contains("JSON syntax error"), "{text}");

    // 3. Multi-file edits with package.json validated cleanly
    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_validate_edits",
        json!({
            "edits": [
                { "path": "package.json", "new_text": valid_pkg },
                { "path": "tsconfig.json", "new_text": "{ \"compilerOptions\": {} }" }
            ]
        }),
    )
    .await
    .expect("multi-file JSON validation succeeds");
    assert!(!res.is_error, "all clean JSON: {res:?}");

    // Gateway was never queried for JSON syntax validation
    assert_eq!(gw.calls(), 0, "JSON files are validated directly in memory");
}

