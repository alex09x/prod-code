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
        ("src/lib.rs", "pub fn answer() {}\n"),
    ]);
    let gw = ScriptedGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    for file in ["Cargo.toml", "Cargo.lock"] {
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
            json!({"edits":[{"path":"src/lib.rs","new_text":"pub fn changed() {}\n"},{"path":"Cargo.lock","new_text":"version = 5\n"}]}),
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

#[tokio::test]
async fn markdown_and_svg_proposals_are_syntax_validated_without_lsp_error() {
    let ws = Workspace::new(&[
        (
            "src/content/blog/example.md",
            "---\ntitle: \"Existing Post\"\npubDate: 2026-10-01\n---\n# Existing\n",
        ),
        (
            "public/img/card.svg",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\"><circle cx=\"50\" cy=\"50\" r=\"40\" fill=\"blue\" /></svg>\n",
        ),
        (
            "proposed.md",
            "---\ntitle: \"New Post\"\npubDate: 2026-10-02\n---\n# New Post\n\n```rust\nfn main() {}\n```\n",
        ),
        (
            "proposed.svg",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\"><rect width=\"100\" height=\"100\" fill=\"red\" /></svg>\n",
        ),
        (
            "broken.md",
            "---\ntitle: \"Broken Post\"\n# Unclosed frontmatter\n",
        ),
        (
            "broken.svg",
            "<svg viewBox=\"0 0 100 100\"><g><circle cx=\"10\" cy=\"10\" r=\"5\" /></path></svg>\n",
        ),
    ]);
    let gw = ScriptedGateway::start(|_, _| serde_json::Value::Null).await;

    // 1. Valid Markdown proposal passes with 0 errors (#778)
    let valid_md = ws.read("proposed.md");
    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_validate_edit",
        json!({
            "path": "src/content/blog/example.md",
            "new_text": valid_md,
        }),
    )
    .await
    .expect("validation succeeds on valid markdown");
    assert!(!res.is_error, "clean markdown should have 0 errors: {res:?}");
    let text = match &res.content[0] {
        prod_code_mcp::protocol::McpContentItem::Text { text } => text,
    };
    assert!(text.contains("0 error(s), 0 warning(s)"), "{text}");

    // 2. Malformed Markdown proposal reports frontmatter error
    let broken_md = ws.read("broken.md");
    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_validate_edit",
        json!({
            "path": "src/content/blog/example.md",
            "new_text": broken_md,
        }),
    )
    .await
    .expect("validation returns report on broken markdown");
    assert!(res.is_error, "broken markdown must report error");
    let text = match &res.content[0] {
        prod_code_mcp::protocol::McpContentItem::Text { text } => text,
    };
    assert!(text.contains("unclosed YAML frontmatter"), "{text}");

    // 3. Valid SVG proposal passes with 0 errors (#778)
    let valid_svg = ws.read("proposed.svg");
    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_validate_edit",
        json!({
            "path": "public/img/card.svg",
            "new_text": valid_svg,
        }),
    )
    .await
    .expect("validation succeeds on valid SVG");
    assert!(!res.is_error, "clean SVG should have 0 errors: {res:?}");

    // 4. Malformed SVG proposal reports mismatched tag error
    let broken_svg = ws.read("broken.svg");
    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_validate_edit",
        json!({
            "path": "public/img/card.svg",
            "new_text": broken_svg,
        }),
    )
    .await
    .expect("validation returns report on broken SVG");
    assert!(res.is_error, "broken SVG must report error");
    let text = match &res.content[0] {
        prod_code_mcp::protocol::McpContentItem::Text { text } => text,
    };
    assert!(text.contains("mismatched closing tag"), "{text}");

    // 5. Multi-file validation with Markdown and SVG together (#778 reproduction)
    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_validate_edits",
        json!({
            "edits": [
                { "path": "src/content/blog/example.md", "new_text": valid_md },
                { "path": "public/img/card.svg", "new_text": valid_svg },
            ]
        }),
    )
    .await
    .expect("multi-file markdown + SVG validation succeeds");
    assert!(!res.is_error, "both files clean: {res:?}");
    let text = match &res.content[0] {
        prod_code_mcp::protocol::McpContentItem::Text { text } => text,
    };
    assert!(text.contains("2 file(s) checked together: 0 error(s), 0 warning(s)"), "{text}");

    // 6. CLI invocation reproduction: prod-code validate src/content/blog/example.md --from proposed.md --with public/img/card.svg=proposed.svg --json
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(gw.addr().to_string())
        .args([
            "validate",
            "src/content/blog/example.md",
            "--from",
            "proposed.md",
            "--with",
            "public/img/card.svg=proposed.svg",
            "--json",
        ])
        .env("PROD_CODE_REMOTE", gw.addr().to_string())
        .current_dir(ws.root())
        .output()
        .await
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "CLI validate must exit 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("src/content/blog/example.md"), "{stdout}");
    assert!(stdout.contains("public/img/card.svg"), "{stdout}");

    // Gateway was never queried for parser-backed documentation or SVG
    assert_eq!(gw.calls(), 0, "Markdown and SVG files are validated locally by syntax parsers");
}
