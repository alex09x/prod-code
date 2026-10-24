/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The commands that analyse the checkout in the client process — `dependencies`,
//! `duplicates`, `structural-search`, `propose-expression` — in their `--json` and text
//! forms, and `sync --pull`'s refusal. The binary is spawned in a scratch checkout against a
//! scripted gateway that nothing asks.

use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;
use std::process::Output;

const CARGO: &str = "[package]\nname = \"scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

const LIB: &str = r#"pub fn parse(text: &str) -> u32 {
    text.trim().parse().unwrap()
}

pub fn label() -> String {
    String::new()
}
"#;

const CLONE: &str = r#"pub fn total(items: &[u32]) -> u32 {
    let mut sum = 0;
    for item in items {
        sum += item;
        sum += 1;
    }
    sum
}
"#;

fn workspace() -> Workspace {
    Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", LIB),
        ("src/first.rs", CLONE),
        ("src/second.rs", CLONE),
    ])
}

async fn cli_in(dir: &Path, remote: SocketAddr, args: &[&str]) -> Output {
    let home = tempfile::Builder::new()
        .prefix("dispatchhome")
        .tempdir()
        .expect("home");
    tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(remote.to_string())
        .args(args)
        .env("PROD_CODE_REMOTE", remote.to_string())
        .env("HOME", home.path())
        .current_dir(dir)
        .output()
        .await
        .expect("run prod-code")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn succeeded(out: &Output) -> String {
    assert!(out.status.success(), "{:?}: {}", out.status, stderr(out));
    stdout(out)
}

fn json_of(out: &Output) -> Value {
    serde_json::from_str(&succeeded(out)).expect("the report is JSON")
}

async fn idle_gateway() -> ScriptedGateway {
    ScriptedGateway::start(|_, _| Value::Null).await
}

#[tokio::test]
async fn dependency_reports_name_the_package_in_both_forms() {
    let ws = workspace();
    let gateway = idle_gateway().await;
    let root = ws.root();
    let run = |args: &'static [&'static str]| cli_in(&root, gateway.addr(), args);

    let report = json_of(&run(&["dependencies", "--json"]).await);
    assert_eq!(report["scope"], "crates");
    assert_eq!(report["total_nodes"], 1);
    assert_eq!(report["total_edges"], 0);
    assert_eq!(report["nodes"][0]["name"], "scratch");
    assert_eq!(report["isolated_nodes"], json!(["scratch"]));

    let text = succeeded(&run(&["dependencies"]).await);
    assert!(
        text.contains("Scope: crates | Nodes: 1 | Dependencies: 0"),
        "{text}"
    );
    assert!(
        text.contains("Isolated (Leaf/Orphan) Nodes (1): scratch"),
        "{text}"
    );

    let modules = json_of(
        &run(&[
            "dependencies",
            "--scope",
            "modules",
            "--path",
            "src",
            "--json",
        ])
        .await,
    );
    assert_eq!(modules["scope"], "modules");
    let names: Vec<&str> = modules["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["src::first", "src::lib", "src::second"]);
    assert_eq!(modules["nodes"][1]["path"], "src/lib.rs");

    let text = succeeded(&run(&["dependencies", "--scope", "modules", "--path", "src"]).await);
    assert!(
        text.contains("Scope: modules | Nodes: 3 | Dependencies: 0"),
        "{text}"
    );
    assert!(
        text.contains("Isolated (Leaf/Orphan) Nodes (3): src::first, src::lib, src::second"),
        "{text}"
    );
}

#[tokio::test]
async fn duplicate_reports_find_the_cloned_function_in_both_forms() {
    let ws = workspace();
    let gateway = idle_gateway().await;
    let root = ws.root();
    let run = |args: &'static [&'static str]| cli_in(&root, gateway.addr(), args);
    let clone_types = |report: &Value| -> Vec<String> {
        report["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["clone_type"].as_str().unwrap().to_string())
            .collect()
    };

    let report = json_of(&run(&["duplicates", "--min-lines", "3", "--json"]).await);
    assert_eq!(report["total_files_scanned"], 3);
    assert_eq!(report["total_clone_groups"], 2);
    assert_eq!(clone_types(&report), ["Type-2 (Parameterized)"; 2]);
    let first = &report["groups"][0]["occurrences"];
    let mut files: Vec<&str> = first
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["file"].as_str().unwrap())
        .collect();
    files.sort();
    assert_eq!(files, ["src/first.rs", "src/second.rs"]);

    let text = succeeded(&run(&["duplicates", "--min-lines", "3"]).await);
    assert!(
        text.contains("Files Scanned: 3 | Lines: 23 | Clone Groups: 2"),
        "{text}"
    );
    assert!(
        text.contains("src/first.rs:") && text.contains("src/second.rs:"),
        "{text}"
    );

    let text = succeeded(
        &run(&[
            "duplicates",
            "--min-lines",
            "3",
            "--path",
            "src",
            "--type3",
            "--max-groups",
            "1",
        ])
        .await,
    );
    assert!(text.contains("Clone Groups: 1 |"), "{text}");
    assert!(text.contains("(Type-1 (Exact))"), "{text}");

    let report = json_of(
        &run(&[
            "duplicates",
            "--min-lines",
            "3",
            "--path",
            "src",
            "--json",
            "--type3",
        ])
        .await,
    );
    assert_eq!(clone_types(&report), ["Type-1 (Exact)"; 2]);
}

#[tokio::test]
async fn structural_search_and_expression_proposals_in_both_forms() {
    let ws = workspace();
    let gateway = idle_gateway().await;
    let root = ws.root();
    let abs = ws.path("src/lib.rs").to_string_lossy().into_owned();
    let addr = gateway.addr();
    macro_rules! run {
        ($args:expr) => {
            cli_in(&root, addr, &$args)
        };
    }

    for args in [
        vec!["structural-search", "$a.unwrap()", "--json"],
        vec![
            "structural-search",
            "$a.unwrap()",
            "--path",
            "src",
            "--json",
        ],
    ] {
        let report = json_of(&run!(args).await);
        assert_eq!(report["total_matches"], 1);
        assert_eq!(report["matches"][0]["file"], "src/lib.rs");
        assert_eq!(report["matches"][0]["line"], 2);
        assert_eq!(report["matches"][0]["col"], 17);
        assert_eq!(report["matches"][0]["matched_text"], "parse().unwrap()");
        assert_eq!(report["matches"][0]["bindings"], json!({"a": "parse()"}));
    }
    for args in [
        vec!["structural-search", "$a.unwrap()"],
        vec!["structural-search", "$a.unwrap()", "--path", "src"],
    ] {
        let text = succeeded(&run!(args).await);
        assert!(text.contains("1 match(es) in 1 file(s)"), "{text}");
        assert!(text.contains("src/lib.rs:2:17  parse().unwrap()"), "{text}");
        assert!(text.contains("[$a = parse()]"), "{text}");
    }

    let location = format!("{abs}:6");
    for file in ["src/lib.rs", abs.as_str()] {
        let report =
            json_of(&run!(vec!["propose-expression", file, "6", "String", "--json"]).await);
        assert_eq!(report["target_type"], "String");
        assert_eq!(report["location"], location.as_str());
        assert_eq!(report["candidate_count"], 0);
        let text = succeeded(&run!(vec!["propose-expression", file, "6", "String"]).await);
        assert!(
            text.contains(&format!("Target Type: `String` at {location}")),
            "{text}"
        );
        assert!(
            text.contains("No in-scope expressions found producing target type."),
            "{text}"
        );
    }
}

#[tokio::test]
async fn pulling_with_nothing_to_pull_is_refused() {
    let ws = workspace();
    let gateway = idle_gateway().await;
    let out = cli_in(&ws.root(), gateway.addr(), &["sync", "--pull"]).await;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(
            "Error: --pull requires at least one file or path to pull (e.g. `prod-code sync --pull path/to/file.rs` or `prod-code pull <files...>`); to push current changes omit --pull"
        ),
        "{}",
        stderr(&out)
    );
}
