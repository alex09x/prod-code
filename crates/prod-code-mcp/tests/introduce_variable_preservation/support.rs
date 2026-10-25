/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::cases::Case;
use prod_code_mcp::introduce_variable::introduce_variable;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::process::Command;
use std::sync::{Arc, Mutex};

fn runtime(source: &str, args: &[&str]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("case.rs");
    let bin = dir.path().join("case");
    std::fs::write(&file, source).unwrap();
    let compiled = Command::new("rustc")
        .args([
            "--edition=2024",
            "-C",
            "overflow-checks=yes",
            "-A",
            "warnings",
            "--error-format=short",
        ])
        .arg(&file)
        .arg("-o")
        .arg(&bin)
        .output()
        .unwrap();
    if !compiled.status.success() {
        return format!(
            "rustc={}\n{}",
            compiled.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&compiled.stderr)
                .replace(&file.to_string_lossy().to_string(), "case.rs")
        );
    }
    let ran = Command::new(bin).args(args).output().unwrap();
    let stderr = String::from_utf8_lossy(&ran.stderr)
        .replace(&file.to_string_lossy().to_string(), "case.rs");
    format!(
        "rustc=0 exit={}\n{}{}",
        ran.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&ran.stdout),
        stderr
    )
}

fn record(text: &str) {
    println!("{text}");
    if let Ok(path) = std::env::var("INTRODUCE_VARIABLE_EVIDENCE") {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(f, "{text}").unwrap();
    }
}

pub async fn exercise(case: Case) {
    let baseline = std::env::var_os("INTRODUCE_VARIABLE_BASELINE").is_some();
    let ws = Workspace::empty();
    let root = ws.root();
    ws.write(
        "Cargo.toml",
        "[package]\nname = \"preservation_fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    );
    let file = ws.write("src/main.rs", &case.source);
    let analyzer = Arc::new(Mutex::new(super::analyzer::Analyzer::start(
        &file,
        &case.source,
    )));
    let connection = Arc::downgrade(&analyzer);
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "rust-analyzer/viewSyntaxTree" | "textDocument/hover" | "textDocument/definition" => {
            connection
                .upgrade()
                .unwrap()
                .lock()
                .unwrap()
                .request(method, params.clone())
        }
        _ => serde_json::Value::Null,
    })
    .await;
    let start = if case.id == "primitive-read" {
        case.source.find("= w").unwrap() + 2
    } else {
        case.source.find(case.expression).unwrap()
    };
    let pos = |at| prod_code_mcp::signature::util::line_col_at(&case.source, at).unwrap();
    let (from, to) = (pos(start), pos(start + case.expression.len()));
    let args: &[&str] = if case.id.contains("shift") {
        &["32"]
    } else {
        &[]
    };
    let before = runtime(&case.source, args);
    assert!(before.starts_with("rustc=0 exit="), "{}: {before}", case.id);
    let plan = introduce_variable(
        gateway.addr(),
        &root,
        &file,
        from,
        to,
        case.name,
        false,
        false,
    )
    .await;
    if !baseline && case.refuse {
        let why = format!("{:#}", plan.expect_err(case.id));
        let expected = if case.id == "escaped-implicit-capture" {
            "macro string escapes"
        } else if case.id == "unknown-macro-capture"
            || case.id == "shadowed-format-macro-capture"
            || case.id == "nested-format-macro-capture"
        {
            "macro-generated references"
        } else if case.id.contains("collision")
            || case.id == "implicit-format-capture"
            || case.id == "raw-implicit-capture"
        {
            "not fresh"
        } else if case.id.contains("4081600661") {
            "ambiguous binding identity"
        } else if case.id.contains("overloaded") {
            "may dispatch user traits"
        } else if case.id.contains("macro") {
            "macro token semantics"
        } else if case.id.contains("generic-mutation") {
            "METHOD_CALL_EXPR"
        } else if case.id.contains("shift") {
            "can panic"
        } else if case.id == "earlier-event" {
            "CALL_EXPR"
        } else if case.id == "earlier-panic" || case.id == "branch-panic-order" {
            "earlier evaluation can panic"
        } else if case.id == "containing-method-mutation" {
            "METHOD_CALL_EXPR"
        } else if case.id == "containing-assignment-mutation" {
            "cannot prove purity of this operator"
        } else {
            "not a unique local record or primitive"
        };
        assert!(
            why.contains(expected),
            "{} refused for the wrong reason: {why}",
            case.id
        );
        for force in [false, true] {
            let err = introduce_variable(
                gateway.addr(),
                &root,
                &file,
                from,
                to,
                case.name,
                true,
                force,
            )
            .await
            .expect_err(case.id);
            assert!(!err.to_string().is_empty());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), case.source);
        }
        record(&format!(
            "{} source:\n{}\noriginal: {before}refused before write (force=false,true): {why}\nunchanged bytes=true",
            case.id, case.source
        ));
        return;
    }
    let plan = plan.expect(case.id);
    assert!(!plan.applied);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), case.source);
    let applied = introduce_variable(
        gateway.addr(),
        &root,
        &file,
        from,
        to,
        case.name,
        true,
        false,
    )
    .await
    .expect(case.id);
    assert!(applied.applied);
    let rewritten = std::fs::read_to_string(&file).unwrap();
    assert_eq!(&plan.rewritten[0].1, &rewritten);
    let after = runtime(&rewritten, args);
    record(&format!(
        "{} source:\n{}\nresult:\n{rewritten}\noriginal: {before}transformed: {after}",
        case.id, case.source
    ));
    if baseline {
        assert_ne!(before, after, "{} must demonstrate the defect", case.id);
    } else {
        assert_eq!(before, after, "{} runtime changed", case.id);
        if case.id.contains("quoted") {
            assert!(rewritten.contains("// (w + 1)\n"));
            assert!(rewritten.contains("/* outer (w + 1) /* nested (w + 1) */ */"));
            assert_eq!(applied.occurrences, 2);
        }
        if case.id == "commented-grouping" {
            assert!(rewritten.contains("(/* keep this */ cached)"));
        }
        if case.id == "primitive-grouping" {
            assert!(!rewritten.contains("(cached)"));
        }
    }
}

pub async fn missing_evidence() {
    for missing in [
        "rust-analyzer/viewSyntaxTree",
        "textDocument/definition",
        "textDocument/hover",
    ] {
        let ws = Workspace::empty();
        ws.write(
            "Cargo.toml",
            "[package]\nname = \"preservation_fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        );
        let source = "fn run(w: u32) {\n    let a = w + 1;\n    let b = w + 1;\n}\n";
        let file = ws.write("src/main.rs", source);
        let analyzer = Arc::new(Mutex::new(super::analyzer::Analyzer::start(&file, source)));
        let connection = Arc::downgrade(&analyzer);
        let gateway = ScriptedGateway::start(move |method, params| {
            if method == missing {
                return serde_json::Value::Null;
            }
            match method {
                "rust-analyzer/viewSyntaxTree"
                | "textDocument/definition"
                | "textDocument/hover" => connection
                    .upgrade()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .request(method, params.clone()),
                "textDocument/diagnostic" => answers::no_diagnostics(),
                _ => serde_json::Value::Null,
            }
        })
        .await;
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let err = introduce_variable(
                gateway.addr(),
                &ws.root(),
                &file,
                (2, 13),
                (2, 18),
                "cached",
                apply,
                force,
            )
            .await
            .expect_err(missing);
            assert!(!err.to_string().is_empty());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
        }
        record(&format!(
            "missing {missing}: explicit refusal, unchanged bytes for plan/apply/forced apply; source:\n{source}"
        ));
    }
}

pub async fn preserve(case: Case) {
    if std::env::var_os("INTRODUCE_VARIABLE_BASELINE").is_none() {
        exercise(case).await;
    }
}
