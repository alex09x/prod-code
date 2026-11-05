/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{cases::Case, support::MANIFEST};
use std::{fs, path::Path, process::Command, sync::LazyLock};

static SERDE_TARGET: LazyLock<tempfile::TempDir> = LazyLock::new(|| tempfile::tempdir().unwrap());

pub fn record(id: &str, name: &str, text: &str) {
    println!("{id}/{name}:\n{text}");
    // Evidence writes require an explicit capture run. Ordinary acceptance never rewrites it.
    if let Some(root) = std::env::var_os("INVERT_VALUE_CAPTURE") {
        let dir = Path::new(&root).join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), text).unwrap();
    }
}

pub fn native(case: &Case, source: &str, label: &str) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), source).unwrap();
    let (compiled, bin, command) = if case.serde {
        fs::write(dir.path().join("Cargo.toml"), format!("{MANIFEST}\n[dependencies]\nserde = {{ version = \"1\", features = [\"derive\"] }}\n")).unwrap();
        let compiled = Command::new("cargo")
            .args(["build", "--offline", "--quiet"])
            .current_dir(dir.path())
            .env("CARGO_TARGET_DIR", SERDE_TARGET.path())
            .output()
            .unwrap();
        (
            compiled,
            SERDE_TARGET.path().join("debug/invert_probe"),
            "cargo build --offline --quiet",
        )
    } else {
        let compiled = Command::new("rustc")
            .current_dir(dir.path())
            .args(["--edition=2024", "-Awarnings", "src/main.rs", "-o", "probe"])
            .output()
            .unwrap();
        (
            compiled,
            dir.path().join("probe"),
            "rustc --edition=2024 -Awarnings src/main.rs -o probe",
        )
    };
    let stderr = String::from_utf8_lossy(&compiled.stderr).replace(
        &dir.path().to_string_lossy().to_string(),
        "<native-fixture>",
    );
    record(
        case.id,
        &format!("{label}.compiler.txt"),
        &format!(
            "NATIVE {command} exit={:?}\n{stderr}",
            compiled.status.code()
        ),
    );
    if !compiled.status.success() {
        return (false, String::new());
    }
    let out = Command::new(bin).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    record(
        case.id,
        &format!("{label}.runtime.txt"),
        &format!(
            "NATIVE fixture binary exit={:?}\nstdout:\n{stdout}\nstderr:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ),
    );
    (out.status.success(), stdout)
}
