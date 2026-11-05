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

/// Writes the fixture's manifest with a `serde` that lives next to it: derive macros named
/// `Serialize` and `Deserialize` that expand to nothing. What the case proves is that the derive
/// attribute, wherever it is written, survives the edit and the program still builds and runs;
/// that needs the names to resolve, not the real crate. The real one would have to be in the
/// machine's crate cache, which a CI runner that builds with Mitts does not have.
fn serde_stand_in(dir: &Path) {
    let macros = dir.join("serde_macros");
    let serde = dir.join("serde");
    for (crate_dir, manifest, source) in [
        (
            &macros,
            "[package]\nname = \"serde_macros\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[lib]\nproc-macro = true\n",
            "use proc_macro::TokenStream;\n\n#[proc_macro_derive(Serialize)]\npub fn serialize(_: TokenStream) -> TokenStream {\n    TokenStream::new()\n}\n\n#[proc_macro_derive(Deserialize)]\npub fn deserialize(_: TokenStream) -> TokenStream {\n    TokenStream::new()\n}\n",
        ),
        (
            &serde,
            "[package]\nname = \"serde\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\nserde_macros = { path = \"../serde_macros\" }\n",
            "pub use serde_macros::{Deserialize, Serialize};\n",
        ),
    ] {
        fs::create_dir_all(crate_dir.join("src")).unwrap();
        fs::write(crate_dir.join("Cargo.toml"), manifest).unwrap();
        fs::write(crate_dir.join("src/lib.rs"), source).unwrap();
    }
    fs::write(
        dir.join("Cargo.toml"),
        format!("{MANIFEST}\n[dependencies]\nserde = {{ path = \"serde\" }}\n"),
    )
    .unwrap();
}

pub fn native(case: &Case, source: &str, label: &str) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), source).unwrap();
    let (compiled, bin, command) = if case.serde {
        serde_stand_in(dir.path());
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
