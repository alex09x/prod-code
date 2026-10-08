/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::types::{
    CppBuild, JsTestRunner, PackageManager, ProjectTools, PythonRuntime, PythonTestRunner,
};

/// How Go is linted (#400): golangci-lint when the node has it, which reads the project's own
/// config, otherwise `go vet`, saying so on stderr. The packages come as the script's arguments
/// (`./...`), so a `--path` narrows them as it does for `go test`.
pub const GO_LINT_SCRIPT: &str = "if command -v golangci-lint >/dev/null 2>&1; then exec golangci-lint run \"$@\"; fi; echo 'prod-code: golangci-lint is not installed on this node; linting with go vet' >&2; exec go vet \"$@\"";

pub fn any_exists(root: &Path, names: &[&str]) -> bool {
    names.iter().any(|n| root.join(n).exists())
}

pub fn file_contains(root: &Path, name: &str, needle: &str) -> bool {
    std::fs::read_to_string(root.join(name))
        .map(|t| t.contains(needle))
        .unwrap_or(false)
}

fn skip_zig_trivia(bytes: &[u8], mut offset: usize) -> usize {
    loop {
        while bytes
            .get(offset)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            offset += 1;
        }
        if bytes.get(offset..offset.saturating_add(2)) != Some(b"//") {
            return offset;
        }
        while bytes.get(offset).is_some_and(|byte| *byte != b'\n') {
            offset += 1;
        }
    }
}

fn zig_test_step_is_declared(source: &str) -> bool {
    let bytes = source.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes.get(offset..offset.saturating_add(2)) == Some(b"//")
            || bytes.get(offset..offset.saturating_add(2)) == Some(b"\\\\")
        {
            while bytes.get(offset).is_some_and(|byte| *byte != b'\n') {
                offset += 1;
            }
            continue;
        }
        if bytes.get(offset..offset.saturating_add(3)) == Some(b"c\\\\") {
            while bytes.get(offset).is_some_and(|byte| *byte != b'\n') {
                offset += 1;
            }
            continue;
        }
        if bytes[offset] == b'"' {
            offset += 1;
            while offset < bytes.len() {
                if bytes[offset] == b'\\' {
                    offset = (offset + 2).min(bytes.len());
                } else if bytes[offset] == b'"' {
                    offset += 1;
                    break;
                } else {
                    offset += 1;
                }
            }
            continue;
        }
        if bytes[offset] == b'\'' {
            offset += 1;
            while offset < bytes.len() {
                if bytes[offset] == b'\\' {
                    offset = (offset + 2).min(bytes.len());
                } else if bytes[offset] == b'\'' {
                    offset += 1;
                    break;
                } else {
                    offset += 1;
                }
            }
            continue;
        }
        if bytes[offset] != b'.' {
            offset += 1;
            continue;
        }

        let mut cursor = skip_zig_trivia(bytes, offset + 1);
        let step_start = cursor;
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            cursor += 1;
        }
        if &bytes[step_start..cursor] != b"step"
            || bytes
                .get(cursor)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            offset += 1;
            continue;
        }

        cursor = skip_zig_trivia(bytes, cursor);
        if bytes.get(cursor) != Some(&b'(') {
            offset += 1;
            continue;
        }
        cursor = skip_zig_trivia(bytes, cursor + 1);
        if bytes.get(cursor) != Some(&b'"') {
            offset += 1;
            continue;
        }
        let name_start = cursor + 1;
        let Some(name_end) = bytes[name_start..].iter().position(|byte| *byte == b'"') else {
            return false;
        };
        let name_end = name_start + name_end;
        cursor = skip_zig_trivia(bytes, name_end + 1);
        if &bytes[name_start..name_end] == b"test" && bytes.get(cursor) == Some(&b',') {
            return true;
        }
        offset += 1;
    }
    false
}

fn has_zig_test_step(root: &Path) -> bool {
    std::fs::read_to_string(root.join("build.zig"))
        .is_ok_and(|source| zig_test_step_is_declared(&source))
}

/// Detects the tooling of the checkout at `root` from its manifests and lock files.
pub fn detect_tools(root: &Path) -> ProjectTools {
    let mut tools = ProjectTools {
        zig_test_step: has_zig_test_step(root),
        ..ProjectTools::default()
    };
    tools.java_gradle_wrapper = root.join("gradlew").is_file();
    tools.java_gradle = !root.join("pom.xml").exists()
        && [
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
        ]
        .iter()
        .any(|name| root.join(name).exists());

    // JavaScript / TypeScript
    let package_json: serde_json::Value = std::fs::read(root.join("package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null);
    let has_dep = |name: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|k| package_json.get(k).and_then(|d| d.get(name)).is_some())
    };
    let declared_pm = package_json
        .get("packageManager")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    tools.package_manager =
        if any_exists(root, &["bun.lockb", "bun.lock"]) || declared_pm.starts_with("bun") {
            PackageManager::Bun
        } else if root.join("pnpm-lock.yaml").exists() || declared_pm.starts_with("pnpm") {
            PackageManager::Pnpm
        } else if root.join("yarn.lock").exists() || declared_pm.starts_with("yarn") {
            PackageManager::Yarn
        } else {
            PackageManager::Npm
        };
    let test_script = package_json
        .get("scripts")
        .and_then(|s| s.get("test"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    tools.js_tests = if has_dep("vitest")
        || any_exists(
            root,
            &[
                "vitest.config.ts",
                "vitest.config.js",
                "vitest.config.mts",
                "vitest.config.mjs",
            ],
        ) {
        JsTestRunner::Vitest
    } else if has_dep("jest")
        || any_exists(
            root,
            &[
                "jest.config.js",
                "jest.config.ts",
                "jest.config.cjs",
                "jest.config.mjs",
                "jest.config.json",
            ],
        )
    {
        JsTestRunner::Jest
    } else if test_script.starts_with("bun test")
        || (tools.package_manager == PackageManager::Bun && !has_dep("mocha"))
    {
        JsTestRunner::BunTest
    } else if has_dep("mocha") || test_script.starts_with("mocha") {
        JsTestRunner::Mocha
    } else {
        JsTestRunner::Script
    };
    tools.js_linter = if has_dep("eslint")
        || any_exists(
            root,
            &[
                "eslint.config.js",
                "eslint.config.mjs",
                "eslint.config.cjs",
                "eslint.config.ts",
                ".eslintrc",
                ".eslintrc.js",
                ".eslintrc.cjs",
                ".eslintrc.json",
                ".eslintrc.yml",
            ],
        ) {
        Some("eslint")
    } else if has_dep("@biomejs/biome") || any_exists(root, &["biome.json", "biome.jsonc"]) {
        Some("biome")
    } else {
        None
    };

    // Python
    tools.python = if root.join("uv.lock").exists() {
        PythonRuntime::Uv
    } else if root.join(".venv/bin/python").is_file() {
        PythonRuntime::Venv(".venv/bin/python".to_string())
    } else if root.join("venv/bin/python").is_file() {
        PythonRuntime::Venv("venv/bin/python".to_string())
    } else {
        PythonRuntime::System
    };
    let pytest_configured = any_exists(root, &["pytest.ini", "conftest.py"])
        || file_contains(root, "pyproject.toml", "pytest")
        || file_contains(root, "setup.cfg", "[tool:pytest]")
        || file_contains(root, "tox.ini", "[pytest]")
        || file_contains(root, "requirements.txt", "pytest")
        || file_contains(root, "requirements-dev.txt", "pytest");
    tools.python_tests = if pytest_configured {
        PythonTestRunner::Pytest
    } else if tests_import_unittest_only(root) {
        PythonTestRunner::Unittest
    } else {
        PythonTestRunner::Pytest
    };

    // C / C++
    tools.cpp = if root.join("CMakeLists.txt").exists() {
        CppBuild::CMake
    } else if root.join("meson.build").exists() {
        CppBuild::Meson
    } else if any_exists(root, &["Makefile", "makefile", "GNUmakefile"]) {
        CppBuild::Make
    } else {
        CppBuild::CMake
    };

    // Go
    tools.golangci_config = any_exists(
        root,
        &[
            ".golangci.yml",
            ".golangci.yaml",
            ".golangci.toml",
            ".golangci.json",
        ],
    );
    tools
}

/// True when the test files under `tests/` (or `test/`) import `unittest` and none imports
/// `pytest`.
pub fn tests_import_unittest_only(root: &Path) -> bool {
    let mut saw_unittest = false;
    for dir in ["tests", "test"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten().take(200) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("py") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if text.contains("import pytest") || text.contains("from pytest") {
                return false;
            }
            if text.contains("import unittest") || text.contains("from unittest") {
                saw_unittest = true;
            }
        }
    }
    saw_unittest
}
