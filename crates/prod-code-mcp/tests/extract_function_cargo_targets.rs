/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

const LIB: &str = "#[path = \"shared.rs\"]\npub mod shared;\n\npub fn calculate() -> i32 {\n    let value = 2 + 3;\n    value\n}\n";
const EXTRACTED: &str = "#[path = \"shared.rs\"]\npub mod shared;\n\npub fn calculate() -> i32 {\n    let value = fun_name();\n    value\n}\n\nfn fun_name() -> i32 {\n    let value = 2 + 3;\n    value\n}\n";
const SHARED: &str = "pub fn same_target() -> i32 {\n    let value = 2 + 3;\n    value\n}\n";
const BIN: &str = "fn main() {\n    let value = 2 + 3;\n    println!(\"{value}\");\n}\n";
const SHARED_BIN: &str = "#[path = \"../shared.rs\"]\nmod shared;\nfn main() {}\n";
const MANIFEST: &str = "[package]\nname = \"target-boundary\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautobins = false\n\n[lib]\npath = \"src/lib.rs\"\n\n[[bin]]\nname = \"runner\"\npath = \"src/bin/runner.rs\"\n";
const BIN_ROOT: &str =
    "mod shared;\nfn main() {\n    let value = 2 + 3;\n    println!(\"{value}\");\n}\n";
const BIN_ROOT_EXTRACTED: &str = "mod shared;\nfn main() {\n    let value = fun_name();\n    println!(\"{value}\");\n}\n\nfn fun_name() -> i32 {\n    let value = 2 + 3;\n    value\n}\n";
const CUSTOM_LIB: &str =
    "pub mod shared;\n\npub fn calculate() -> i32 {\n    let value = 2 + 3;\n    value\n}\n";
const CUSTOM_LIB_EXTRACTED: &str = "pub mod shared;\n\npub fn calculate() -> i32 {\n    let value = fun_name();\n    value\n}\n\nfn fun_name() -> i32 {\n    let value = 2 + 3;\n    value\n}\n";
const SELECTED_ROOT: &str = "#[cfg(feature = \"selected\")]\n#[path = \"selected.rs\"]\npub mod selected;\n#[path = \"common.rs\"]\npub mod common;\n";
const SELECTED_SOURCE: &str =
    "pub fn selected_value() -> i32 {\n    let value = 2 + 3;\n    value\n}\n";
const CANDIDATE_ROOT: &str = "pub mod selected;\n#[cfg(feature = \"candidate\")]\n#[path = \"common.rs\"]\npub mod common;\n";

fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn gateway(
    workspace: &Workspace,
    path: &str,
    source: &str,
    extracted: &str,
) -> ScriptedGateway {
    let selected = workspace.root().join(path);
    let (source, extracted) = (source.to_string(), extracted.to_string());
    ScriptedGateway::start(move |method, _| match method {
        "prodCode/applyAssist" => answers::whole_file(&selected, &source, &extracted),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "prod-code/shadow-run" => json!({ "exit_code": 0 }),
        _ => Value::Null,
    })
    .await
}

async fn extract(
    gateway: &ScriptedGateway,
    workspace: &Workspace,
    path: &str,
    line: u64,
) -> prod_code_mcp::protocol::McpToolCallResult {
    execute_tool(
        gateway.addr(),
        &workspace.root(),
        "code_extract_function",
        json!({
            "path": path,
            "line": line,
            "character": 5,
            "end_line": line,
            "end_character": 23,
            "name": "calculate_value",
            "duplicates": true,
            "other_files": true,
            "apply": true,
        }),
    )
    .await
    .expect("the public extraction operation should return a plan")
}

fn cross_file_copies(workspace: &Workspace, path: &str, source: &str) -> Vec<std::path::PathBuf> {
    let file = workspace.path(path);
    let start = source
        .find("let value = 2 + 3;")
        .expect("selected expression");
    prod_code_mcp::extract_function::duplicates::collect_copies(
        &workspace.root(),
        &file,
        source,
        "let value = 2 + 3;",
        start,
        start + "let value = 2 + 3;".len(),
        true,
        false,
        true,
    )
    .expect("Cargo ownership should be known")
    .into_iter()
    .map(|(path, _, _)| path)
    .collect()
}

#[tokio::test]
async fn other_file_duplicates_stay_within_the_selected_cargo_target() {
    let workspace = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", LIB),
        ("src/shared.rs", SHARED),
        ("src/bin/runner.rs", BIN),
    ]);
    let gateway = gateway(&workspace, "src/lib.rs", LIB, EXTRACTED).await;
    let result = extract(&gateway, &workspace, "src/lib.rs", 5).await;
    let report = text_of(&result);

    assert!(!result.is_error, "{report}");
    assert!(
        workspace
            .read("src/shared.rs")
            .contains("crate::calculate_value()"),
        "report={report}\nshared={}\nbin={}",
        workspace.read("src/shared.rs"),
        workspace.read("src/bin/runner.rs")
    );
    assert_eq!(workspace.read("src/bin/runner.rs"), BIN);
}

#[tokio::test]
async fn a_path_attributed_module_shared_by_targets_is_not_a_duplicate() {
    let workspace = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", LIB),
        ("src/shared.rs", SHARED),
        ("src/bin/runner.rs", SHARED_BIN),
    ]);
    let gateway = gateway(&workspace, "src/lib.rs", LIB, EXTRACTED).await;
    let result = extract(&gateway, &workspace, "src/lib.rs", 5).await;

    assert!(!result.is_error, "{}", text_of(&result));
    assert_eq!(workspace.read("src/shared.rs"), SHARED);
}

#[tokio::test]
async fn explicit_binary_target_uses_its_cargo_root_for_duplicate_calls() {
    let manifest = "[package]\nname = \"binary-boundary\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautobins = false\n\n[[bin]]\nname = \"runner\"\npath = \"src/bin/runner.rs\"\n";
    let workspace = Workspace::new(&[
        ("Cargo.toml", manifest),
        ("src/bin/runner.rs", BIN_ROOT),
        ("src/bin/shared.rs", SHARED),
    ]);
    let gateway = gateway(
        &workspace,
        "src/bin/runner.rs",
        BIN_ROOT,
        BIN_ROOT_EXTRACTED,
    )
    .await;

    let result = extract(&gateway, &workspace, "src/bin/runner.rs", 3).await;
    let report = text_of(&result);

    assert!(!result.is_error, "{report}");
    assert!(
        workspace
            .read("src/bin/shared.rs")
            .contains("crate::calculate_value()"),
        "report={report}\nshared={}",
        workspace.read("src/bin/shared.rs")
    );
}

#[tokio::test]
async fn explicit_library_target_outside_src_preserves_same_target_duplicates() {
    let manifest = "[package]\nname = \"custom-root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"rust/lib.rs\"\n";
    let workspace = Workspace::new(&[
        ("Cargo.toml", manifest),
        ("rust/lib.rs", CUSTOM_LIB),
        ("rust/shared.rs", SHARED),
    ]);
    let gateway = gateway(&workspace, "rust/lib.rs", CUSTOM_LIB, CUSTOM_LIB_EXTRACTED).await;

    let result = extract(&gateway, &workspace, "rust/lib.rs", 4).await;

    assert!(!result.is_error, "{}", text_of(&result));
    assert!(
        workspace
            .read("rust/shared.rs")
            .contains("crate::calculate_value()"),
        "{}",
        workspace.read("rust/shared.rs")
    );
}

#[tokio::test]
async fn cfg_selected_duplicate_includes_unconditional_candidate() {
    let manifest = "[package]\nname = \"cfg-selected\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nselected = []\n";
    let workspace = Workspace::new(&[
        ("Cargo.toml", manifest),
        ("src/lib.rs", SELECTED_ROOT),
        ("src/selected.rs", SELECTED_SOURCE),
        ("src/common.rs", SHARED),
    ]);
    let copies = cross_file_copies(&workspace, "src/selected.rs", SELECTED_SOURCE);

    assert!(
        copies.contains(&workspace.path("src/common.rs")),
        "{copies:?}"
    );
}

#[tokio::test]
async fn unconditional_selected_duplicate_skips_cfg_candidate() {
    let manifest = "[package]\nname = \"cfg-candidate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ncandidate = []\n";
    let workspace = Workspace::new(&[
        ("Cargo.toml", manifest),
        ("src/lib.rs", CANDIDATE_ROOT),
        ("src/selected.rs", SELECTED_SOURCE),
        ("src/common.rs", SHARED),
    ]);
    let copies = cross_file_copies(&workspace, "src/selected.rs", SELECTED_SOURCE);

    assert!(
        !copies.contains(&workspace.path("src/common.rs")),
        "{copies:?}"
    );
}
