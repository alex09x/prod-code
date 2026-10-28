/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn only_missing_rmeta_files_from_cargo_check_trigger_a_warmup() {
    let cargo_check = vec!["/usr/bin/cargo".to_string(), "check".to_string()];
    let cargo_toolchain_check = vec![
        "cargo".to_string(),
        "+stable".to_string(),
        "check".to_string(),
    ];
    let cargo_test = vec!["cargo".to_string(), "test".to_string()];
    let missing =
        "error: extern location for serde does not exist: target/debug/deps/libserde-abc.rmeta";
    assert!(needs_dependency_metadata_warmup(&cargo_check, missing));
    assert!(needs_dependency_metadata_warmup(
        &cargo_toolchain_check,
        missing
    ));
    assert!(!needs_dependency_metadata_warmup(&cargo_test, missing));
    assert!(!needs_dependency_metadata_warmup(
        &cargo_check,
        "error: could not write target/debug/deps/libserde.rmeta: No space left on device"
    ));
}

/// #1443: the warm-up runs on a disposable source copy and keeps Cargo output in the workspace
/// target directory that overlay retries expose through their lower layer.
#[test]
fn the_warmup_isolates_build_script_changes_on_success_and_failure() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("persistent-workspace");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"warmup_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn ready() {}\n").unwrap();
    std::fs::write(
        root.join("build.rs"),
        "fn main() {\n    let dir = std::path::PathBuf::from(std::env::var_os(\"CARGO_MANIFEST_DIR\").unwrap());\n    std::fs::write(dir.join(\"build-script-side-effect.txt\"), \"written\").unwrap();\n    if std::env::var_os(\"FAIL_BUILD\").is_some() { panic!(\"controlled build failure\"); }\n}\n",
    )
    .unwrap();

    let command = ["cargo", "check"].map(str::to_string);
    let warmup = warmup_command(&root, None, &command, &[], root.to_str().unwrap()).unwrap();

    let success = std::process::Command::new(&warmup[0])
        .args(&warmup[1..])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    assert!(!root.join("Cargo.lock").exists());
    assert!(!root.join("build-script-side-effect.txt").exists());
    assert!(root.join("target/debug").exists());

    let failing_root = temp.path().join("failing-workspace");
    std::fs::create_dir_all(failing_root.join("src")).unwrap();
    std::fs::write(
        failing_root.join("Cargo.toml"),
        "[package]\nname = \"warmup_failure\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    )
    .unwrap();
    std::fs::write(failing_root.join("src/lib.rs"), "pub fn ready() {}\n").unwrap();
    std::fs::copy(root.join("build.rs"), failing_root.join("build.rs")).unwrap();
    let failing = warmup_command(
        &failing_root,
        None,
        &command,
        &[],
        failing_root.to_str().unwrap(),
    )
    .unwrap();
    let failure = std::process::Command::new(&failing[0])
        .args(&failing[1..])
        .current_dir(&failing_root)
        .env("FAIL_BUILD", "1")
        .output()
        .unwrap();
    assert!(!failure.status.success());
    assert!(!failing_root.join("Cargo.lock").exists());
    assert!(!failing_root.join("build-script-side-effect.txt").exists());

    // A lock at the root still makes the warm-up invocation immutable.
    std::fs::write(root.join("Cargo.lock"), "").unwrap();
    let locked = warmup_command(
        &root,
        Some("src"),
        &["cargo", "check"].map(str::to_string),
        &[],
        root.to_str().unwrap(),
    )
    .unwrap();
    assert!(locked.iter().any(|arg| arg == "--locked"));
}

#[test]
fn warmup_preserves_custom_cargo_toolchain_and_manifest_location() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("gateway-workspace");
    let current = root.join("nested");
    std::fs::create_dir_all(&current).unwrap();
    let manifest = current.join("Cargo.toml");
    let command = [
        "/opt/custom/cargo".to_string(),
        "+nightly".to_string(),
        "check".to_string(),
        "--manifest-path".to_string(),
        manifest.to_string_lossy().into_owned(),
    ];

    let warmup =
        warmup_command(&root, Some("nested"), &command, &[], root.to_str().unwrap()).unwrap();

    assert!(
        warmup.contains(&"/opt/custom/cargo".to_string()),
        "{warmup:?}"
    );
    assert!(warmup.contains(&"+nightly".to_string()), "{warmup:?}");
    assert!(warmup.contains(&"Cargo.toml".to_string()), "{warmup:?}");
    assert!(!warmup[2].contains("locate-project"), "{warmup:?}");
}
