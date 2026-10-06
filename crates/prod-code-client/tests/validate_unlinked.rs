/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `prod-code validate` of a proposal rust-analyzer did not check (#467).
//!
//! The analyzer answers a file no crate includes — a new Cargo integration test — with one
//! `unlinked-file` hint and nothing else. The base revision printed `0 error(s)`, or `"errors": 0`
//! with `--json`, and exited 0 for such a proposal, alone or with `--with`. Now it exits 1 with
//! the hint counted as an error that says the file was not checked; a linked proposal still
//! exits 0, and `prod-code diagnostics` of a file on disk still shows the hint as a hint.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;
use std::process::Output;

const CARGO: &str = "[package]\nname = \"unlinked\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str = "pub fn answer() -> u32 {\n    42\n}\n";
const LIB_PROPOSED: &str =
    "pub fn answer() -> u32 {\n    42\n}\n\npub fn twice() -> u32 {\n    answer() * 2\n}\n";
/// The proposal from #467: a new integration test with a type error on its second line.
const PROPOSAL: &str = "#[test]\nfn a_type_error_must_not_validate() {\n    let number: u32 = \"this is not a number\";\n    assert_eq!(number, 3);\n}\n";
const NEW_TEST: &str = "tests/a_type_error_must_not_validate.rs";
const STRAY: &str = "tests/stray/helper.rs";
const STRAY_TEXT: &str = "pub fn helper() -> u32 {\n    1\n}\n";

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

/// The checkout, and the proposals as files outside it.
fn workspace() -> (Workspace, tempfile::TempDir) {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", LIB),
        (STRAY, STRAY_TEXT),
    ]);
    let proposals = tempfile::tempdir().expect("proposals dir");
    std::fs::write(proposals.path().join("proposal.rs"), PROPOSAL).expect("write");
    std::fs::write(proposals.path().join("lib.rs"), LIB_PROPOSED).expect("write");
    (ws, proposals)
}

async fn run(root: &Path, home: &Path, remote: SocketAddr, args: &[&str]) -> Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(remote.to_string())
        .args(args)
        .env("PROD_CODE_REMOTE", remote.to_string())
        .env("HOME", home)
        .current_dir(root)
        .output()
        .await
        .expect("run prod-code")
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// One report of `validate --json`: the `unlinked-file` item is counted and explained, and no
/// type error is claimed.
fn assert_refused(report: &Value, shown: &str) {
    assert_eq!(report["errors"], 1, "{shown}");
    let items = report["items"].as_array().expect("items");
    let item = items
        .iter()
        .find(|d| d["code"] == "unlinked-file")
        .unwrap_or_else(|| panic!("the analyzer's evidence is kept: {shown}"));
    assert_eq!(item["severity"], "error", "{shown}");
    assert!(
        item["note"]
            .as_str()
            .is_some_and(|n| n.contains("not type-checked")),
        "{shown}"
    );
    assert!(
        !shown.contains("E0308") && !shown.contains("mismatched types"),
        "{shown}"
    );
}

#[tokio::test]
async fn validate_json_of_a_new_unlinked_test_exits_nonzero() {
    let (ws, proposals) = workspace();
    let remote = gateway().await;
    let proposal = proposals.path().join("proposal.rs");
    let out = run(
        &ws.root(),
        &ws.root(),
        remote,
        &[
            "validate",
            NEW_TEST,
            "--from",
            proposal.to_str().unwrap(),
            "--json",
        ],
    )
    .await;
    let shown = stdout_of(&out);
    assert_eq!(out.status.code(), Some(1), "validated clean:\n{shown}");
    let report: Value = serde_json::from_str(&shown).expect("one JSON report");
    assert_refused(&report, &shown);

    let text = run(
        &ws.root(),
        &ws.root(),
        remote,
        &["validate", NEW_TEST, "--from", proposal.to_str().unwrap()],
    )
    .await;
    let shown = stdout_of(&text);
    assert_eq!(text.status.code(), Some(1), "{shown}");
    assert!(
        shown.contains("1 error(s)") && shown.contains("[unlinked-file]"),
        "{shown}"
    );
    assert!(!ws.path(NEW_TEST).exists(), "nothing was written");
}

#[tokio::test]
async fn validate_with_an_unlinked_file_exits_nonzero() {
    let (ws, proposals) = workspace();
    let remote = gateway().await;
    let proposal = proposals.path().join("proposal.rs");
    let with = format!("src/lib.rs={}", proposals.path().join("lib.rs").display());
    let out = run(
        &ws.root(),
        &ws.root(),
        remote,
        &[
            "validate",
            NEW_TEST,
            "--from",
            proposal.to_str().unwrap(),
            "--with",
            &with,
            "--json",
        ],
    )
    .await;
    let shown = stdout_of(&out);
    assert_eq!(out.status.code(), Some(1), "validated clean:\n{shown}");
    let reports: Value = serde_json::from_str(&shown).expect("JSON reports");
    let reports = reports.as_array().expect("one report per file");
    assert_eq!(reports.len(), 2, "{shown}");
    assert_refused(&reports[0], &shown);
    assert_eq!(reports[1]["errors"], 0, "{shown}");
    assert_eq!(ws.read("src/lib.rs"), LIB, "nothing was written");
}

/// Control: a linked proposal validates, and read-only diagnostics keep the hint a hint.
#[tokio::test]
async fn a_linked_proposal_and_read_only_diagnostics_exit_zero() {
    let (ws, proposals) = workspace();
    let remote = gateway().await;
    let lib = proposals.path().join("lib.rs");
    let out = run(
        &ws.root(),
        &ws.root(),
        remote,
        &[
            "validate",
            "src/lib.rs",
            "--from",
            lib.to_str().unwrap(),
            "--json",
        ],
    )
    .await;
    let shown = stdout_of(&out);
    assert!(out.status.success(), "{shown}");
    let report: Value = serde_json::from_str(&shown).expect("one JSON report");
    assert_eq!(report["errors"], 0, "{shown}");

    let out = run(
        &ws.root(),
        &ws.root(),
        remote,
        &["diagnostics", STRAY, "--json"],
    )
    .await;
    let shown = stdout_of(&out);
    assert!(out.status.success(), "{shown}");
    let report: Value = serde_json::from_str(&shown).expect("one JSON report");
    assert_eq!(report["errors"], 0, "{shown}");
    assert_eq!(report["items"][0]["code"], "unlinked-file", "{shown}");
    assert_eq!(report["items"][0]["severity"], "hint", "{shown}");
}

/// The CLI against a running gateway and its real rust-analyzer. It runs when
/// `PROD_CODE_LIVE_GATEWAY` holds the address of a running gateway; invoke this ignored
/// integration test explicitly with that prerequisite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires PROD_CODE_LIVE_GATEWAY pointing to a running gateway"]
async fn a_real_rust_analyzer_unlinked_test_fails_the_validate_command() {
    let remote = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .expect("set PROD_CODE_LIVE_GATEWAY to run this integration test")
        .parse::<SocketAddr>()
        .expect("PROD_CODE_LIVE_GATEWAY must be a socket address");
    // Not a dot-directory: some tools pass over hidden ones.
    let dir = tempfile::Builder::new()
        .prefix("unlinked-cli-live-")
        .tempdir()
        .expect("checkout dir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    for (rel, text) in [("Cargo.toml", CARGO), ("src/lib.rs", LIB)] {
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
    // The client keeps its cluster state under HOME; a scratch one leaves the real one alone.
    let home = tempfile::Builder::new()
        .prefix("unlinked-cli-home-")
        .tempdir()
        .expect("home dir");
    let proposals = tempfile::tempdir().expect("proposals dir");
    let proposal = proposals.path().join("proposal.rs");
    let lib = proposals.path().join("lib.rs");
    let broken = proposals.path().join("broken.rs");
    std::fs::write(&proposal, PROPOSAL).expect("write");
    std::fs::write(&lib, LIB_PROPOSED).expect("write");
    std::fs::write(
        &broken,
        "pub fn answer() -> u32 {\n    \"not a number\"\n}\n",
    )
    .expect("write");

    // Until the analyzer has loaded the crate it reports nothing useful; asking is cheap, so it
    // is repeated until the linked type error comes back.
    let mut seen = String::new();
    let mut loaded = false;
    for _ in 0..120 {
        let out = run(
            &root,
            home.path(),
            remote,
            &["validate", "src/lib.rs", "--from", broken.to_str().unwrap()],
        )
        .await;
        seen = format!(
            "{}{}",
            stdout_of(&out),
            String::from_utf8_lossy(&out.stderr)
        );
        if out.status.code() == Some(1) && seen.contains("E0308") {
            loaded = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }
    assert!(
        loaded,
        "the analyzer never reported the linked type error:\n{seen}"
    );
    eprintln!("linked type error:\n{seen}");

    let out = run(
        &root,
        home.path(),
        remote,
        &[
            "validate",
            "src/lib.rs",
            "--from",
            lib.to_str().unwrap(),
            "--json",
        ],
    )
    .await;
    let shown = stdout_of(&out);
    eprintln!("linked clean (exit {:?}):\n{shown}", out.status.code());
    assert!(out.status.success(), "{shown}");

    let out = run(
        &root,
        home.path(),
        remote,
        &[
            "validate",
            NEW_TEST,
            "--from",
            proposal.to_str().unwrap(),
            "--json",
        ],
    )
    .await;
    let shown = stdout_of(&out);
    eprintln!(
        "new integration test (exit {:?}):\n{shown}",
        out.status.code()
    );
    assert_eq!(out.status.code(), Some(1), "validated clean:\n{shown}");
    let report: Value = serde_json::from_str(&shown).expect("one JSON report");
    assert_refused(&report, &shown);

    let with = format!("src/lib.rs={}", lib.display());
    let out = run(
        &root,
        home.path(),
        remote,
        &[
            "validate",
            NEW_TEST,
            "--from",
            proposal.to_str().unwrap(),
            "--with",
            &with,
            "--json",
        ],
    )
    .await;
    let shown = stdout_of(&out);
    eprintln!("together (exit {:?}):\n{shown}", out.status.code());
    assert_eq!(out.status.code(), Some(1), "validated clean:\n{shown}");
    let reports: Value = serde_json::from_str(&shown).expect("JSON reports");
    assert_refused(&reports[0], &shown);
    assert_eq!(reports[1]["errors"], 0, "{shown}");
    assert!(!root.join(NEW_TEST).exists(), "nothing was written");
}
