/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::Workspace;
use serde_json::{Value, json};
use std::{fs, io::Write, process::Command};

fn execute(language: &str, source: &str) -> Value {
    if language == "swift" {
        return json!({"limitation": "Swift compiler/runtime unavailable on acceptance node"});
    }
    let ws = Workspace::new(&[]);
    let root = ws.root();
    let (mut compiler, file) = if language == "cpp" {
        let mut command = Command::new("c++");
        command.args(["-std=c++17", "pets.cpp", "-o", "pets"]);
        (command, "pets.cpp")
    } else {
        let mut command = Command::new("tsc");
        command.args([
            "--target",
            "es2022",
            "--experimentalDecorators",
            "--pretty",
            "false",
            "pets.ts",
        ]);
        (command, "pets.ts")
    };
    fs::write(root.join(file), source).unwrap();
    let compile = compiler.current_dir(&root).output().unwrap();
    let mut result = json!({
        "compiler_exit": compile.status.code(),
        "compiler_stdout": String::from_utf8_lossy(&compile.stdout).replace(root.to_str().unwrap(), "<fixture>"),
        "compiler_stderr": String::from_utf8_lossy(&compile.stderr).replace(root.to_str().unwrap(), "<fixture>")
    });
    if compile.status.success() {
        let mut runtime = if language == "cpp" {
            Command::new(root.join("pets"))
        } else {
            let mut command = Command::new("node");
            command.arg("pets.js");
            command
        };
        let output = runtime.current_dir(&root).output().unwrap();
        result["runtime_exit"] = json!(output.status.code());
        result["stdout"] = json!(String::from_utf8_lossy(&output.stdout));
        result["stderr"] = json!(
            String::from_utf8_lossy(&output.stderr).replace(root.to_str().unwrap(), "<fixture>")
        );
    }
    result
}

pub fn record(label: &str, language: &str, before: &str, after: &str) {
    let hashes = Command::new("python3")
        .args([
            "-c",
            "import hashlib,json,sys; print(json.dumps([hashlib.sha256(s.encode()).hexdigest() for s in sys.argv[1:]]))",
            before,
            after,
        ])
        .output()
        .unwrap();
    assert!(hashes.status.success());
    let hashes: Vec<String> = serde_json::from_slice(&hashes.stdout).unwrap();
    let original = execute(language, before);
    let transformed = execute(language, after);
    let evidence = json!({
        "label": label, "language": language,
        "before_source": before, "before_sha256": hashes[0], "before_control": original,
        "after_source": after, "after_sha256": hashes[1], "after_control": transformed
    });
    writeln!(std::io::stderr(), "SHARED_CONSUMER_EVIDENCE {evidence}").unwrap();
    if language != "swift" {
        assert_eq!(original["compiler_exit"], 0, "original control: {original}");
        assert_eq!(original["runtime_exit"], 0, "original control: {original}");
        assert_eq!(
            transformed["compiler_exit"], 0,
            "transformed control: {transformed}"
        );
        assert_eq!(
            transformed["runtime_exit"], 0,
            "transformed control: {transformed}"
        );
        assert_eq!(transformed["stdout"], original["stdout"]);
    }
}
