/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Compiler controls run only inside this focused remote integration target. No downloads.
use super::*;
use std::process::{Command, Output};

fn compile(ws: &Workspace, extra: &[&str]) -> Output {
    Command::new("rustc")
        .args([
            "--edition=2021",
            "--crate-type=lib",
            "--error-format=json",
            "src/lib.rs",
            "-o",
            "control.rlib",
        ])
        .args(extra)
        .current_dir(ws.root())
        .output()
        .expect("verified remote Rust compiler")
}

fn error_answer(output: &Output, code: &str) -> Value {
    assert_eq!(
        output.status.code(),
        Some(1),
        "compiler must reject the control"
    );
    let errors: Vec<Value> = String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|d| d["level"] == "error" && d["code"]["code"] == code)
        .collect();
    assert!(
        !errors.is_empty(),
        "missing {code}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let items: Vec<Value> = errors
        .iter()
        .map(|d| {
            let span = d["spans"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["is_primary"] == true)
                .unwrap();
            json!({"severity": 1, "source": "rustc", "code": code, "message": d["message"],
            "range": {"start": {"line": span["line_start"].as_u64().unwrap()-1,
                "character": span["column_start"].as_u64().unwrap()-1},
                "end": {"line": span["line_end"].as_u64().unwrap()-1,
                "character": span["column_end"].as_u64().unwrap()-1}}})
        })
        .collect();
    eprintln!(
        "compiler control: exit=1 code={code} errors={}",
        items.len()
    );
    json!({"kind": "full", "items": items})
}

async fn assert_validation_rejects(text: &str, answer: Value) {
    let ws = workspace("");
    let gw = gateway(&ws, move |text| {
        if text.is_empty() {
            answers::no_diagnostics()
        } else {
            answer.clone()
        }
    })
    .await;
    let report = validate_text(gw.addr(), &ws.root(), &ws.path("src/lib.rs"), text)
        .await
        .unwrap();
    assert!(
        !report.ok(),
        "a genuine compiler error became clean: {report:?}"
    );
    assert!(report.items.iter().any(|d| d.severity == "error"));
    let rendered = report.render();
    assert!(
        rendered.contains("not proof"),
        "limitation context must explain retained authority: {rendered}"
    );
    assert_eq!(ws.read("src/lib.rs"), "");
}

#[tokio::test]
async fn real_send_sync_unpin_errors_keep_their_authority() {
    for (name, text) in [
        (
            "Send",
            "pub fn run() { let x = std::rc::Rc::new(1); std::thread::spawn(move || { drop(x); }); }\n",
        ),
        (
            "Sync",
            "fn require<T: Sync>() {}\npub fn run() { require::<std::rc::Rc<u8>>(); }\n",
        ),
        (
            "Unpin",
            "fn require<T: Unpin>() {}\npub fn run() { require::<std::marker::PhantomPinned>(); }\n",
        ),
    ] {
        let ws = workspace(text);
        let output = compile(&ws, &[]);
        eprintln!("{name} control");
        let answer = error_answer(&output, "E0277");
        assert_validation_rejects(text, answer).await;
    }
}

#[tokio::test]
async fn real_custom_derive_inference_error_keeps_its_authority() {
    let text = "use broken::Broken;\n#[derive(Broken)]\npub struct S;\n";
    let ws = workspace(text);
    ws.write("derive.rs", "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro_derive(Broken)]\npub fn broken(_: TokenStream) -> TokenStream {\n    \"impl S { fn ambiguous() { let _value = None; } }\".parse().unwrap()\n}\n");
    let library = format!("libbroken{}", std::env::consts::DLL_SUFFIX);
    let built = Command::new("rustc")
        .args([
            "--edition=2021",
            "--crate-type=proc-macro",
            "--crate-name=broken",
            "derive.rs",
            "-o",
            &library,
        ])
        .current_dir(ws.root())
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    eprintln!("local proc-macro build: exit=0");
    let output = compile(&ws, &["--extern", &format!("broken={library}")]);
    let answer = error_answer(&output, "E0282");
    assert_eq!(
        answer["items"][0]["range"]["start"]["line"], 1,
        "the compiler attributes the error to the derive"
    );
    assert_validation_rejects(text, answer).await;
}
