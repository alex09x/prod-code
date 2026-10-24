/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

#[tokio::test]
async fn fixture_explicit_language_and_absolute_hint_table() {
    for (language, source, fence, snippet) in [
        ("RS", "pub struct Sample;\n", "rust", "let sample = Sample;"),
        (
            "golang",
            "type Sample struct {}\n",
            "go",
            "var sample = Sample{\n\n}",
        ),
        (
            "ts",
            "type Sample = {};\n",
            "typescript",
            "const sample: Sample = {\n\n};",
        ),
        (
            "js",
            "type Sample = {};\n",
            "javascript",
            "const sample: Sample = {\n\n};",
        ),
        (
            "py",
            "class Sample:\n    amount: int\n",
            "python",
            "sample = Sample(\n    amount=0,\n)",
        ),
        (
            "c",
            "struct Sample {};\n",
            "c",
            "auto sample = Sample{\n\n};",
        ),
        (
            "c++",
            "struct Sample {};\n",
            "cpp",
            "auto sample = Sample{\n\n};",
        ),
        (
            "swift",
            "struct Sample {}\n",
            "swift",
            "let sample = Sample(\n\n)",
        ),
        (
            "unknown",
            "pub struct Sample;\n",
            "rust",
            "let sample = Sample;",
        ),
    ] {
        let ws = Workspace::new(&[
            (
                "Cargo.toml",
                "[package]\nname=\"sample\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
            ),
            ("src/sample.rs", source),
        ]);
        let file = ws.path("src/sample.rs");
        let target = file.clone();
        let gateway = ScriptedGateway::start(move |method, _| {
            if method == "workspace/symbol" {
                json!([answers::symbol("Sample", 23, &target, 1, 1)])
            } else {
                Value::Null
            }
        })
        .await;
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
            .arg("-r")
            .arg(gateway.addr().to_string())
            .args([
                "fixture",
                "Sample",
                "--path",
                file.to_str().unwrap(),
                "--no-verify",
                "--lang",
                language,
            ])
            .env("HOME", ws.root())
            .current_dir(ws.root())
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{language}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = format!(
            "fixture for `Sample` (declared in src/sample.rs)\n\n```{fence}\n{snippet}\n```\n\nnot verified (pass `verify: true` to type-check it)\n\n"
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            expected,
            "{language}"
        );
        assert_eq!(ws.read("src/sample.rs"), source);
    }
}
