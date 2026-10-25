/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::native::{
    Fixture,
    bridge::{Answer, Bridge},
};
use anyhow::{Result, ensure};
use prod_code_mcp::introduce_variable::introduce_variable;
use std::{net::SocketAddr, process::Command};

const ORIGINAL: &str = concat!(
    "fn compute(w: u32) -> u32 {\n",
    "    let a = w + 1;\n",
    "    let b = w + 1;\n",
    "    a + b\n",
    "}\n",
    "fn main() {\n",
    "    for input in [0, 7, 100] { println!(\"{}\", compute(input)); }\n",
    "}\n",
);

fn artifact(name: &str, text: &str) {
    if let Some(dir) = std::env::var_os("NATIVE_CONSUMER_EVIDENCE") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }
}

fn runtime(source: &str, label: &str) -> String {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("main.rs");
    let bin = temp.path().join("main");
    std::fs::write(&file, source).unwrap();
    let compiled = Command::new("rustc")
        .args([
            "--edition=2021",
            "-C",
            "overflow-checks=yes",
            "--error-format=short",
        ])
        .arg(&file)
        .arg("-o")
        .arg(&bin)
        .output()
        .unwrap();
    let compile = format!(
        "{label} rustc exit={}\nstdout={}\nstderr={}\n",
        compiled.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    println!("{compile}");
    artifact(&format!("{label}-compiler.txt"), &compile);
    assert!(compiled.status.success(), "{compile}");
    let ran = Command::new(bin).output().unwrap();
    let output = format!(
        "runtime exit={}\nstdout={}\nstderr={}\n",
        ran.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    println!("{label} {output}");
    artifact(&format!("{label}-runtime.txt"), &output);
    assert!(ran.status.success(), "{output}");
    assert_eq!(ran.stdout, b"2\n16\n202\n");
    assert!(ran.stderr.is_empty());
    output
}

async fn consumer(fixture: &Fixture, addr: SocketAddr) -> Result<String> {
    let root = &fixture.workspace.root;
    let file = root.join("src/lib.rs");
    // Same supported expression for positive and negative cases: refusal cannot
    // stand in for the positive consumer compatibility proof.
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        let refused = introduce_variable(addr, root, &file, (2, 13), (2, 18), "w", apply, force)
            .await
            .expect_err("existing parameter must refuse capture");
        println!("COLLISION apply={apply} force={force}: {refused:#}");
        ensure!(
            refused.to_string().contains("not fresh"),
            "wrong refusal: {refused:#}"
        );
        ensure!(
            std::fs::read(&file)? == ORIGINAL.as_bytes(),
            "refusal changed source bytes"
        );
    }
    let preview =
        introduce_variable(addr, root, &file, (2, 13), (2, 18), "cached", false, false).await;
    println!("PUBLIC PREVIEW {preview:#?}");
    let preview = preview?;
    ensure!(
        !preview.applied && preview.occurrences == 2,
        "incorrect preview metadata"
    );
    ensure!(
        preview.diagnostics.is_empty(),
        "preview diagnostics: {:?}",
        preview.diagnostics
    );
    ensure!(
        std::fs::read(&file)? == ORIGINAL.as_bytes(),
        "preview changed disk"
    );
    ensure!(
        preview.rewritten.len() == 1 && preview.rewritten[0].0 == file.to_string_lossy(),
        "incorrect rewritten path"
    );
    let rewritten = &preview.rewritten[0].1;
    artifact("returned.rs", rewritten);
    println!("RETURNED SOURCE\n{rewritten}");
    ensure!(
        rewritten
            == &ORIGINAL
                .replacen(
                    "    let a = w + 1;",
                    "    let cached = w + 1;\n    let a = cached;",
                    1
                )
                .replacen("    let b = w + 1;", "    let b = cached;", 1),
        "unexpected transformation"
    );
    let applied =
        introduce_variable(addr, root, &file, (2, 13), (2, 18), "cached", true, false).await;
    println!("PUBLIC APPLY {applied:#?}");
    let applied = applied?;
    ensure!(
        applied.applied && applied.occurrences == 2,
        "incorrect apply metadata"
    );
    ensure!(
        applied.diagnostics.is_empty(),
        "apply diagnostics: {:?}",
        applied.diagnostics
    );
    ensure!(
        applied.rewritten == preview.rewritten,
        "preview/apply differ"
    );
    ensure!(
        std::fs::read(&file)? == rewritten.as_bytes(),
        "applied bytes differ"
    );
    Ok(rewritten.clone())
}

fn check_answers(answers: &[Answer]) {
    for method in [
        "rust-analyzer/viewSyntaxTree",
        "textDocument/definition",
        "textDocument/hover",
    ] {
        let seen: Vec<_> = answers.iter().filter(|a| a.method == method).collect();
        assert!(!seen.is_empty(), "consumer never requested {method}");
        for answer in seen {
            assert!(
                !answer.validation,
                "planning must use ordinary native sessions"
            );
            assert!(answer.response.get("error").is_none(), "{answer:?}");
            let result = &answer.response["result"];
            assert!(!result.is_null(), "{answer:?}");
            if method.ends_with("viewSyntaxTree") {
                super::check_tree(
                    &serde_json::from_str(result.as_str().unwrap()).unwrap(),
                    ORIGINAL,
                );
            } else if method.ends_with("definition") {
                assert!(!result.as_array().unwrap().is_empty(), "{answer:?}");
            } else {
                assert!(result.to_string().contains("u32"), "{answer:?}");
            }
        }
    }
    assert!(
        answers
            .iter()
            .any(|a| a.validation && a.method == "textDocument/diagnostic")
    );
    for a in answers {
        println!(
            "CONSUMER session={} validation={} method={} request={} response={}",
            a.session, a.validation, a.method, a.request, a.response
        );
    }
}

#[tokio::test]
async fn public_introduce_variable_preserves_native_rust_behavior() {
    artifact("original.rs", ORIGINAL);
    println!("ORIGINAL SOURCE\n{ORIGINAL}");
    let before = runtime(ORIGINAL, "original");
    let fixture = Fixture::new(ORIGINAL, "2021").await;
    let bridge = Bridge::bind().await;
    let addr = bridge.addr;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let client = async {
        let result = consumer(&fixture, addr).await;
        stop.send(true).unwrap();
        result
    };
    let (result, answers) = tokio::join!(client, bridge.run(&fixture, stopped));
    // The transport and all registered sessions have completed before assertions.
    let rewritten = result.expect("supported public introduce-variable must succeed");
    check_answers(&answers);
    assert_eq!(before, runtime(&rewritten, "returned"));
}
