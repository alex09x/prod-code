/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::verify::plan::{fix_command, plan_command_with};
use crate::verify::types::{CppBuild, ProjectTools, VerifyKind};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

fn program(path: &Path, script: &[u8]) {
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

async fn check_plan(build: CppBuild) {
    for fix in [false, true] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let bin = root.join("bin");
        std::fs::create_dir(&bin).unwrap();
        for name in ["cmake", "meson"] {
            program(
                &bin.join(name),
                b"#!/bin/sh\nmkdir -p build\n: > build/ignored.cpp\n",
            );
        }
        program(
            &bin.join("clang-tidy"),
            br#"#!/bin/sh
printf '%s\000' "$@" > "$TIDY_ARGS"
"#,
        );
        let names = [
            "with space.cpp",
            "single'quote.cc",
            r"back\slash.cxx",
            "normal.c",
        ];
        for name in names {
            std::fs::write(root.join(name), "int sample(void);\n").unwrap();
        }
        let tools = ProjectTools {
            cpp: build,
            ..Default::default()
        };
        let argv = if fix {
            fix_command(&tools, "cpp").unwrap().unwrap()
        } else {
            plan_command_with(&tools, "cpp", VerifyKind::Lint, None).unwrap()
        };
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let output_path = root.join("arguments.bin");
        let error_path = root.join("stderr.txt");
        let mut child = tokio::process::Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(root)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("TIDY_ARGS", &output_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(std::fs::File::create(&error_path).unwrap()))
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        let status = match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
            Ok(status) => status.unwrap(),
            Err(_) => {
                // SAFETY: this process group was created for our owned fixture child.
                unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
                let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
                panic!("clang-tidy fixture command timed out");
            }
        };
        assert!(
            status.success(),
            "fix={fix}: {}",
            std::fs::read_to_string(&error_path).unwrap()
        );
        let bytes = std::fs::read(output_path).unwrap();
        let args: Vec<_> = bytes
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| std::str::from_utf8(part).unwrap().to_string())
            .collect();
        let mut expected = vec!["-p".into(), "build".into(), "--quiet".into()];
        if fix {
            expected.push("-fix".into());
        }
        expected.extend(names.map(|name| format!("./{name}")));
        let mut actual = args;
        actual.sort();
        expected.sort();
        assert_eq!(
            actual, expected,
            "exact arguments and build-directory pruning"
        );
    }
}

#[tokio::test]
async fn clang_tidy_filename_boundaries_cmake() {
    check_plan(CppBuild::CMake).await;
}

#[tokio::test]
async fn clang_tidy_filename_boundaries_meson() {
    check_plan(CppBuild::Meson).await;
}
