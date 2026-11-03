/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1811: the mutation tools select a verified sibling worktree with `workspace`, through the
//! public dispatch, and every refused selection or path writes nothing.

use super::super::schemas::{
    WORKSPACE_SELECTABLE_MUTATIONS, advanced_refactoring::advanced_refactoring_tools,
    refactoring::refactoring_tools,
};
use super::{execute_tool, is_read_only_tool, resolve_workspace_root};
use crate::protocol::{McpContentItem, McpToolCallResult};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const TWICE: &str = "pub fn a(x: u32) -> u32 {\n    let y = x * 2;\n    y + 1\n}\n\npub fn b(x: u32) -> u32 {\n    let y = x * 2;\n    y\n}\n";
const TWICE_EXTRACTED: &str = "pub fn a(x: u32) -> u32 {\n    let y = fun_name(x);\n    y + 1\n}\n\nfn fun_name(x: u32) -> u32 {\n    let y = x * 2;\n    y\n}\n\npub fn b(x: u32) -> u32 {\n    let y = x * 2;\n    y\n}\n";

struct Checkouts {
    main: Workspace,
    _parent: tempfile::TempDir,
    sibling: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

/// The configured main checkout and a linked sibling worktree of the same repository.
fn checkouts() -> Checkouts {
    let main = Workspace::new(&[("src/lib.rs", TWICE)]);
    let parent = tempfile::tempdir().unwrap();
    let sibling = parent.path().join("sibling");
    let raw = sibling.to_str().unwrap();
    git(
        &main.root(),
        &["worktree", "add", "-q", "-b", "sibling", raw],
    );
    let sibling = sibling.canonicalize().unwrap();
    Checkouts {
        main,
        _parent: parent,
        sibling,
    }
}

fn text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect()
}

fn extract_args(extra: Value) -> Value {
    let mut args = json!({ "path": "src/lib.rs", "line": 2, "character": 5,
        "end_line": 2, "end_character": 19, "name": "doubled", "apply": true });
    for (key, value) in extra.as_object().unwrap() {
        args[key] = value.clone();
    }
    args
}

async fn rust_gateway(path: &Path) -> ScriptedGateway {
    let assist = answers::whole_file(path, TWICE, TWICE_EXTRACTED);
    ScriptedGateway::start(move |method, _params| match method {
        "prodCode/applyAssist" => assist.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "prod-code/shadow-run" => json!({ "exit_code": 0 }),
        _ => Value::Null,
    })
    .await
}

fn snapshot(files: &[PathBuf]) -> Vec<Option<Vec<u8>>> {
    files.iter().map(|file| std::fs::read(file).ok()).collect()
}

#[tokio::test]
async fn issue_1811_extract_function_writes_only_the_selected_sibling() {
    for absolute in [true, false] {
        let c = checkouts();
        let target = c.sibling.join("src/lib.rs");
        let main_file = c.main.path("src/lib.rs");
        let main_before = std::fs::read(&main_file).unwrap();
        let path = if absolute {
            json!(target)
        } else {
            json!("src/lib.rs")
        };
        let gw = rust_gateway(&target).await;
        let args = extract_args(json!({ "workspace": c.sibling, "path": path }));
        let done = execute_tool(gw.addr(), &c.main.root(), "code_extract_function", args)
            .await
            .unwrap_or_else(|error| panic!("absolute={absolute}: {error:#}"));
        assert!(!done.is_error, "absolute={absolute}: {}", text(&done));
        let now = std::fs::read_to_string(&target).unwrap();
        assert!(now.contains("fn doubled(x: u32) -> u32 {"), "{now}");
        assert_eq!(
            std::fs::read(&main_file).unwrap(),
            main_before,
            "the configured main checkout stays byte-identical"
        );
    }
}

#[tokio::test]
async fn issue_1811_absolute_sibling_path_without_selector_writes_nothing() {
    let c = checkouts();
    let target = c.sibling.join("src/lib.rs");
    let files = [c.main.path("src/lib.rs"), target.clone()];
    let before = snapshot(&files);
    let gw = rust_gateway(&target).await;
    let args = extract_args(json!({ "path": target }));
    let outcome = execute_tool(gw.addr(), &c.main.root(), "code_extract_function", args).await;
    if let Ok(done) = &outcome {
        assert!(done.is_error, "{}", text(done));
    }
    assert_eq!(snapshot(&files), before, "no checkout was written");
}

#[tokio::test]
async fn issue_1811_unrelated_outside_and_escaping_selections_write_nothing() {
    let c = checkouts();
    let unrelated = Workspace::new(&[("src/lib.rs", TWICE)]);
    let nested = c.sibling.join("nested");
    std::fs::create_dir_all(nested.join("src")).unwrap();
    std::fs::write(nested.join("src/lib.rs"), TWICE).unwrap();
    git(&nested, &["init", "-q"]);
    #[cfg(unix)]
    std::os::unix::fs::symlink(c.main.path("src/lib.rs"), c.sibling.join("src/escape.rs")).unwrap();
    let files = [
        c.main.path("src/lib.rs"),
        c.sibling.join("src/lib.rs"),
        nested.join("src/lib.rs"),
        unrelated.path("src/lib.rs"),
    ];
    let before = snapshot(&files);
    let main_file = c.main.path("src/lib.rs");
    let mut cases = vec![
        (
            "code_schema_rename",
            json!({ "workspace": c.sibling, "repos": [".."], "field": "old",
                "to": "new", "apply": true }),
            "cannot combine `workspace` with non-empty `repos`",
        ),
        (
            "code_schema_rename",
            json!({ "workspace": c.sibling, "repos": [c.main.root(), unrelated.root()],
                "field": "old", "to": "new", "apply": true }),
            "cannot combine `workspace` with non-empty `repos`",
        ),
        (
            "code_extract_function",
            extract_args(json!({ "workspace": unrelated.root() })),
            "same Git repository",
        ),
        (
            "code_extract_function",
            extract_args(json!({ "workspace": c.sibling.join("src"), "path": "lib.rs" })),
            "root of a Git worktree",
        ),
        (
            "code_extract_function",
            extract_args(json!({ "workspace": "sibling" })),
            "absolute path",
        ),
        (
            "code_extract_function",
            extract_args(json!({ "workspace": c.sibling, "path": main_file })),
            "outside the selected workspace",
        ),
        (
            "code_extract_function",
            extract_args(json!({ "workspace": c.sibling, "path": "../sibling/src/lib.rs" })),
            "..",
        ),
        (
            "code_extract_function",
            extract_args(json!({ "workspace": c.sibling, "path": "nested/src/lib.rs" })),
            "nested Git",
        ),
        (
            "code_move",
            json!({ "workspace": c.sibling, "path": "src/lib.rs", "line": 1, "character": 8,
                "to": main_file, "apply": true }),
            "outside the selected workspace",
        ),
        (
            "code_check",
            json!({ "workspace": c.sibling, "fix": true }),
            "workspace is supported only by",
        ),
    ];
    #[cfg(unix)]
    cases.push((
        "code_extract_function",
        extract_args(json!({ "workspace": c.sibling, "path": "src/escape.rs" })),
        "outside the selected workspace",
    ));
    let gw = rust_gateway(&c.sibling.join("src/lib.rs")).await;
    for (tool, args, expected) in cases {
        let error = execute_tool(gw.addr(), &c.main.root(), tool, args.clone())
            .await
            .err()
            .unwrap_or_else(|| panic!("{tool} {args} must be refused"));
        let message = format!("{error:#}");
        assert!(message.contains(expected), "{tool} {args}: {message}");
        assert_eq!(snapshot(&files), before, "{tool} {args} wrote a file");
    }
}

#[tokio::test]
async fn issue_1811_workflow_steps_cannot_select_another_workspace() {
    let c = checkouts();
    let before = snapshot(&[c.main.path("src/lib.rs"), c.sibling.join("src/lib.rs")]);
    let plan = json!({ "steps": [{ "id": "x", "tool": "code_outline",
        "args": { "workspace": c.sibling, "path": "src/lib.rs" } }] });
    let outcome = execute_tool(
        "127.0.0.1:1".parse().unwrap(),
        &c.main.root(),
        "code_workflow",
        plan,
    )
    .await;
    assert!(outcome.is_err(), "a step cannot choose its own workspace");
    let after = snapshot(&[c.main.path("src/lib.rs"), c.sibling.join("src/lib.rs")]);
    assert_eq!(after, before);
}

#[test]
fn issue_1811_reads_keep_selection_and_mutations_consume_it() {
    let c = checkouts();
    for tool in ["code_definition", "code_outline", "code_references"] {
        let mut args = json!({ "workspace": c.sibling, "path": "src/lib.rs" });
        let root = resolve_workspace_root(&c.main.root(), tool, &mut args).unwrap();
        assert_eq!(root, c.sibling, "{tool}");
        assert!(is_read_only_tool(tool, &args), "{tool} stays a read");
    }
    let mut args = extract_args(json!({ "workspace": c.sibling }));
    let root = resolve_workspace_root(&c.main.root(), "code_extract_function", &mut args).unwrap();
    assert_eq!(root, c.sibling);
    assert!(args.get("workspace").is_none(), "the selector is consumed");
    for repos in [json!(null), json!([])] {
        let mut args = json!({ "workspace": c.sibling, "repos": repos });
        let root = resolve_workspace_root(&c.main.root(), "code_schema_rename", &mut args)
            .expect("an empty repo list stays within the selected checkout");
        assert_eq!(root, c.sibling);
    }
    assert_eq!(args["path"], "src/lib.rs");
    let mut args = extract_args(json!({}));
    let root = resolve_workspace_root(&c.main.root(), "code_extract_function", &mut args).unwrap();
    assert_eq!(
        root,
        c.main.root(),
        "the configured checkout stays the default"
    );
}

#[test]
fn issue_1811_mutation_catalog_advertises_and_accepts_the_selector() {
    let tools = crate::tools::list_tools();
    let mut catalog: Vec<String> = refactoring_tools()
        .into_iter()
        .chain(advanced_refactoring_tools())
        .map(|tool| tool.name)
        .collect();
    catalog.push("code_assist".into());
    for name in &catalog {
        assert!(
            WORKSPACE_SELECTABLE_MUTATIONS.contains(&name.as_str()),
            "{name} must accept workspace at runtime"
        );
        let tool = tools.iter().find(|tool| &tool.name == name).unwrap();
        assert_eq!(
            tool.input_schema["properties"]["workspace"]["type"], "string",
            "{name} must advertise workspace"
        );
        let args = json!({ "workspace": "/w", "apply": false });
        assert!(!is_read_only_tool(name, &args), "{name} stays a mutation");
    }
    assert_eq!(catalog.len(), WORKSPACE_SELECTABLE_MUTATIONS.len());
}
