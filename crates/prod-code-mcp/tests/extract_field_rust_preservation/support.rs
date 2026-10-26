/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::process::Command;

use super::evidence;
use prod_code_mcp::extract_field::{ExtractedField, extract};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

pub fn position(text: &str, at: usize) -> (u32, u32) {
    prod_code_mcp::signature::position_at(text, at).unwrap()
}

pub fn fixture(text: &str) -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname=\"preservation\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
        ),
        ("src/main.rs", text),
    ])
}

// These are scripted symbol locations, not evidence of a real analyzer's diagnostics.
// Every Store occurrence in these controlled fixtures denotes the same declared type.
pub async fn promote(
    ws: &Workspace,
    selection: &str,
    all: bool,
    apply: bool,
    force: bool,
) -> anyhow::Result<ExtractedField> {
    promote_initialized(ws, selection, "64", all, apply, force).await
}

pub async fn promote_initialized(
    ws: &Workspace,
    selection: &str,
    init: &str,
    all: bool,
    apply: bool,
    force: bool,
) -> anyhow::Result<ExtractedField> {
    let file = ws.path("src/main.rs");
    let source = ws.read("src/main.rs");
    let declaration = position(&source, source.find("struct Store").unwrap() + 7);
    let refs: Vec<_> = source
        .match_indices("Store")
        .skip(1)
        .map(|(at, _)| position(&source, at))
        .collect();
    println!(
        "SCRIPTED definition=src/main.rs:{declaration:?}; references=src/main.rs:{refs:?}; diagnostics=[]"
    );
    let root = ws.root();
    let gateway = ScriptedGateway::start(move |method, request| {
        evidence::reply(
            &root,
            method,
            request,
            match method {
                "textDocument/definition" => answers::locations(&file, &[declaration]),
                "textDocument/references" => answers::locations(&file, &refs),
                "textDocument/diagnostic" => answers::no_diagnostics(),
                _ => serde_json::Value::Null,
            },
        )
    })
    .await;
    let from = source.find(selection).unwrap();
    evidence::operation(
        &ws.root(),
        &ws.path("src/main.rs"),
        from,
        from + selection.len(),
        serde_json::json!({"all": all, "apply": apply, "force": force, "init": init}),
    );
    println!(
        "SCRIPTED selection=src/main.rs:{:?}..{:?}; bytes={from}..{}; all={all}; apply={apply}; force={force}",
        position(&source, from),
        position(&source, from + selection.len()),
        from + selection.len(),
    );
    let result = extract(
        gateway.addr(),
        &ws.root(),
        &ws.path("src/main.rs"),
        position(&source, from),
        position(&source, from + selection.len()),
        "cap",
        Some("usize"),
        Some(init),
        all,
        apply,
        force,
    )
    .await;
    match &result {
        Ok(r) => {
            evidence::result(&ws.root(), r);
            println!(
                "PLAN replaced={} constructors={} blocked={:?} unmatched={:?}",
                r.replaced, r.constructors, r.blocked, r.unmatched
            )
        }
        Err(e) => {
            evidence::emit(serde_json::json!({"kind": "refused", "error": e.to_string()}));
            println!("REFUSED {e}")
        }
    }
    result
}

pub fn execute(root: &Path, label: &str) -> (bool, String) {
    execute_edition(root, label, "2021")
}

pub fn execute_edition(root: &Path, label: &str, edition: &str) -> (bool, String) {
    let source = ["src/main.rs", "src/model.rs", "src/app.rs"]
        .iter()
        .filter_map(|path| {
            std::fs::read_to_string(root.join(path))
                .ok()
                .map(|text| format!("// {path}\n{text}"))
        })
        .collect::<String>();
    let compile = Command::new("rustc")
        .current_dir(root)
        .args([
            &format!("--edition={edition}"),
            "--crate-name",
            "preservation",
            "src/main.rs",
            "-o",
            "program",
        ])
        .output()
        .expect("rustc must execute");
    evidence::native(
        root,
        label,
        "compile",
        &[
            "rustc".into(),
            format!("--edition={edition}"),
            "--crate-name".into(),
            "preservation".into(),
            "src/main.rs".into(),
            "-o".into(),
            "program".into(),
        ],
        &compile,
    );
    let stderr =
        String::from_utf8_lossy(&compile.stderr).replace(&root.display().to_string(), "<fixture>");
    let mut report = format!(
        "=== {label} ===\n{source}\nrustc exit={}\n{stderr}",
        compile.status.code().unwrap_or(-1)
    );
    let mut ok = compile.status.success();
    if ok {
        let run = Command::new(root.join("program")).output().unwrap();
        evidence::native(root, label, "runtime", &["./program".into()], &run);
        report.push_str(&format!(
            "runtime exit={}\nstdout={}\nstderr={}\n",
            run.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        ));
        ok = run.status.success();
    }
    println!("{report}");
    (ok, report)
}

pub fn returned(ws: &Workspace, result: &ExtractedField) {
    for (path, text) in &result.rewritten {
        assert!(Path::new(path).starts_with(ws.root()));
        std::fs::write(path, text).unwrap();
    }
}
