/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A directory outline asks each file's own engine (#1415): in a Rust checkout, a loose
//! `scripts` directory with a Python and a TypeScript file opens one session per language,
//! instead of sending both files through the session the directory chose. A directory of one
//! language keeps its one session.

use prod_code_mcp::tools::{OutlineOptions, outline_directory};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::{Arc, Mutex};

async fn outline_log(files: &[(&str, &str)], dir: &str) -> (String, Vec<String>) {
    let mut all = vec![
        (
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub fn f() {}\n"),
    ];
    all.extend_from_slice(files);
    let ws = Workspace::new(&all);
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&log);
    let gateway = ScriptedGateway::start(move |method, params| {
        match method {
            "prod-code/handshake" => seen.lock().unwrap().push(format!(
                "session {}",
                params["preferred_engine"].as_str().unwrap_or("none")
            )),
            "textDocument/documentSymbol" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                let name = uri.rsplit('/').next().unwrap_or("").to_string();
                seen.lock().unwrap().push(name);
                return serde_json::json!([answers::document_symbol("main", 12, 1, 1, 10)]);
            }
            _ => {}
        }
        serde_json::Value::Null
    })
    .await;
    let root = ws.root();
    let text = outline_directory(
        gateway.addr(),
        &root,
        &root.join(dir),
        std::path::Path::new(dir),
        &OutlineOptions::all(usize::MAX, false, "include_locals"),
    )
    .await
    .expect("the directory is outlined");
    let log = log.lock().unwrap().clone();
    (text, log)
}

#[tokio::test]
async fn each_language_of_a_mixed_directory_gets_its_own_session() {
    let (text, log) = outline_log(
        &[
            ("scripts/a.py", "def main():\n    pass\n"),
            ("scripts/b.ts", "export function main() {}\n"),
        ],
        "scripts",
    )
    .await;
    assert_eq!(
        log,
        ["session python", "a.py", "session typescript", "b.ts"],
        "{text}"
    );
    assert!(text.contains("2 file(s) outlined, 0 skipped"), "{text}");
}

#[tokio::test]
async fn a_directory_of_one_language_keeps_one_session() {
    let (text, log) = outline_log(
        &[
            ("scripts/a.py", "def main():\n    pass\n"),
            ("scripts/b.py", "def main():\n    pass\n"),
        ],
        "scripts",
    )
    .await;
    assert_eq!(
        log.iter().filter(|l| l.starts_with("session")).count(),
        1,
        "{log:?}"
    );
    assert_eq!(&log[1..], ["a.py", "b.py"], "{text}");
}
