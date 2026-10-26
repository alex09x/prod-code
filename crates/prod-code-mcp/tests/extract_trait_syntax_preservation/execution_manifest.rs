/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::json;
use std::io::Write;
use std::process::{Command, Stdio};

/// Hash bytes embedded in the executing test binary, rather than assuming that
/// the files currently on disk are the revision Cargo compiled.
pub fn capture() {
    if std::env::var_os("EXTRACT_TRAIT_SYNTAX_CAPTURE").is_none() {
        return;
    }
    macro_rules! sources {
        ($($path:literal),+ $(,)?) => {
            vec![$(($path, include_bytes!($path).as_slice())),+]
        };
    }
    let sources = sources![
        "../../src/extract_trait/delimiters.rs",
        "../../src/extract_trait/execute.rs",
        "../../src/extract_trait/generics.rs",
        "../../src/extract_trait/items.rs",
        "../../src/extract_trait/lex.rs",
        "../../src/extract_trait/mod.rs",
        "../../src/extract_trait/rewrite.rs",
        "../../src/extract_trait/tests.rs",
        "../../src/extract_trait/types.rs",
        "../../src/extract_trait/visibility.rs",
        "../extract_trait_syntax_preservation.rs",
        "conditional_cases.rs",
        "control_cases.rs",
        "execution_manifest.rs",
        "generic_const_cases.rs",
        "recovered_cases.rs",
        "support.rs",
        "visibility_cases.rs",
    ];
    let mut hashes = Vec::new();
    for (path, bytes) in sources {
        let mut child = Command::new("sha256sum")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "sha256sum failed: {out:?}");
        let stdout = String::from_utf8(out.stdout).unwrap();
        hashes.push(json!({"path_relative_to_test_module": path,
            "sha256": stdout.split_whitespace().next().unwrap(),
            "subprocess": "sha256sum (embedded source on stdin)",
            "exit": out.status.code(), "stdout": stdout,
            "stderr": String::from_utf8_lossy(&out.stderr)}));
    }
    super::support::record_evidence(json!({"execution_source_manifest": hashes,
        "basis": "include_bytes embedded in this executing test binary",
        "node_kind": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)}));
}
