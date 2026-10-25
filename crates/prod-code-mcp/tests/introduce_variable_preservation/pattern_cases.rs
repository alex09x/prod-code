/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::introduce_variable::introduce_variable;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::{
    process::Command,
    sync::{Arc, Mutex},
};

const ORIGINAL: &str = "fn f(n:u32)->u32 { let a=0; let b=match n {0=>1,_=>2}; a+b }\nfn main() { for n in [0,1,2,u32::MAX] { println!(\"{n}:{}\", f(n)); } }\n";
const REUSED: &str = "fn f(n:u32)->u32 { let a=0; let c=0; let b=match n {(0 | 2)=>1,0..=4=>2,_=>3}; a+c+b }\nfn main() { for n in [0,1,2,3,4,5,u32::MAX] { println!(\"{n}:{}\", f(n)); } }\n";

fn record(text: &str) {
    println!("{text}");
    if let Ok(path) = std::env::var("INTRODUCE_VARIABLE_EVIDENCE") {
        use std::io::Write;
        writeln!(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap(),
            "{text}"
        )
        .unwrap();
    }
}

fn runtime(source: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("case.rs");
    let bin = dir.path().join("case");
    std::fs::write(&file, source).unwrap();
    let compiled = Command::new("rustc")
        .args(["--edition=2024", "--error-format=short"])
        .arg(&file)
        .arg("-o")
        .arg(&bin)
        .output()
        .unwrap();
    let stderr =
        String::from_utf8_lossy(&compiled.stderr).replace(file.to_str().unwrap(), "case.rs");
    assert!(compiled.status.success(), "{stderr}");
    let ran = Command::new(bin).output().unwrap();
    assert!(ran.status.success());
    record(&format!(
        "rustc=0 warnings:\n{stderr}runtime exit=0:\n{}",
        String::from_utf8_lossy(&ran.stdout)
    ));
    String::from_utf8(ran.stdout).unwrap()
}

async fn check(source: &str, count: usize, baseline: bool) {
    let ws = Workspace::empty();
    ws.write(
        "Cargo.toml",
        "[package]\nname=\"preservation_fixture\"\nversion=\"0.0.0\"\nedition=\"2024\"\n",
    );
    let file = ws.write("src/main.rs", source);
    let analyzer = Arc::new(Mutex::new(super::analyzer::Analyzer::start(&file, source)));
    let connection = Arc::downgrade(&analyzer);
    // As in the existing preservation groups, diagnostics are not a semantic oracle.
    // Both proposals and actual applied bytes are compiled and executed independently.
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
    let start = source.find("=0").unwrap() + 1;
    let pos = |at| prod_code_mcp::signature::util::line_col_at(source, at).unwrap();
    let (from, to) = (pos(start), pos(start + 1));
    record(&format!("original source:\n{source}"));
    let before = runtime(source);
    for force in [false, true] {
        ws.write("src/main.rs", source);
        let plan = introduce_variable(
            gateway.addr(),
            &ws.root(),
            &file,
            from,
            to,
            "cached",
            false,
            force,
        )
        .await
        .unwrap();
        assert!(!plan.applied);
        assert!(plan.diagnostics.is_empty());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
        let proposed = &plan.rewritten[0].1;
        record(&format!(
            "preview force={force} occurrences={}\n{proposed}",
            plan.occurrences
        ));
        let preview_result = runtime(proposed);
        let applied = introduce_variable(
            gateway.addr(),
            &ws.root(),
            &file,
            from,
            to,
            "cached",
            true,
            force,
        )
        .await
        .unwrap();
        assert!(applied.applied);
        assert!(applied.diagnostics.is_empty());
        let rewritten = std::fs::read_to_string(&file).unwrap();
        assert_eq!(&rewritten, proposed);
        record(&format!("apply force={force} result:\n{rewritten}"));
        let after = runtime(&rewritten);
        assert_eq!(preview_result, after);
        if baseline {
            assert_eq!(plan.occurrences, 2);
            assert!(rewritten.contains("match n {cached=>1,_=>2}"));
            assert!(before.contains("1:2\n"));
            assert!(after.contains("1:1\n"));
            assert_ne!(before, after);
        } else {
            assert_eq!(plan.occurrences, count);
            let pattern = source
                .split("match n ")
                .nth(1)
                .unwrap()
                .split(';')
                .next()
                .unwrap();
            assert!(rewritten.contains(pattern), "pattern bytes changed");
            assert_eq!(before, after);
            assert_eq!(rewritten.matches("=cached;").count(), count);
        }
    }
    if baseline {
        return;
    }
    // Direct pattern selection must fail even with force, before any write.
    ws.write("src/main.rs", source);
    let pattern = source.find("match n ").unwrap();
    let pattern_end = pattern + source[pattern..].find(';').unwrap();
    for (offset, _) in source[pattern..pattern_end].match_indices('0') {
        let start = pattern + offset;
        let (from, to) = (pos(start), pos(start + 1));
        for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
            let err = introduce_variable(
                gateway.addr(),
                &ws.root(),
                &file,
                from,
                to,
                "cached",
                apply,
                force,
            )
            .await
            .unwrap_err();
            assert!(
                err.to_string()
                    .contains("not a complete executable Rust expression"),
                "{err:#}"
            );
            assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
            record(&format!(
                "direct pattern at={start} apply={apply} force={force}: {err}; unchanged bytes=true"
            ));
        }
    }
}

pub async fn preserve() {
    let baseline = std::env::var_os("INTRODUCE_VARIABLE_PATTERN_BASELINE").is_some();
    check(ORIGINAL, 1, baseline).await;
    if !baseline {
        check(REUSED, 2, false).await;
    }
}
