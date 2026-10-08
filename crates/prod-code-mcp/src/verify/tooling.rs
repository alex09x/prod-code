/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::Serialize;

/// JavaScript package manager, from the lock file (or `packageManager` in package.json).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl PackageManager {
    /// Runs a binary from the project's dependencies (`npx tsc`, `bunx tsc`, ...).
    pub fn exec(self) -> Vec<&'static str> {
        match self {
            PackageManager::Npm => vec!["npx", "--no-install"],
            PackageManager::Pnpm => vec!["pnpm", "exec"],
            PackageManager::Yarn => vec!["yarn"],
            PackageManager::Bun => vec!["bunx"],
        }
    }

    /// Runs a package.json script.
    pub fn run(self) -> Vec<&'static str> {
        match self {
            PackageManager::Npm => vec!["npm", "run", "--silent"],
            PackageManager::Pnpm => vec!["pnpm", "run", "--silent"],
            PackageManager::Yarn => vec!["yarn", "run"],
            PackageManager::Bun => vec!["bun", "run"],
        }
    }
}

/// JavaScript / TypeScript test runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum JsTestRunner {
    Vitest,
    Jest,
    BunTest,
    Mocha,
    /// `npm test` (or the package manager's equivalent): output is not parsed.
    Script,
}

/// How Python is invoked: `uv run`, the checkout's virtual environment, or the system one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PythonRuntime {
    Uv,
    Venv(String),
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PythonTestRunner {
    Pytest,
    Unittest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CppBuild {
    CMake,
    Meson,
    Make,
}

/// The build, lint and test tooling detected in a checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectTools {
    /// The checkout declares a named Zig test step in its build script.
    #[serde(skip)]
    pub(crate) zig_test_step: bool,
    /// The Java project uses Gradle rather than Maven.
    pub java_gradle: bool,
    /// Whether the Java Gradle project provides its wrapper script.
    pub java_gradle_wrapper: bool,
    pub package_manager: PackageManager,
    pub js_tests: JsTestRunner,
    /// `eslint` or `biome` when configured.
    pub js_linter: Option<&'static str>,
    pub python: PythonRuntime,
    pub python_tests: PythonTestRunner,
    pub cpp: CppBuild,
    /// A golangci-lint config (`.golangci.yml` and the like) at the root: the project expects
    /// golangci-lint, and `lint --fix` runs its fix mode.
    pub golangci_config: bool,
}

impl Default for ProjectTools {
    fn default() -> Self {
        Self {
            zig_test_step: false,
            java_gradle: false,
            java_gradle_wrapper: false,
            package_manager: PackageManager::Npm,
            js_tests: JsTestRunner::Script,
            js_linter: None,
            python: PythonRuntime::System,
            python_tests: PythonTestRunner::Pytest,
            cpp: CppBuild::CMake,
            golangci_config: false,
        }
    }
}
