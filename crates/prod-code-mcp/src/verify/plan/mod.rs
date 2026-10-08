/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod basic;
pub mod scope;

use anyhow::{Result, anyhow};

use super::detect::GO_LINT_SCRIPT;
use super::types::{
    CppBuild, JsTestRunner, ProjectTools, PythonRuntime, PythonTestRunner, VerifyKind,
};
pub use basic::{clang_tidy_script, fix_command, plan_command_basic, strs};
pub use scope::{
    cargo_package_name, has_xcode_project, member_dir_named, narrow_scope, plan_xcode_command,
};

/// The command a verification kind maps to for `language` with default tooling
/// (npm, pytest, CMake). Prefer [`plan_command_with`] with detected tools.
pub fn plan_command(language: &str, kind: VerifyKind, filter: Option<&str>) -> Result<Vec<String>> {
    plan_command_with(&ProjectTools::default(), language, kind, filter)
}

/// The command for `kind` in `language` given the checkout's detected tooling.
pub fn plan_command_with(
    tools: &ProjectTools,
    language: &str,
    kind: VerifyKind,
    filter: Option<&str>,
) -> Result<Vec<String>> {
    let filter = filter.filter(|f| !f.is_empty());
    if language == "zig" && kind == VerifyKind::Test && tools.zig_test_step && filter.is_some() {
        return Err(anyhow!(
            "Zig build test steps do not support the generic test-name filter; rerun without a filter"
        ));
    }
    let pm = tools.package_manager;
    let py: Vec<String> = match &tools.python {
        PythonRuntime::Uv => strs(&["uv", "run", "python"]),
        PythonRuntime::Venv(path) => vec![path.clone()],
        PythonRuntime::System => strs(&["python3"]),
    };
    let mut cmd: Vec<String> = match (language, kind) {
        ("java", VerifyKind::Check) if tools.java_gradle => vec![
            if tools.java_gradle_wrapper {
                "./gradlew"
            } else {
                "gradle"
            }
            .to_string(),
            "testClasses".to_string(),
        ],
        ("java", VerifyKind::Test) if tools.java_gradle => {
            let mut command = vec![
                if tools.java_gradle_wrapper {
                    "./gradlew"
                } else {
                    "gradle"
                }
                .to_string(),
                "test".to_string(),
            ];
            if let Some(filter) = filter {
                command.push("--tests".to_string());
                command.push(filter.to_string());
            }
            command
        }
        ("typescript", VerifyKind::Check) => {
            let mut c = strs(&pm.exec());
            c.extend(strs(&["tsc", "--noEmit", "--pretty", "false"]));
            c
        }
        ("typescript", VerifyKind::Lint) => match tools.js_linter {
            Some("eslint") => {
                let mut c = strs(&pm.exec());
                c.extend(strs(&["eslint", ".", "-f", "unix"]));
                c
            }
            Some("biome") => {
                let mut c = strs(&pm.exec());
                c.extend(strs(&["biome", "lint", "."]));
                c
            }
            _ => {
                return Err(anyhow!(
                    "no linter configured (eslint or biome) in this project"
                ));
            }
        },
        ("typescript", VerifyKind::Test) => match tools.js_tests {
            JsTestRunner::Vitest => {
                let mut c = strs(&pm.exec());
                c.extend(strs(&["vitest", "run", "--reporter=verbose"]));
                if let Some(f) = filter {
                    c.extend(strs(&["-t", f]));
                }
                c
            }
            JsTestRunner::Jest => {
                let mut c = strs(&pm.exec());
                c.extend(strs(&["jest", "--ci", "--colors=false", "--verbose"]));
                if let Some(f) = filter {
                    c.extend(strs(&["-t", f]));
                }
                c
            }
            JsTestRunner::BunTest => {
                let mut c = strs(&["bun", "test"]);
                if let Some(f) = filter {
                    c.extend(strs(&["-t", f]));
                }
                c
            }
            JsTestRunner::Mocha => {
                let mut c = strs(&pm.exec());
                c.push("mocha".to_string());
                if let Some(f) = filter {
                    c.extend(strs(&["-g", f]));
                }
                c
            }
            JsTestRunner::Script => {
                let mut c = strs(&pm.run());
                c.push("test".to_string());
                c
            }
        },
        ("python", VerifyKind::Check) => match &tools.python {
            PythonRuntime::Uv => strs(&["uv", "run", "basedpyright", "--outputjson"]),
            PythonRuntime::Venv(path) => {
                strs(&["basedpyright", "--outputjson", "--pythonpath", path])
            }
            PythonRuntime::System => strs(&["basedpyright", "--outputjson"]),
        },
        ("python", VerifyKind::Lint) => {
            let mut c = if tools.python == PythonRuntime::Uv {
                strs(&["uv", "run", "ruff"])
            } else {
                strs(&["ruff"])
            };
            c.extend(strs(&["check", ".", "--output-format", "concise"]));
            c
        }
        ("python", VerifyKind::Test) => {
            let mut c = py.clone();
            match tools.python_tests {
                PythonTestRunner::Pytest => c.extend(strs(&["-m", "pytest", "-q", "-rf"])),
                PythonTestRunner::Unittest => c.extend(strs(&["-m", "unittest", "-v"])),
            }
            if let Some(f) = filter {
                c.extend(strs(&["-k", f]));
            }
            c
        }
        ("zig", VerifyKind::Test) if tools.zig_test_step => strs(&["zig", "build", "test"]),
        ("zig", VerifyKind::Test) => {
            return Err(anyhow!(
                "Zig test planning needs a build.zig test step or an explicit source target"
            ));
        }
        ("go", VerifyKind::Lint) => strs(&["sh", "-c", GO_LINT_SCRIPT, "sh", "./..."]),
        ("cpp", VerifyKind::Lint) => strs(&["sh", "-c", &clang_tidy_script(tools.cpp, false)?]),
        ("cpp", VerifyKind::Check) => match tools.cpp {
            CppBuild::CMake => strs(&[
                "sh",
                "-c",
                "cmake -S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON >/dev/null && cmake --build build",
            ]),
            CppBuild::Meson => strs(&[
                "sh",
                "-c",
                "[ -d build ] || meson setup build >/dev/null; meson compile -C build",
            ]),
            CppBuild::Make => strs(&["make"]),
        },
        ("cpp", VerifyKind::Test) => match tools.cpp {
            CppBuild::CMake => {
                // ctest runs whatever binaries exist: build first so edits are tested.
                let mut script = String::from(
                    "cmake --build build >/dev/null && ctest --test-dir build --output-on-failure",
                );
                if let Some(f) = filter {
                    script.push_str(&format!(" -R '{}'", f.replace('\'', "'\\''")));
                }
                strs(&["sh", "-c", &script])
            }
            CppBuild::Meson => {
                let mut c = strs(&["meson", "test", "-C", "build", "--print-errorlogs"]);
                if let Some(f) = filter {
                    c.push(f.to_string());
                }
                c
            }
            CppBuild::Make => strs(&["make", "test"]),
        },
        _ => plan_command_basic(language, kind, filter)?,
    };
    if cmd.is_empty() {
        cmd = plan_command_basic(language, kind, filter)?;
    }
    Ok(cmd)
}
