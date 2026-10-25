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
fn rejects_memory_limits_outside_contract() {
    for (limit, reason) in [
        (0, "must be at least 64MB"),
        (63, "must be at least 64MB"),
        (16 * 1024 * 1024 + 1, "exceeds 16TB maximum"),
        (u64::MAX, "exceeds 16TB maximum"),
    ] {
        assert_eq!(
            prepare_sandboxed_srv(Path::new("unused"), limit)
                .unwrap_err()
                .to_string(),
            format!("Invalid proc-macro memory limit {limit}MB: {reason}")
        );
    }
}

#[cfg(unix)]
#[test]
fn private_directory_creation_repair_and_refusals() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("nested/farm");
    ensure_secure_farm_dir(&dir).unwrap();
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    ensure_secure_farm_dir(&dir).unwrap();
    ensure_secure_farm_dir(&dir).unwrap();
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let link = temp.path().join("link");
    symlink(&dir, &link).unwrap();
    assert_eq!(
        ensure_secure_farm_dir(&link).unwrap_err().to_string(),
        format!(
            "Security violation: farm directory {} is a symlink",
            link.display()
        )
    );
    let file = temp.path().join("file");
    std::fs::write(&file, "contents").unwrap();
    assert_eq!(
        ensure_secure_farm_dir(&file).unwrap_err().to_string(),
        format!(
            "Security violation: farm directory path {} is not a directory",
            file.display()
        )
    );
    assert_eq!(
        ensure_secure_farm_dir(&file.join("child"))
            .unwrap_err()
            .to_string(),
        "Failed to inspect farm directory metadata"
    );
}

#[cfg(unix)]
#[test]
fn launcher_reuses_valid_file_and_replaces_tampering() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("worker");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s\\n' \"$RUST_ANALYZER_INTERNALS_DO_NOT_USE\" \"$1\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let wrapper = prepare_sandboxed_srv(&executable, 1024).unwrap();
    let expected = std::fs::read_to_string(&wrapper).unwrap();
    assert!(expected.contains("ulimit -v 1048576"));
    assert_eq!(prepare_sandboxed_srv(&executable, 1024).unwrap(), wrapper);
    assert_eq!(
        std::fs::metadata(&wrapper).unwrap().permissions().mode() & 0o777,
        0o700
    );
    std::fs::write(&wrapper, "tampered").unwrap();
    assert_eq!(prepare_sandboxed_srv(&executable, 1024).unwrap(), wrapper);
    assert_eq!(std::fs::read_to_string(&wrapper).unwrap(), expected);
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(prepare_sandboxed_srv(&executable, 1024).unwrap(), wrapper);
    assert_eq!(
        std::fs::metadata(&wrapper).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let output = std::process::Command::new(&wrapper)
        .arg("argument with spaces")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"this is unstable\nargument with spaces\n");
    assert_eq!(output.stderr, b"");
    std::fs::remove_file(wrapper).unwrap();
}

#[cfg(unix)]
#[test]
fn isolated_launcher_refusals() {
    for scenario in ["farm", "scratch", "install"] {
        let temp = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "proc_macro_farm::sandbox::tests::isolated_launcher_child",
                "--nocapture",
            ])
            .env("SANDBOX_REFUSAL_CASE", scenario)
            .env("TMPDIR", temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{scenario}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(unix)]
#[test]
fn isolated_launcher_child() {
    use std::os::unix::fs::symlink;
    let Some(scenario) = std::env::var_os("SANDBOX_REFUSAL_CASE") else {
        return;
    };
    let root = std::env::temp_dir();
    let farm = root.join(format!("prod-code-proc-macro-farm-{}", current_uid()));
    let target = root.join("target");
    std::fs::create_dir(&target).unwrap();
    match scenario.to_str().unwrap() {
        "farm" => {
            symlink(&target, &farm).unwrap();
            let error = prepare_sandboxed_srv(Path::new("unused"), 64).unwrap_err();
            assert_eq!(
                format!("{error:#}"),
                format!(
                    "Failed to ensure secure farm directory {}: Security violation: farm directory {} is a symlink",
                    farm.display(),
                    farm.display()
                )
            );
        }
        "scratch" => {
            ensure_secure_farm_dir(&farm).unwrap();
            let scratch = farm.join("scratch");
            symlink(&target, &scratch).unwrap();
            let error = prepare_sandboxed_srv(Path::new("unused"), 64).unwrap_err();
            assert_eq!(
                format!("{error:#}"),
                format!(
                    "Failed to ensure secure scratch directory {}: Security violation: farm directory {} is a symlink",
                    scratch.display(),
                    scratch.display()
                )
            );
        }
        "install" => {
            let wrapper = prepare_sandboxed_srv(Path::new("unused"), 64).unwrap();
            std::fs::remove_file(&wrapper).unwrap();
            std::fs::create_dir(&wrapper).unwrap();
            let error = prepare_sandboxed_srv(Path::new("unused"), 64).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "Failed to atomically install launcher to {}",
                    wrapper.display()
                )
            );
            assert!(wrapper.is_dir());
            assert_eq!(std::fs::read_dir(&wrapper).unwrap().count(), 0);
        }
        _ => panic!("unknown refusal case"),
    }
}
