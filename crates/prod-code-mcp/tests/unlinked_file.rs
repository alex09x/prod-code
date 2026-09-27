//! A proposal rust-analyzer did not check is not a proposal it found clean (#467).
//!
//! rust-analyzer answers a file no crate includes — a new Cargo integration test, which is not a
//! target until it exists on disk and the workspace reloads — with a single `unlinked-file` hint:
//! it "can't offer IDE services" there, so it reports no type error, no unresolved name, nothing.
//! The base revision counted that hint as nothing and validated such a proposal with 0 errors
//! and 0 warnings, alone or in a multi-file change, through the library, `code_validate_edit` and
//! `code_validate_edits`. Now a proposal's `unlinked-file` is counted as an error that says what
//! it is (not a type error: an unchecked file), while a linked proposal validates as before and
//! read-only diagnostics still show the hint as a hint.

use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;

const CARGO: &str = "[package]\nname = \"unlinked\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str = "pub fn answer() -> u32 {\n    42\n}\n";
const LIB_PROPOSED: &str =
    "pub fn answer() -> u32 {\n    42\n}\n\npub fn twice() -> u32 {\n    answer() * 2\n}\n";

/// The proposal from #467: a new integration test with a type error on its second line.
const PROPOSAL: &str = "#[test]\nfn a_type_error_must_not_validate() {\n    let number: u32 = \"this is not a number\";\n    assert_eq!(number, 3);\n}\n";
const NEW_TEST: &str = "tests/a_type_error_must_not_validate.rs";
/// A file already on disk that no crate includes.
const STRAY: &str = "tests/stray/helper.rs";
const STRAY_TEXT: &str = "pub fn helper() -> u32 {\n    1\n}\n";

/// rust-analyzer's pull-diagnostics answer for a file no crate includes.
fn unlinked() -> Value {
    json!({ "kind": "full", "items": [ {
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": 0, "character": 0 }
        },
        "severity": 4,
        "code": "unlinked-file",
        "source": "rust-analyzer",
        "message": "This file is not included in any crates, so rust-analyzer can't offer IDE services.\n\nIf you're intentionally working on unowned files, you can silence this warning by adding \"unlinked-file\" to rust-analyzer.diagnostics.disable in your settings."
    } ] })
}

/// A gateway whose analyzer includes `src/` in the crate and nothing under `tests/`.
async fn gateway() -> SocketAddr {
    ScriptedGateway::start(|method, params| match method {
        "textDocument/diagnostic" => {
            let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
            if uri.contains("/tests/") {
                unlinked()
            } else {
                answers::no_diagnostics()
            }
        }
        _ => Value::Null,
    })
    .await
    .addr()
}

fn workspace() -> Workspace {
    Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", LIB),
        (STRAY, STRAY_TEXT),
    ])
}

fn text_of(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The report's `unlinked-file` item is counted and explained, and does not claim a type error
/// the analyzer never observed.
fn assert_refused(report: &prod_code_mcp::diagnostics::DiagnosticsReport, what: &str) {
    let rendered = report.render();
    assert!(
        !report.ok(),
        "{what}: an unchecked file validated clean:\n{rendered}"
    );
    assert_eq!(report.errors, 1, "{what}:\n{rendered}");
    let item = report
        .items
        .iter()
        .find(|d| d.code.as_deref() == Some("unlinked-file"))
        .unwrap_or_else(|| panic!("{what}: the analyzer's evidence is kept:\n{rendered}"));
    assert_eq!(item.severity, "error", "{what}:\n{rendered}");
    assert!(
        item.message.contains("not included in any crates"),
        "{what}: the analyzer's own words: {}",
        item.message
    );
    let note = item.note.as_deref().unwrap_or("");
    assert!(
        note.contains("not type-checked") && note.contains("--all-targets"),
        "{what}: says what was not checked and what the compiler would: {note}"
    );
    assert!(
        !rendered.contains("E0308") && !rendered.contains("mismatched types"),
        "{what}: claims a type error nobody observed:\n{rendered}"
    );
}

#[tokio::test]
async fn a_new_unlinked_test_proposal_does_not_validate() {
    let ws = workspace();
    let remote = gateway().await;
    let file = ws.path(NEW_TEST);
    assert!(!file.exists(), "the proposal is a new file");
    let report = prod_code_mcp::diagnostics::validate_text(remote, &ws.root(), &file, PROPOSAL)
        .await
        .expect("validation runs");
    assert_refused(&report, "a new integration test");
    assert!(
        report
            .render()
            .contains("tests/a_type_error_must_not_validate.rs: 1 error(s)"),
        "{}",
        report.render()
    );
    assert!(!file.exists(), "validation writes nothing");
}

/// A file that is already unlinked on disk was not checked before the edit either; that is not a
/// reason to set its `unlinked-file` aside as something the file already had.
#[tokio::test]
async fn an_unlinked_file_on_disk_is_not_set_aside_as_preexisting() {
    let ws = workspace();
    let remote = gateway().await;
    let report = prod_code_mcp::diagnostics::validate_text(
        remote,
        &ws.root(),
        &ws.path(STRAY),
        "pub fn helper() -> u32 {\n    \"two\"\n}\n",
    )
    .await
    .expect("validation runs");
    assert!(
        report
            .preexisting
            .iter()
            .all(|d| d.code.as_deref() != Some("unlinked-file")),
        "{:?}",
        report.preexisting
    );
    assert_refused(&report, "an edit to a file already unlinked");
    assert_eq!(ws.read(STRAY), STRAY_TEXT);
}

#[tokio::test]
async fn a_linked_proposal_still_validates() {
    let ws = workspace();
    let remote = gateway().await;
    let report = prod_code_mcp::diagnostics::validate_text(
        remote,
        &ws.root(),
        &ws.path("src/lib.rs"),
        LIB_PROPOSED,
    )
    .await
    .expect("validation runs");
    assert!(report.ok(), "{}", report.render());
    assert_eq!((report.errors, report.warnings), (0, 0));
    assert!(report.items.is_empty(), "{:?}", report.items);
}

#[tokio::test]
async fn a_multi_file_proposal_with_an_unlinked_file_does_not_validate() {
    let ws = workspace();
    let remote = gateway().await;
    let root = ws.root();
    let edits = vec![
        (ws.path(NEW_TEST), PROPOSAL.to_string()),
        (ws.path("src/lib.rs"), LIB_PROPOSED.to_string()),
    ];
    let reports = prod_code_mcp::diagnostics::validate_texts(remote, &root, &edits, &[])
        .await
        .expect("validation runs");
    assert_eq!(reports.len(), 2);
    assert_refused(&reports[0], "the new test in a multi-file change");
    assert!(reports[1].ok(), "{}", reports[1].render());
    assert!(
        reports.iter().any(|r| !r.ok()),
        "the change as a whole is not clean"
    );

    // An unchanged file to check against that no crate includes is unchecked just the same.
    let reports = prod_code_mcp::diagnostics::validate_texts(
        remote,
        &root,
        &[(ws.path("src/lib.rs"), LIB_PROPOSED.to_string())],
        &[ws.path(STRAY)],
    )
    .await
    .expect("validation runs");
    assert!(reports[0].ok(), "{}", reports[0].render());
    assert_refused(&reports[1], "an unlinked file checked against");

    // Control: a new module declared by the other proposed file is linked; the change validates.
    let reports = prod_code_mcp::diagnostics::validate_texts(
        remote,
        &root,
        &[
            (
                ws.path("src/lib.rs"),
                format!("pub mod extra;\n{LIB_PROPOSED}"),
            ),
            (ws.path("src/extra.rs"), "pub fn extra() {}\n".to_string()),
        ],
        &[],
    )
    .await
    .expect("validation runs");
    assert!(
        reports.iter().all(|r| r.ok()),
        "{}",
        reports.iter().map(|r| r.render()).collect::<String>()
    );
}

/// Read-only diagnostics of a file on disk are not a validation: the hint stays a hint.
#[tokio::test]
async fn read_only_diagnostics_still_show_the_hint() {
    let ws = workspace();
    let remote = gateway().await;
    let report = prod_code_mcp::diagnostics::diagnostics(remote, &ws.root(), &ws.path(STRAY))
        .await
        .expect("diagnostics run");
    assert!(report.ok(), "{}", report.render());
    assert_eq!(report.items.len(), 1, "{:?}", report.items);
    assert_eq!(report.items[0].severity, "hint");
    assert_eq!(report.items[0].code.as_deref(), Some("unlinked-file"));
}

#[tokio::test]
async fn the_validate_tools_report_an_unlinked_proposal_as_an_error() {
    let ws = workspace();
    let remote = gateway().await;
    let root = ws.root();

    let single = execute_tool(
        remote,
        &root,
        "code_validate_edit",
        json!({ "path": NEW_TEST, "new_text": PROPOSAL }),
    )
    .await
    .expect("code_validate_edit runs");
    let text = text_of(&single);
    assert!(single.is_error, "validated clean:\n{text}");
    assert!(
        text.contains("1 error(s)") && text.contains("[unlinked-file]") && text.contains("note:"),
        "{text}"
    );

    let multi = execute_tool(
        remote,
        &root,
        "code_validate_edits",
        json!({ "edits": [
            { "path": NEW_TEST, "new_text": PROPOSAL },
            { "path": "src/lib.rs", "new_text": LIB_PROPOSED }
        ] }),
    )
    .await
    .expect("code_validate_edits runs");
    let text = text_of(&multi);
    assert!(multi.is_error, "validated clean:\n{text}");
    assert!(
        text.contains("2 file(s) checked together: 1 error(s)"),
        "{text}"
    );

    let linked = execute_tool(
        remote,
        &root,
        "code_validate_edit",
        json!({ "path": "src/lib.rs", "new_text": LIB_PROPOSED }),
    )
    .await
    .expect("code_validate_edit runs");
    assert!(!linked.is_error, "{}", text_of(&linked));

    let read_only = execute_tool(remote, &root, "code_diagnostics", json!({ "path": STRAY }))
        .await
        .expect("code_diagnostics runs");
    let text = text_of(&read_only);
    assert!(!read_only.is_error, "{text}");
    assert!(text.contains("0 error(s)"), "{text}");
    assert!(!ws.path(NEW_TEST).exists(), "nothing was written");
}

/// The same scenario against a running gateway and its real rust-analyzer: a linked proposal
/// with a type error is reported (so the analyzer has loaded the crate), a clean linked one
/// validates, and the new integration test is refused alone, together with another file and
/// through `code_validate_edit`. It runs when `PROD_CODE_LIVE_GATEWAY` holds the address of a
/// running gateway; invoke this ignored integration test explicitly with that prerequisite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires PROD_CODE_LIVE_GATEWAY pointing to a running gateway"]
async fn a_real_rust_analyzer_unlinked_test_proposal_does_not_validate() {
    let remote = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .expect("set PROD_CODE_LIVE_GATEWAY to run this integration test")
        .parse::<SocketAddr>()
        .expect("PROD_CODE_LIVE_GATEWAY must be a socket address");
    // Not a dot-directory: some tools pass over hidden ones.
    let dir = tempfile::Builder::new()
        .prefix("unlinked-live-")
        .tempdir()
        .expect("checkout dir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    for (rel, text) in [
        ("Cargo.toml", CARGO),
        ("src/lib.rs", LIB),
        (STRAY, STRAY_TEXT),
    ] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git runs")
    };
    assert!(git(&["init", "-q"]).success());
    assert!(git(&["add", "-A"]).success());
    git(&[
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "user.name=test",
        "commit",
        "-qm",
        "fixture",
    ]);
    let lib = root.join("src/lib.rs");
    let broken = "pub fn answer() -> u32 {\n    \"not a number\"\n}\n";
    // Until the analyzer has loaded the crate it reports nothing useful; asking is cheap, so it
    // is repeated until the linked type error comes back.
    let mut seen = String::new();
    let mut loaded = false;
    for _ in 0..120 {
        match prod_code_mcp::diagnostics::validate_text(remote, &root, &lib, broken).await {
            Ok(report) => {
                seen = report.render();
                if report
                    .items
                    .iter()
                    .any(|d| d.code.as_deref() == Some("E0308"))
                {
                    loaded = true;
                    break;
                }
            }
            Err(e) => seen = format!("{e:#}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }
    assert!(
        loaded,
        "the analyzer never reported the linked type error:\n{seen}"
    );
    eprintln!("linked type error:\n{seen}");

    let clean = prod_code_mcp::diagnostics::validate_text(remote, &root, &lib, LIB_PROPOSED)
        .await
        .expect("validation runs");
    eprintln!("linked clean:\n{}", clean.render());
    assert!(clean.ok(), "{}", clean.render());
    assert!(
        clean
            .items
            .iter()
            .all(|d| d.code.as_deref() != Some("unlinked-file")),
        "{}",
        clean.render()
    );

    let file = root.join(NEW_TEST);
    let report = prod_code_mcp::diagnostics::validate_text(remote, &root, &file, PROPOSAL)
        .await
        .expect("validation runs");
    eprintln!("new integration test:\n{}", report.render());
    assert_refused(&report, "a real rust-analyzer, new integration test");

    let reports = prod_code_mcp::diagnostics::validate_texts(
        remote,
        &root,
        &[
            (file.clone(), PROPOSAL.to_string()),
            (lib.clone(), LIB_PROPOSED.to_string()),
        ],
        &[],
    )
    .await
    .expect("validation runs");
    for r in &reports {
        eprintln!("together:\n{}", r.render());
    }
    assert_refused(&reports[0], "a real rust-analyzer, multi-file");
    assert!(reports[1].ok(), "{}", reports[1].render());

    let tool = execute_tool(
        remote,
        &root,
        "code_validate_edit",
        json!({ "path": NEW_TEST, "new_text": PROPOSAL }),
    )
    .await
    .expect("code_validate_edit runs");
    eprintln!("code_validate_edit:\n{}", text_of(&tool));
    assert!(tool.is_error, "{}", text_of(&tool));

    let on_disk = prod_code_mcp::diagnostics::diagnostics(remote, &root, &root.join(STRAY))
        .await
        .expect("diagnostics run");
    eprintln!("read-only stray file:\n{}", on_disk.render());
    assert!(on_disk.ok(), "{}", on_disk.render());
    assert!(
        on_disk
            .items
            .iter()
            .any(|d| d.code.as_deref() == Some("unlinked-file") && d.severity == "hint"),
        "{}",
        on_disk.render()
    );
    assert!(!file.exists(), "nothing was written");
}
