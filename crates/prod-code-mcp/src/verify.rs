//! Typed verification commands (Phase 6.4): `check`, `lint` and `test` run through remote exec
//! with machine-readable output where the toolchain offers it, parsed into structured
//! diagnostics and test failures for terminals and agents.

use crate::exec::{TailBuffer, run_remote};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyKind {
    Check,
    Lint,
    Test,
    Bench,
}

impl VerifyKind {
    pub fn label(&self) -> &'static str {
        match self {
            VerifyKind::Check => "check",
            VerifyKind::Lint => "lint",
            VerifyKind::Test => "test",
            VerifyKind::Bench => "bench",
        }
    }
}

/// One benchmark's result: its estimate and, when the harness gives one, the range around it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchResult {
    pub name: String,
    /// `2.3500 ns`, `1234 ns/iter`, `1234 ns/op`: as the harness prints it.
    pub estimate: String,
    /// `2.3499 ns .. 2.3502 ns` (criterion's interval) or `+/- 56` (libtest).
    pub range: Option<String>,
}

/// The benchmark results in a run's output: criterion's `name time: [low estimate high]` (the
/// name on the line before when it is long), libtest's `test name ... bench: N ns/iter (+/- M)`
/// and Go's `BenchmarkName-8  N  T ns/op`.
pub fn parse_bench_text(text: &str) -> Vec<BenchResult> {
    let mut out = Vec::new();
    let mut previous = "";
    for line in text.lines() {
        let t = line.trim();
        if let Some(at) = t.find("time:") {
            let inner = t[at + 5..]
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']');
            let parts: Vec<&str> = inner.split_whitespace().collect();
            let name = t[..at].trim();
            let name = if name.is_empty() { previous } else { name };
            if parts.len() == 6 && !name.is_empty() {
                out.push(BenchResult {
                    name: name.to_string(),
                    estimate: format!("{} {}", parts[2], parts[3]),
                    range: Some(format!(
                        "{} {} .. {} {}",
                        parts[0], parts[1], parts[4], parts[5]
                    )),
                });
            }
        } else if let Some(rest) = t.strip_prefix("test ")
            && let Some((name, result)) = rest.split_once(" ... bench:")
        {
            let result = result.trim();
            let (estimate, range) = match result.split_once('(') {
                Some((e, r)) => (e.trim(), Some(r.trim_end_matches(')').trim().to_string())),
                None => (result, None),
            };
            out.push(BenchResult {
                name: name.trim().to_string(),
                estimate: estimate.replace(',', ""),
                range,
            });
        } else if t.starts_with("Benchmark") {
            let words: Vec<&str> = t.split_whitespace().collect();
            if words.len() >= 4 && words[3].ends_with("/op") {
                out.push(BenchResult {
                    name: words[0].to_string(),
                    estimate: format!("{} {}", words[2], words[3]),
                    range: None,
                });
            }
        }
        if !t.is_empty() && !t.starts_with("Benchmarking") {
            previous = t;
        }
    }
    out
}

/// One compiler or linter finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub level: String,
    pub code: Option<String>,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u64>,
    pub column: Option<u64>,
}

impl Diagnostic {
    pub fn render(&self) -> String {
        let at = match (&self.file, self.line, self.column) {
            (Some(f), Some(l), Some(c)) => format!("{f}:{l}:{c}"),
            (Some(f), Some(l), None) => format!("{f}:{l}"),
            (Some(f), _, _) => f.clone(),
            _ => String::new(),
        };
        let code = self
            .code
            .as_ref()
            .map(|c| format!("[{c}] "))
            .unwrap_or_default();
        if at.is_empty() {
            format!("{}: {code}{}", self.level, self.message)
        } else {
            format!("{}: {code}{} ({at})", self.level, self.message)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestFailure {
    pub name: String,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    pub kind: VerifyKind,
    pub language: String,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub diagnostics: Vec<Diagnostic>,
    pub tests_passed: u64,
    pub tests_failed: u64,
    pub failures: Vec<TestFailure>,
    /// Tail of the raw combined output, for anything the parsers did not understand.
    pub tail: String,
    /// The compiler's machine-applicable fixes (Rust check and lint), for `fix: true`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<crate::fixit::Fix>,
    /// Benchmark results, for a `bench` run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub benches: Vec<BenchResult>,
    /// CPU time and peak memory of the command and its children, when the node could tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<prod_code_protocol::ExecUsage>,
    /// The OS and architecture the run was on (`linux x86_64`): a diagnostic or a fix is for
    /// that platform (#140).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }

    pub fn errors(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.level == "error")
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.level == "warning")
            .count()
    }

    pub fn summary(&self) -> String {
        let status = match (self.timed_out, self.exit_code) {
            (true, _) => "TIMED OUT".to_string(),
            (false, Some(0)) => "OK".to_string(),
            (false, Some(code)) => format!("FAILED (exit {code})"),
            (false, None) => "KILLED".to_string(),
        };
        let mut parts = vec![format!(
            "{} {}: {status} in {:.1}s{}",
            self.language,
            self.kind.label(),
            self.duration_ms as f64 / 1000.0,
            self.platform
                .as_deref()
                .map(|p| format!(" on {p}"))
                .unwrap_or_default()
        )];
        if self.kind == VerifyKind::Test {
            parts.push(format!(
                "{} passed, {} failed",
                self.tests_passed, self.tests_failed
            ));
        }
        if self.kind == VerifyKind::Bench {
            parts.push(format!("{} benchmark(s)", self.benches.len()));
        }
        if !self.diagnostics.is_empty() {
            parts.push(format!(
                "{} error(s), {} warning(s)",
                self.errors(),
                self.warnings()
            ));
        }
        if let Some(usage) = &self.usage {
            parts.push(usage.render());
        }
        parts.join("; ")
    }

    /// Human/agent readable report: summary, diagnostics, test failures, then the raw tail
    /// only when nothing structured explains a failure.
    pub fn render(&self, max_items: usize) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "$ {}\n{}\n",
            self.command.join(" "),
            self.summary()
        ));
        for d in self.diagnostics.iter().take(max_items) {
            out.push_str("  ");
            out.push_str(&d.render());
            out.push('\n');
        }
        if self.diagnostics.len() > max_items {
            out.push_str(&format!(
                "  ... {} more diagnostic(s)\n",
                self.diagnostics.len() - max_items
            ));
        }
        for b in self.benches.iter().take(max_items) {
            out.push_str(&format!("  {}  {}", b.name, b.estimate));
            if let Some(range) = &b.range {
                out.push_str(&format!("  [{range}]"));
            }
            out.push('\n');
        }
        for f in self.failures.iter().take(max_items) {
            out.push_str(&format!(
                "--- FAILED {} ---\n{}\n",
                f.name,
                f.output.trim_end()
            ));
        }
        if self.failures.len() > max_items {
            out.push_str(&format!(
                "... {} more failed test(s)\n",
                self.failures.len() - max_items
            ));
        }
        if !self.ok() && self.diagnostics.is_empty() && self.failures.is_empty() {
            out.push_str("--- output tail ---\n");
            out.push_str(self.tail.trim_end());
            out.push('\n');
        }
        out
    }
}

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
    pub package_manager: PackageManager,
    pub js_tests: JsTestRunner,
    /// `eslint` or `biome` when configured.
    pub js_linter: Option<&'static str>,
    pub python: PythonRuntime,
    pub python_tests: PythonTestRunner,
    pub cpp: CppBuild,
}

impl Default for ProjectTools {
    fn default() -> Self {
        Self {
            package_manager: PackageManager::Npm,
            js_tests: JsTestRunner::Script,
            js_linter: None,
            python: PythonRuntime::System,
            python_tests: PythonTestRunner::Pytest,
            cpp: CppBuild::CMake,
        }
    }
}

fn any_exists(root: &Path, names: &[&str]) -> bool {
    names.iter().any(|n| root.join(n).exists())
}

fn file_contains(root: &Path, name: &str, needle: &str) -> bool {
    std::fs::read_to_string(root.join(name))
        .map(|t| t.contains(needle))
        .unwrap_or(false)
}

/// Detects the tooling of the checkout at `root` from its manifests and lock files.
pub fn detect_tools(root: &Path) -> ProjectTools {
    let mut tools = ProjectTools::default();

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
    tools
}

/// True when the test files under `tests/` (or `test/`) import `unittest` and none imports
/// `pytest`.
fn tests_import_unittest_only(root: &Path) -> bool {
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

/// The command a verification kind maps to for `language` with default tooling
/// (npm, pytest, CMake). Prefer [`plan_command_with`] with detected tools.
pub fn plan_command(language: &str, kind: VerifyKind, filter: Option<&str>) -> Result<Vec<String>> {
    plan_command_with(&ProjectTools::default(), language, kind, filter)
}

fn strs(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// The shell script that runs clang-tidy over a C/C++ project's sources with the build's
/// compilation database (configured first), with `-fix` when `fix`. A Make project has no
/// database to give it, and is refused.
fn clang_tidy_script(build: CppBuild, fix: bool) -> Result<String> {
    let configure = match build {
        CppBuild::CMake => "cmake -S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON >/dev/null",
        CppBuild::Meson => "{ [ -d build ] || meson setup build >/dev/null; }",
        CppBuild::Make => {
            return Err(anyhow!(
                "clang-tidy needs a compilation database, which a Make build does not write; \
                 use CMake or Meson, or generate one with bear"
            ));
        }
    };
    Ok(format!(
        "{configure} && find . -path ./build -prune -o \\( -name '*.c' -o -name '*.cc' -o -name '*.cpp' -o -name '*.cxx' \\) -print | xargs -r clang-tidy -p build --quiet{}",
        if fix { " -fix" } else { "" }
    ))
}

/// The command that applies a linter's own fixes in place, for `lint --fix` on a language
/// whose linter has a fix mode: `ruff check --fix`, `eslint --fix`, `biome lint --write`,
/// `clang-tidy -fix`. `None` when it has none (`go vet`), or for Rust, whose fixes are read
/// from the compiler's JSON instead.
pub fn fix_command(tools: &ProjectTools, language: &str) -> Result<Option<Vec<String>>> {
    let pm = tools.package_manager;
    Ok(Some(match language {
        "python" => {
            let mut c = if tools.python == PythonRuntime::Uv {
                strs(&["uv", "run", "ruff"])
            } else {
                strs(&["ruff"])
            };
            c.extend(strs(&["check", ".", "--fix", "--output-format", "concise"]));
            c
        }
        "typescript" => match tools.js_linter {
            Some("eslint") => {
                let mut c = strs(&pm.exec());
                c.extend(strs(&["eslint", ".", "--fix", "-f", "unix"]));
                c
            }
            Some("biome") => {
                let mut c = strs(&pm.exec());
                c.extend(strs(&["biome", "lint", "--write", "."]));
                c
            }
            _ => return Ok(None),
        },
        "cpp" => strs(&["sh", "-c", &clang_tidy_script(tools.cpp, true)?]),
        _ => return Ok(None),
    }))
}

/// The command for `kind` in `language` given the checkout's detected tooling.
pub fn plan_command_with(
    tools: &ProjectTools,
    language: &str,
    kind: VerifyKind,
    filter: Option<&str>,
) -> Result<Vec<String>> {
    let filter = filter.filter(|f| !f.is_empty());
    let pm = tools.package_manager;
    let py: Vec<String> = match &tools.python {
        PythonRuntime::Uv => strs(&["uv", "run", "python"]),
        PythonRuntime::Venv(path) => vec![path.clone()],
        PythonRuntime::System => strs(&["python3"]),
    };
    let mut cmd: Vec<String> = match (language, kind) {
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

/// A `path` inside the project narrows the command to that part: `cargo … -p <crate>` for
/// the Cargo package containing it, `./dir/...` for a Go package tree, the directory or file
/// for pytest. Anything else keeps the whole-project command.
pub fn narrow_scope(command: &mut Vec<String>, language: &str, project_dir: &Path, hint: &Path) {
    let canon_dir =
        std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    // A hint that is not a path may be a crate / package name ("prod-code-gateway").
    let hint_owned;
    let hint = if hint.exists() {
        hint
    } else if let Some(dir) = member_dir_named(&canon_dir, hint) {
        hint_owned = dir;
        &hint_owned
    } else {
        return;
    };
    let target = std::fs::canonicalize(hint).unwrap_or_else(|_| hint.to_path_buf());
    let dir = if target.is_file() {
        target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| target.clone())
    } else {
        target.clone()
    };
    if !dir.starts_with(&canon_dir) || dir == canon_dir {
        return;
    }
    let rel_of = |p: &Path| -> String {
        p.strip_prefix(&canon_dir)
            .map(|r| {
                r.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default()
    };
    match language {
        "rust" => {
            let mut probe = dir.clone();
            while probe.starts_with(&canon_dir) && probe != canon_dir {
                if let Some(name) = cargo_package_name(&probe.join("Cargo.toml")) {
                    if let Some(i) = command.iter().position(|a| a == "--workspace") {
                        command.splice(i..=i, ["-p".to_string(), name]);
                    }
                    return;
                }
                match probe.parent() {
                    Some(parent) => probe = parent.to_path_buf(),
                    None => break,
                }
            }
        }
        "go" => {
            if let Some(i) = command.iter().position(|a| a == "./...") {
                command[i] = format!("./{}/...", rel_of(&dir));
            }
        }
        "python" if command.iter().any(|a| a == "pytest") => {
            command.push(rel_of(&target));
        }
        _ => {}
    }
}

/// The directory of a workspace member whose Cargo package name or directory name is the
/// last component of `hint` (two levels deep: `crates/x`, `x`).
fn member_dir_named(root: &Path, hint: &Path) -> Option<PathBuf> {
    let wanted = hint.file_name()?.to_string_lossy().into_owned();
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        candidates.push(path.clone());
        if let Ok(children) = std::fs::read_dir(&path) {
            candidates.extend(children.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
    }
    candidates.into_iter().find(|dir| {
        dir.file_name()
            .map(|n| n.to_string_lossy() == wanted)
            .unwrap_or(false)
            || cargo_package_name(&dir.join("Cargo.toml")).as_deref() == Some(wanted.as_str())
    })
}

/// `[package] name` of a Cargo manifest (None for a workspace-only or missing manifest).
fn cargo_package_name(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package
            && let Some(rest) = line.strip_prefix("name")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// Commands that do not depend on detected tooling (Rust, Go, Swift packages).
fn plan_command_basic(
    language: &str,
    kind: VerifyKind,
    filter: Option<&str>,
) -> Result<Vec<String>> {
    let mut cmd: Vec<String> = match (language, kind) {
        ("rust", VerifyKind::Check) => vec![
            "cargo",
            "check",
            "--workspace",
            "--all-targets",
            "--message-format=json",
        ],
        ("rust", VerifyKind::Lint) => vec![
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--message-format=json",
            "--",
            "-D",
            "warnings",
        ],
        ("rust", VerifyKind::Test) => vec!["cargo", "test", "--workspace"],
        ("rust", VerifyKind::Bench) => vec!["cargo", "bench", "--workspace"],
        ("go", VerifyKind::Bench) => vec!["go", "test", "-run", "^$", "-bench"],
        ("go", VerifyKind::Check) => vec!["go", "build", "./..."],
        ("go", VerifyKind::Lint) => vec!["go", "vet", "./..."],
        ("go", VerifyKind::Test) => vec!["go", "test", "-json", "./..."],
        ("swift", VerifyKind::Check) => vec!["swift", "build"],
        ("swift", VerifyKind::Test) => vec!["swift", "test"],
        _ => {
            return Err(anyhow!(
                "no {} command for language {language}",
                kind.label()
            ));
        }
    }
    .into_iter()
    .map(str::to_string)
    .collect();
    if let Some(filter) = filter.filter(|f| !f.is_empty()) {
        match (language, kind) {
            ("rust", VerifyKind::Test) => cmd.push(filter.to_string()),
            ("go", VerifyKind::Test) => {
                cmd.push("-run".to_string());
                cmd.push(filter.to_string());
            }
            ("swift", VerifyKind::Test) => {
                cmd.push("--filter".to_string());
                cmd.push(filter.to_string());
            }
            ("rust", VerifyKind::Bench) => cmd.push(filter.to_string()),
            _ => {}
        }
    }
    // `go test -bench` takes the pattern right after it, then the packages.
    if (language, kind) == ("go", VerifyKind::Bench) {
        cmd.push(filter.filter(|f| !f.is_empty()).unwrap_or(".").to_string());
        cmd.push("./...".to_string());
    }
    Ok(cmd)
}

/// Parses one `cargo --message-format=json` line into a diagnostic (None for non-diagnostics
/// and for the trailing "N warnings emitted" summaries).
pub fn parse_cargo_json_line(line: &str) -> Option<Diagnostic> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("reason")?.as_str()? != "compiler-message" {
        return None;
    }
    let message = value.get("message")?;
    let level = message.get("level")?.as_str()?.to_string();
    if level != "error" && level != "warning" {
        return None;
    }
    let spans = message.get("spans").and_then(|s| s.as_array());
    let primary = spans.and_then(|spans| {
        spans
            .iter()
            .find(|s| {
                s.get("is_primary")
                    .and_then(|p| p.as_bool())
                    .unwrap_or(false)
            })
            .or_else(|| spans.first())
    });
    // No span: "aborting due to N previous errors", "N warnings emitted".
    let primary = primary?;
    Some(Diagnostic {
        level,
        code: message
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|c| c.as_str())
            .map(str::to_string),
        message: message.get("message")?.as_str()?.to_string(),
        file: primary
            .get("file_name")
            .and_then(|f| f.as_str())
            .map(str::to_string),
        line: primary.get("line_start").and_then(|l| l.as_u64()),
        column: primary.get("column_start").and_then(|c| c.as_u64()),
    })
}

/// Parses rustc's human-readable output (`error[E0425]: ...` followed by `--> file:line:col`).
pub fn parse_rustc_text(text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut pending: Option<Diagnostic> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        let header = line
            .strip_prefix("error")
            .map(|rest| ("error", rest))
            .or_else(|| line.strip_prefix("warning").map(|rest| ("warning", rest)));
        if let Some((level, rest)) = header
            && let Some((code_part, msg)) = rest.split_once(": ")
            && (code_part.is_empty() || (code_part.starts_with('[') && code_part.ends_with(']')))
        {
            if let Some(d) = pending.take() {
                out.push(d);
            }
            if msg.starts_with("aborting due to")
                || msg.contains("warning(s) emitted")
                || msg.contains("warnings emitted")
                || msg.starts_with("could not compile")
                || msg.starts_with("build failed")
            {
                continue;
            }
            pending = Some(Diagnostic {
                level: level.to_string(),
                code: (!code_part.is_empty())
                    .then(|| code_part.trim_matches(['[', ']']).to_string()),
                message: msg.to_string(),
                file: None,
                line: None,
                column: None,
            });
            continue;
        }
        if let Some(d) = pending.as_mut()
            && d.file.is_none()
            && let Some(loc) = line.trim_start().strip_prefix("--> ")
        {
            let mut parts = loc.rsplitn(3, ':');
            let col = parts.next().and_then(|c| c.parse().ok());
            let ln = parts.next().and_then(|l| l.parse().ok());
            let file = parts.next().map(str::to_string);
            if let Some(file) = file {
                d.file = Some(file);
                d.line = ln;
                d.column = col;
            }
        }
    }
    if let Some(d) = pending {
        out.push(d);
    }
    out
}

/// Parses `cargo test` (libtest) output: per-test results and failure output blocks.
pub fn parse_cargo_test_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures = Vec::new();
    let mut current: Option<TestFailure> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("test result: ")
            && let Some((_, counts)) = rest.split_once(". ")
        {
            for part in counts.split("; ") {
                let mut it = part.split_whitespace();
                if let (Some(n), Some(what)) = (it.next(), it.next())
                    && let Ok(n) = n.parse::<u64>()
                {
                    match what {
                        "passed" => passed += n,
                        "failed" => failed += n,
                        _ => {}
                    }
                }
            }
            continue;
        }
        if let Some(name) = line
            .strip_prefix("---- ")
            .and_then(|r| r.strip_suffix(" stdout ----"))
        {
            if let Some(f) = current.take() {
                failures.push(f);
            }
            current = Some(TestFailure {
                name: name.to_string(),
                output: String::new(),
            });
            continue;
        }
        if line == "failures:" || line.starts_with("test result:") {
            if let Some(f) = current.take() {
                failures.push(f);
            }
            continue;
        }
        if let Some(f) = current.as_mut() {
            f.output.push_str(line);
            f.output.push('\n');
        }
    }
    if let Some(f) = current {
        failures.push(f);
    }
    (passed, failed, failures)
}

/// Parses `path:line:col: (error|warning|note): message` lines as emitted by clang, gcc,
/// cmake builds, ruff (`--output-format concise`) and eslint (`-f unix`).
pub fn parse_colon_diagnostics(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let mut parts = line.splitn(4, ':');
            let file = parts.next()?.trim();
            let ln = parts.next()?.trim().parse::<u64>().ok()?;
            let col_part = parts.next()?.trim();
            let rest = parts.next()?.trim();
            if file.is_empty() || file.contains(' ') {
                return None;
            }
            let (col, message) = match col_part.parse::<u64>() {
                Ok(col) => (Some(col), rest.to_string()),
                Err(_) => (None, format!("{col_part}: {rest}")),
            };
            let (level, message) = if let Some(m) = message.strip_prefix("error:") {
                ("error", m.trim().to_string())
            } else if let Some(m) = message.strip_prefix("warning:") {
                ("warning", m.trim().to_string())
            } else if let Some(m) = message.strip_prefix("fatal error:") {
                ("error", m.trim().to_string())
            } else if message.starts_with("note:") {
                return None;
            } else {
                ("error", message)
            };
            Some(Diagnostic {
                level: level.to_string(),
                code: None,
                message,
                file: Some(file.to_string()),
                line: Some(ln),
                column: col,
            })
        })
        .collect()
}

/// Parses `tsc --pretty false` lines: `src/a.ts(12,5): error TS2322: message`.
pub fn parse_tsc_text(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| {
            let (loc, rest) = line.split_once("): ")?;
            let (file, pos) = loc.rsplit_once('(')?;
            let (ln, col) = pos.split_once(',')?;
            let (level, rest) = rest.split_once(' ')?;
            let (code, message) = rest.split_once(": ")?;
            Some(Diagnostic {
                level: level.to_string(),
                code: Some(code.to_string()),
                message: message.to_string(),
                file: Some(file.trim().to_string()),
                line: ln.parse().ok(),
                column: col.parse().ok(),
            })
        })
        .collect()
}

/// Parses `basedpyright --outputjson`: `generalDiagnostics[]` with file, range and severity.
pub fn parse_pyright_json(text: &str) -> Vec<Diagnostic> {
    let Some(start) = text.find('{') else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text[start..]) else {
        return Vec::new();
    };
    value
        .get("generalDiagnostics")
        .and_then(|d| d.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|d| {
                    let level = d.get("severity")?.as_str()?;
                    if level != "error" && level != "warning" {
                        return None;
                    }
                    Some(Diagnostic {
                        level: level.to_string(),
                        code: d.get("rule").and_then(|r| r.as_str()).map(str::to_string),
                        message: d.get("message")?.as_str()?.to_string(),
                        file: d.get("file").and_then(|f| f.as_str()).map(str::to_string),
                        line: d
                            .pointer("/range/start/line")
                            .and_then(|l| l.as_u64())
                            .map(|l| l + 1),
                        column: d
                            .pointer("/range/start/character")
                            .and_then(|c| c.as_u64())
                            .map(|c| c + 1),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses pytest `-q -rf` output: the `FAILED path::test - message` summary lines and the
/// final `N passed, M failed` line.
pub fn parse_pytest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("FAILED ") {
            let (name, msg) = rest.split_once(" - ").unwrap_or((rest, ""));
            failures.push(TestFailure {
                name: name.trim().to_string(),
                output: msg.trim().to_string(),
            });
        }
        if line.contains(" passed") || line.contains(" failed") {
            for part in line.trim_matches(|c| c == '=' || c == ' ').split(", ") {
                let mut it = part.split_whitespace();
                if let (Some(n), Some(what)) = (it.next(), it.next())
                    && let Ok(n) = n.parse::<u64>()
                {
                    match what.trim_end_matches(|c: char| !c.is_alphabetic()) {
                        "passed" => passed = n,
                        "failed" => failed = n,
                        _ => {}
                    }
                }
            }
        }
    }
    (passed, failed, failures)
}

/// Parses `go build` / `go vet` output lines of the form `path/file.go:12:34: message`.
pub fn parse_go_text(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                return None;
            }
            let mut parts = line.splitn(4, ':');
            let file = parts.next()?;
            let ln = parts.next()?.trim().parse::<u64>().ok()?;
            let rest = parts.next()?;
            let (col, message) = match (rest.trim().parse::<u64>(), parts.next()) {
                (Ok(col), Some(msg)) => (Some(col), msg.trim().to_string()),
                _ => (None, rest.trim().to_string()),
            };
            if !file.ends_with(".go") {
                return None;
            }
            Some(Diagnostic {
                level: "error".to_string(),
                code: None,
                message,
                file: Some(file.to_string()),
                line: Some(ln),
                column: col,
            })
        })
        .collect()
}

/// Parses `go test -json` events into pass/fail counts and per-test failure output.
pub fn parse_go_test_json(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut outputs: std::collections::BTreeMap<String, String> = Default::default();
    let mut failures = Vec::new();
    for line in text.lines() {
        let Ok(ev) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(test) = ev.get("Test").and_then(|t| t.as_str()) else {
            continue;
        };
        let key = format!(
            "{}.{}",
            ev.get("Package").and_then(|p| p.as_str()).unwrap_or(""),
            test
        );
        match ev.get("Action").and_then(|a| a.as_str()) {
            Some("output") => {
                outputs
                    .entry(key)
                    .or_default()
                    .push_str(ev.get("Output").and_then(|o| o.as_str()).unwrap_or(""));
            }
            Some("pass") => passed += 1,
            Some("fail") => {
                failed += 1;
                failures.push(TestFailure {
                    name: key.clone(),
                    output: outputs.remove(&key).unwrap_or_default(),
                });
            }
            _ => {}
        }
    }
    (passed, failed, failures)
}

/// Parses jest output: `Tests:       1 failed, 2 passed, 3 total` plus `  ● suite › name`
/// failure headers followed by their message.
pub fn parse_jest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(rest) = line.trim().strip_prefix("Tests:") {
            for part in rest.split(',') {
                let part = part.trim();
                let mut it = part.split_whitespace();
                let n: u64 = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                match it.next() {
                    Some("passed") => passed = n,
                    Some("failed") => failed = n,
                    _ => {}
                }
            }
            continue;
        }
        if let Some(name) = line.trim().strip_prefix("● ") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            if !name.starts_with("Test suite failed") {
                current = Some((name.trim().to_string(), Vec::new()));
            }
            continue;
        }
        if let Some((_, lines)) = current.as_mut() {
            if lines.len() < 40 {
                lines.push(line.to_string());
            }
        }
    }
    if let Some((n, lines)) = current.take() {
        failures.push(TestFailure {
            name: n,
            output: lines.join("\n"),
        });
    }
    failures.retain(|f| !f.name.is_empty());
    (passed, failed, failures)
}

/// Parses vitest output: ` Tests  1 failed | 2 passed (3)` and `FAIL  file > name` / ` × name`.
pub fn parse_vitest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Tests ") {
            for part in rest.split('|') {
                let mut it = part.split_whitespace();
                let n: u64 = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                match it.next() {
                    Some("passed") => passed = n,
                    Some("failed") => failed = n,
                    _ => {}
                }
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("FAIL ") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            current = Some((rest.trim().to_string(), Vec::new()));
            continue;
        }
        if trimmed.starts_with("Test Files") || trimmed.starts_with("Start at") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            continue;
        }
        if let Some((_, lines)) = current.as_mut() {
            if lines.len() < 40 && !trimmed.is_empty() {
                lines.push(line.to_string());
            }
        }
    }
    if let Some((n, lines)) = current.take() {
        failures.push(TestFailure {
            name: n,
            output: lines.join("\n"),
        });
    }
    (passed, failed, failures)
}

/// Parses `bun test` output: ` 2 pass`, ` 1 fail` and `(fail) name` lines.
pub fn parse_bun_test_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for raw in text.lines() {
        let trimmed = raw.trim();
        let mut it = trimmed.split_whitespace();
        if let (Some(n), Some(word), None) = (it.next(), it.next(), it.next())
            && let Ok(n) = n.parse::<u64>()
        {
            match word {
                "pass" => {
                    passed = n;
                    continue;
                }
                "fail" => {
                    failed = n;
                    continue;
                }
                _ => {}
            }
        }
        if let Some(name) = trimmed.strip_prefix("(fail) ") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            current = Some((
                name.split(" [").next().unwrap_or(name).to_string(),
                Vec::new(),
            ));
            continue;
        }
        if trimmed.starts_with("(pass) ") || trimmed.starts_with("Ran ") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            continue;
        }
        if let Some((_, lines)) = current.as_mut() {
            if lines.len() < 40 && !trimmed.is_empty() {
                lines.push(raw.trim_end().to_string());
            }
        }
    }
    if let Some((n, lines)) = current.take() {
        failures.push(TestFailure {
            name: n,
            output: lines.join("\n"),
        });
    }
    (passed, failed, failures)
}

/// Parses `python -m unittest -v` output: `test_x (mod.Class.test_x) ... ok|FAIL|ERROR`,
/// `Ran N tests`, and the `FAIL:` / `ERROR:` blocks with their tracebacks.
pub fn parse_unittest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for raw in text.lines() {
        let trimmed = raw.trim_end();
        if trimmed.ends_with("... ok") {
            passed += 1;
            continue;
        }
        if trimmed.ends_with("... FAIL") || trimmed.ends_with("... ERROR") {
            failed += 1;
            continue;
        }
        if let Some(name) = trimmed
            .strip_prefix("FAIL: ")
            .or_else(|| trimmed.strip_prefix("ERROR: "))
        {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            current = Some((name.trim().to_string(), Vec::new()));
            continue;
        }
        if trimmed.starts_with("Ran ") || trimmed.starts_with("======") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            continue;
        }
        if trimmed.starts_with("------") {
            continue;
        }
        if let Some((_, lines)) = current.as_mut() {
            if lines.len() < 40 && !trimmed.trim().is_empty() {
                lines.push(trimmed.to_string());
            }
        }
    }
    if let Some((n, lines)) = current.take() {
        failures.push(TestFailure {
            name: n,
            output: lines.join("\n"),
        });
    }
    (passed, failed, failures)
}

/// Parses `meson test` output: `1/3 name   OK|FAIL|ERROR|TIMEOUT` lines and the summary.
pub fn parse_meson_test_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        let Some((idx, rest)) = trimmed.split_once(' ') else {
            continue;
        };
        if !idx.contains('/') || !idx.chars().all(|c| c.is_ascii_digit() || c == '/') {
            continue;
        }
        let words: Vec<&str> = rest.split_whitespace().collect();
        let Some(name) = words.first() else {
            continue;
        };
        if words.contains(&"OK") {
            passed += 1;
        } else if words
            .iter()
            .any(|w| matches!(*w, "FAIL" | "ERROR" | "TIMEOUT"))
        {
            failed += 1;
            failures.push(TestFailure {
                name: name.to_string(),
                output: rest.to_string(),
            });
        }
    }
    (passed, failed, failures)
}

/// Parses XCTest (`swift test`) output on macOS and Linux: `Test Case '-[Suite test]' passed
/// (0.001 seconds)` / `Test Case 'Suite.test' failed`, assertion lines `file:line: error:
/// -[Suite test] : message`, plus swift-testing `✔ Test "name" passed` / `✘ Test "name" failed`.
pub fn parse_xctest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut assertions: Vec<(String, String)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("Test Case '") {
            let Some((name, status)) = rest.split_once("' ") else {
                continue;
            };
            let name = name
                .trim_start_matches("-[")
                .trim_end_matches(']')
                .replace(' ', ".");
            if status.starts_with("passed") {
                passed += 1;
            } else if status.starts_with("failed") {
                failed += 1;
                let output = assertions
                    .iter()
                    .filter(|(test, _)| *test == name)
                    .map(|(_, msg)| msg.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                failures.push(TestFailure { name, output });
            }
            continue;
        }
        if let Some(pos) = line.find(": error: -[") {
            let location = &line[..pos];
            let rest = &line[pos + ": error: -[".len()..];
            if let Some((test, message)) = rest.split_once("] : ") {
                assertions.push((
                    test.replace(' ', "."),
                    format!("{location}: {}", message.trim()),
                ));
            }
            continue;
        }
        // swift-testing: `✔ Test "adds"() passed after 0.001 seconds.`
        if let Some(rest) = line
            .strip_prefix("✔ Test ")
            .or_else(|| line.strip_prefix("✘ Test "))
        {
            if rest.starts_with("run ") {
                continue;
            }
            let name = rest
                .split(" passed")
                .next()
                .and_then(|n| n.split(" failed").next())
                .unwrap_or(rest)
                .trim_matches('"')
                .to_string();
            if line.starts_with('✔') {
                passed += 1;
            } else {
                failed += 1;
                failures.push(TestFailure {
                    name,
                    output: line.to_string(),
                });
            }
        }
    }
    (passed, failed, failures)
}

/// Parses `ctest --output-on-failure` summaries: `1/3 Test #1: name ...... Passed` /
/// `***Failed`, with the failing test's output captured until the next test line.
pub fn parse_ctest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    let flush = |current: &mut Option<(String, Vec<String>)>, failures: &mut Vec<TestFailure>| {
        if let Some((name, lines)) = current.take() {
            failures.push(TestFailure {
                name,
                output: lines.join("\n"),
            });
        }
    };
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        let is_test_line = trimmed
            .split_once(' ')
            .is_some_and(|(n, rest)| n.contains('/') && rest.starts_with("Test #"));
        if is_test_line {
            flush(&mut current, &mut failures);
            let name = trimmed
                .split_once(": ")
                .map(|(_, r)| r.split(" .").next().unwrap_or(r).trim().to_string())
                .unwrap_or_default();
            if trimmed.ends_with("Passed") || trimmed.contains(" Passed ") {
                passed += 1;
            } else if trimmed.contains("Failed") || trimmed.contains("Timeout") {
                failed += 1;
                current = Some((name, Vec::new()));
            }
            continue;
        }
        if trimmed.starts_with("% tests passed") || trimmed.contains("% tests passed,") {
            flush(&mut current, &mut failures);
            continue;
        }
        if let Some((_, lines)) = current.as_mut() {
            lines.push(line.to_string());
        }
    }
    flush(&mut current, &mut failures);
    (passed, failed, failures)
}

/// Rewrites diagnostic file paths that the tool printed as absolute server paths into
/// checkout-relative ones.
fn relativize_diagnostics(diagnostics: &mut [Diagnostic], server_root: &str) {
    if server_root.is_empty() {
        return;
    }
    let prefix = format!("{}/", server_root.trim_end_matches('/'));
    for diagnostic in diagnostics {
        if let Some(file) = diagnostic.file.as_mut()
            && let Some(rel) = file.strip_prefix(&prefix)
        {
            *file = rel.to_string();
        }
    }
}

/// Whether `root` is an Xcode project or workspace rather than a SwiftPM package: then
/// `xcodebuild` drives builds and tests (needed for app bundles, simulators and UI tests).
pub fn has_xcode_project(root: &Path) -> bool {
    if root.join("Package.swift").exists() {
        return false;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".xcodeproj") || name.ends_with(".xcworkspace")
            })
        })
        .unwrap_or(false)
}

/// `xcodebuild build` / `xcodebuild test` for an Xcode project: the scheme is `filter` when
/// given, otherwise the first scheme `xcodebuild -list` reports; iOS targets run on the
/// first available iPhone simulator, everything else on the Mac.
pub fn plan_xcode_command(kind: VerifyKind, scheme: Option<&str>) -> Result<Vec<String>> {
    let action = match kind {
        VerifyKind::Check => "build",
        VerifyKind::Test => "test",
        VerifyKind::Lint => return Err(anyhow!("no lint command for Xcode projects")),
        VerifyKind::Bench => return Err(anyhow!("no bench command for Xcode projects")),
    };
    let scheme_expr = match scheme.filter(|s| !s.is_empty()) {
        Some(s) => format!("'{}'", s.replace('\'', "'\\''")),
        None => "\"$(xcodebuild -list -json 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); d=d.get(\"project\") or d.get(\"workspace\"); print(d[\"schemes\"][0])')\"".to_string(),
    };
    let script = format!(
        "scheme={scheme_expr}; \
if xcodebuild -showBuildSettings -scheme \"$scheme\" 2>/dev/null | grep -q 'SDKROOT.*iPhoneOS'; then \
  dest=\"platform=iOS Simulator,name=$(xcrun simctl list devices available | grep -m1 -o 'iPhone[^(]*' | sed 's/ *$//')\"; \
else dest='platform=macOS'; fi; \
xcodebuild {action} -scheme \"$scheme\" -destination \"$dest\" -quiet 2>&1"
    );
    Ok(vec!["sh".to_string(), "-c".to_string(), script])
}

/// Runs the verification remotely and parses its output.
/// Something a run reports while it runs: a diagnostic or a test's result, read from a line
/// of its output as soon as the line arrives (cargo's JSON, cargo test's `test … ok`, `go test
/// -json`). The parsed report at the end holds everything again.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunEvent {
    Diagnostic(Diagnostic),
    Test { name: String, ok: bool },
}

/// The event one line of `language`'s `kind` output carries, when it carries one.
pub fn event_of_line(language: &str, kind: VerifyKind, line: &str) -> Option<RunEvent> {
    match (language, kind) {
        ("rust", VerifyKind::Check | VerifyKind::Lint) => {
            parse_cargo_json_line(line).map(RunEvent::Diagnostic)
        }
        ("rust", VerifyKind::Test) => {
            let rest = line.strip_prefix("test ")?;
            let (name, outcome) = rest.rsplit_once(" ... ")?;
            match outcome.trim() {
                "ok" => Some(RunEvent::Test {
                    name: name.to_string(),
                    ok: true,
                }),
                "FAILED" => Some(RunEvent::Test {
                    name: name.to_string(),
                    ok: false,
                }),
                _ => None,
            }
        }
        ("go", VerifyKind::Test) => {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            let name = v.get("Test")?.as_str()?.to_string();
            match v.get("Action")?.as_str()? {
                "pass" => Some(RunEvent::Test { name, ok: true }),
                "fail" => Some(RunEvent::Test { name, ok: false }),
                _ => None,
            }
        }
        _ => None,
    }
}

pub async fn run_verify(
    remote: SocketAddr,
    root: &Path,
    project_hint: Option<&Path>,
    kind: VerifyKind,
    filter: Option<&str>,
    timeout_secs: u64,
) -> Result<VerifyReport> {
    run_verify_with(
        remote,
        root,
        project_hint,
        kind,
        filter,
        timeout_secs,
        &[],
        |_| {},
    )
    .await
}

/// [`run_verify`] with extra environment for the command (`RUST_BACKTRACE=1`) and every
/// [`RunEvent`] handed to `on_event` as its line arrives.
#[allow(clippy::too_many_arguments)]
pub async fn run_verify_with(
    remote: SocketAddr,
    root: &Path,
    project_hint: Option<&Path>,
    kind: VerifyKind,
    filter: Option<&str>,
    timeout_secs: u64,
    extra_env: &[(String, String)],
    mut on_event: impl FnMut(RunEvent),
) -> Result<VerifyReport> {
    // A nested project of another language (a SwiftPM package in a Rust repository) is
    // verified in its own directory with its own tooling.
    let (subdir, language) = crate::sync::engine_project(root, project_hint.unwrap_or(root));
    let language = language.ok_or_else(|| {
        anyhow!(
            "no project manifest (Cargo.toml, go.mod, package.json, pyproject.toml, CMakeLists.txt, Package.swift) at {}",
            root.display()
        )
    })?;
    let project_dir = match &subdir {
        Some(sub) => root.join(sub),
        None => root.to_path_buf(),
    };
    let tools = detect_tools(&project_dir);
    let mut command = if language == "swift" && has_xcode_project(&project_dir) {
        plan_xcode_command(kind, filter)?
    } else {
        plan_command_with(&tools, language, kind, filter)?
    };
    if let Some(hint) = project_hint {
        narrow_scope(&mut command, language, &project_dir, hint);
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut tail = TailBuffer::new(8 * 1024);
    let mut env = vec![
        ("CARGO_TERM_COLOR".to_string(), "never".to_string()),
        ("NO_COLOR".to_string(), "1".to_string()),
    ];
    env.extend(extra_env.iter().cloned());
    // Complete stdout lines become events as they arrive; a line split across chunks waits.
    let mut pending: Vec<u8> = Vec::new();
    let outcome = run_remote(
        remote,
        root,
        subdir.as_deref(),
        command.clone(),
        env,
        timeout_secs,
        false,
        |is_stderr, data| {
            if !is_stderr {
                pending.extend_from_slice(data);
                while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=end).collect();
                    if let Some(event) =
                        event_of_line(language, kind, String::from_utf8_lossy(&line).trim_end())
                    {
                        on_event(event);
                    }
                }
            }
            tail.push(data);
            if is_stderr {
                stderr.extend_from_slice(data);
            } else {
                stdout.extend_from_slice(data);
            }
        },
    )
    .await?;
    if let Some(err) = &outcome.exit.error {
        return Err(anyhow!("remote {} failed to start: {err}", kind.label()));
    }
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);

    let mut diagnostics = Vec::new();
    let mut fixes = Vec::new();
    let mut benches = Vec::new();
    let (mut tests_passed, mut tests_failed, mut failures) = (0, 0, Vec::new());
    match (language, kind) {
        ("rust", VerifyKind::Check) | ("rust", VerifyKind::Lint) => {
            diagnostics.extend(stdout.lines().filter_map(parse_cargo_json_line));
            fixes.extend(stdout.lines().flat_map(crate::fixit::parse_fixes));
        }
        (_, VerifyKind::Bench) => {
            diagnostics.extend(parse_rustc_text(&stderr));
            benches.extend(parse_bench_text(&format!("{stdout}\n{stderr}")));
        }
        ("rust", VerifyKind::Test) => {
            diagnostics.extend(parse_rustc_text(&stderr));
            let (p, f, fails) = parse_cargo_test_text(&stdout);
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("go", VerifyKind::Test) => {
            diagnostics.extend(parse_go_text(&stderr));
            let (p, f, fails) = parse_go_test_json(&stdout);
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("go", _) => {
            diagnostics.extend(parse_go_text(&stderr));
            diagnostics.extend(parse_go_text(&stdout));
        }
        ("typescript", VerifyKind::Check) => {
            diagnostics.extend(parse_tsc_text(&stdout));
            diagnostics.extend(parse_tsc_text(&stderr));
        }
        ("python", VerifyKind::Check) => {
            diagnostics.extend(parse_pyright_json(&stdout));
        }
        ("python", VerifyKind::Test) => {
            let combined = format!("{stdout}\n{stderr}");
            let (p, f, fails) = match tools.python_tests {
                PythonTestRunner::Pytest => parse_pytest_text(&stdout),
                PythonTestRunner::Unittest => parse_unittest_text(&combined),
            };
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("typescript", VerifyKind::Test) => {
            let combined = format!("{stdout}\n{stderr}");
            let (p, f, fails) = match tools.js_tests {
                JsTestRunner::Vitest => parse_vitest_text(&combined),
                JsTestRunner::Jest => parse_jest_text(&combined),
                JsTestRunner::BunTest => parse_bun_test_text(&combined),
                JsTestRunner::Mocha | JsTestRunner::Script => (0, 0, Vec::new()),
            };
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("swift", VerifyKind::Test) => {
            let combined = format!("{stdout}\n{stderr}");
            diagnostics.extend(parse_colon_diagnostics(&stderr));
            diagnostics.retain(|d| !d.message.starts_with("-["));
            let (p, f, fails) = parse_xctest_text(&combined);
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("cpp", VerifyKind::Test) => {
            let (p, f, fails) = match tools.cpp {
                CppBuild::CMake => parse_ctest_text(&stdout),
                CppBuild::Meson => parse_meson_test_text(&stdout),
                CppBuild::Make => (0, 0, Vec::new()),
            };
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        _ => {
            diagnostics.extend(parse_colon_diagnostics(&stderr));
            diagnostics.extend(parse_colon_diagnostics(&stdout));
        }
    }
    diagnostics.dedup();
    if let Some(sub) = &subdir {
        let nested = format!(
            "{}/{sub}",
            outcome.exit.server_workspace_root.trim_end_matches('/')
        );
        relativize_diagnostics(&mut diagnostics, &nested);
    }
    relativize_diagnostics(&mut diagnostics, &outcome.exit.server_workspace_root);
    for failure in &mut failures {
        let prefix = format!(
            "{}/",
            outcome.exit.server_workspace_root.trim_end_matches('/')
        );
        if prefix.len() > 1 {
            failure.output = failure.output.replace(&prefix, "");
        }
    }

    Ok(VerifyReport {
        kind,
        language: language.to_string(),
        command,
        exit_code: outcome.exit.exit_code,
        timed_out: outcome.exit.timed_out,
        duration_ms: outcome.exit.duration_ms,
        diagnostics,
        tests_passed,
        tests_failed,
        failures,
        tail: tail.text(),
        fixes,
        benches,
        usage: outcome.exit.usage,
        platform: outcome.exit.platform.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_scope_picks_cargo_member_go_dir_and_pytest_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        let member = root.join("crates/gw/src");
        std::fs::create_dir_all(&member).unwrap();
        std::fs::write(
            root.join("crates/gw/Cargo.toml"),
            "[package]\nname = \"gw\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(member.join("lib.rs"), "").unwrap();

        let mut cmd = strs(&["cargo", "test", "--workspace"]);
        narrow_scope(&mut cmd, "rust", root, &member.join("lib.rs"));
        assert_eq!(cmd, strs(&["cargo", "test", "-p", "gw"]));

        let mut cmd = strs(&["cargo", "test", "--workspace"]);
        narrow_scope(&mut cmd, "rust", root, root);
        assert_eq!(cmd, strs(&["cargo", "test", "--workspace"]));

        // A crate name instead of a path.
        let mut cmd = strs(&["cargo", "test", "--workspace"]);
        narrow_scope(&mut cmd, "rust", root, &root.join("gw"));
        assert_eq!(cmd, strs(&["cargo", "test", "-p", "gw"]));

        let mut cmd = strs(&["go", "test", "-json", "./..."]);
        narrow_scope(&mut cmd, "go", root, &root.join("crates/gw"));
        assert_eq!(cmd, strs(&["go", "test", "-json", "./crates/gw/..."]));

        let mut cmd = strs(&["python3", "-m", "pytest", "-q"]);
        narrow_scope(&mut cmd, "python", root, &member.join("lib.rs"));
        assert_eq!(cmd.last().unwrap(), "crates/gw/src/lib.rs");
    }

    #[test]
    fn detects_js_and_python_tooling() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"vitest":"1","eslint":"9"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("bun.lock"), "").unwrap();
        std::fs::write(root.join("uv.lock"), "").unwrap();
        std::fs::write(root.join("meson.build"), "").unwrap();
        let tools = detect_tools(root);
        assert_eq!(tools.package_manager, PackageManager::Bun);
        assert_eq!(tools.js_tests, JsTestRunner::Vitest);
        assert_eq!(tools.js_linter, Some("eslint"));
        assert_eq!(tools.python, PythonRuntime::Uv);
        assert_eq!(tools.cpp, CppBuild::Meson);
        let cmd = plan_command_with(&tools, "typescript", VerifyKind::Test, Some("adds")).unwrap();
        assert_eq!(cmd[..3], ["bunx", "vitest", "run"]);
        assert!(cmd.ends_with(&["-t".to_string(), "adds".to_string()]));
        let cmd = plan_command_with(&tools, "python", VerifyKind::Check, None).unwrap();
        assert_eq!(cmd[..2], ["uv", "run"]);
        let cmd = plan_command_with(&tools, "python", VerifyKind::Test, None).unwrap();
        assert_eq!(cmd[..5], ["uv", "run", "python", "-m", "pytest"]);

        std::fs::remove_file(root.join("bun.lock")).unwrap();
        std::fs::remove_file(root.join("uv.lock")).unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::create_dir_all(root.join(".venv/bin")).unwrap();
        std::fs::write(root.join(".venv/bin/python"), "").unwrap();
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(root.join("tests/test_a.py"), "import unittest\n").unwrap();
        let tools = detect_tools(root);
        assert_eq!(tools.package_manager, PackageManager::Pnpm);
        assert_eq!(tools.python, PythonRuntime::Venv(".venv/bin/python".into()));
        assert_eq!(tools.python_tests, PythonTestRunner::Unittest);
        let cmd = plan_command_with(&tools, "python", VerifyKind::Test, None).unwrap();
        assert_eq!(cmd[..3], [".venv/bin/python", "-m", "unittest"]);
        let cmd = plan_command_with(&tools, "python", VerifyKind::Check, None).unwrap();
        assert!(cmd.contains(&"--pythonpath".to_string()));

        let defaults = ProjectTools::default();
        assert_eq!(
            plan_command_with(&defaults, "typescript", VerifyKind::Check, None).unwrap()[..3],
            ["npx", "--no-install", "tsc"]
        );
        assert!(plan_command_with(&defaults, "typescript", VerifyKind::Lint, None).is_err());
    }

    #[test]
    fn parses_js_and_python_test_runners() {
        let jest = "PASS src/a.test.ts\nFAIL src/b.test.ts\n  ● math › adds\n\n    expect(received).toBe(expected)\n\nTests:       1 failed, 1 passed, 2 total\n";
        let (p, f, fails) = parse_jest_text(jest);
        assert_eq!((p, f), (1, 1));
        assert_eq!(fails[0].name, "math › adds");
        assert!(fails[0].output.contains("expect(received)"));

        let vitest = " ✓ src/a.test.ts (1)\n ❯ src/b.test.ts (1)\n   × adds\n\n FAIL  src/b.test.ts > adds\nAssertionError: expected 2 to be 3\n\n Test Files  1 failed | 1 passed (2)\n      Tests  1 failed | 1 passed (2)\n";
        let (p, f, fails) = parse_vitest_text(vitest);
        assert_eq!((p, f), (1, 1));
        assert_eq!(fails[0].name, "src/b.test.ts > adds");
        assert!(fails[0].output.contains("AssertionError"));

        let bun = "bun test v1.4.2\n\nsrc/a.test.ts:\n(pass) adds\n(fail) subtracts [0.10ms]\nerror: expect(received).toBe(expected)\n\n 1 pass\n 1 fail\nRan 2 tests across 1 file.\n";
        let (p, f, fails) = parse_bun_test_text(bun);
        assert_eq!((p, f), (1, 1));
        assert_eq!(fails[0].name, "subtracts");
        assert!(fails[0].output.contains("expect(received)"));

        let unittest = "test_adds (tests.test_a.T.test_adds) ... ok\ntest_subs (tests.test_a.T.test_subs) ... FAIL\n\n======================================================================\nFAIL: test_subs (tests.test_a.T.test_subs)\n----------------------------------------------------------------------\nTraceback (most recent call last):\nAssertionError: 2 != 3\n\n----------------------------------------------------------------------\nRan 2 tests in 0.001s\n\nFAILED (failures=1)\n";
        let (p, f, fails) = parse_unittest_text(unittest);
        assert_eq!((p, f), (1, 1));
        assert_eq!(fails[0].name, "test_subs (tests.test_a.T.test_subs)");
        assert!(fails[0].output.contains("AssertionError: 2 != 3"));

        let meson = "1/2 adds        OK              0.01s\n2/2 subs        FAIL            0.02s   exit status 1\n\nOk:                 1\nFail:               1\n";
        let (p, f, fails) = parse_meson_test_text(meson);
        assert_eq!((p, f), (1, 1));
        assert_eq!(fails[0].name, "subs");
    }

    #[test]
    fn plans_xcodebuild_commands() {
        let cmd = plan_xcode_command(VerifyKind::Test, Some("Tako")).unwrap();
        assert_eq!(&cmd[..2], &["sh".to_string(), "-c".to_string()]);
        assert!(cmd[2].contains("scheme='Tako'"));
        assert!(cmd[2].contains("xcodebuild test -scheme"));
        let cmd = plan_xcode_command(VerifyKind::Check, None).unwrap();
        assert!(cmd[2].contains("xcodebuild -list -json"));
        assert!(cmd[2].contains("xcodebuild build -scheme"));
        assert!(plan_xcode_command(VerifyKind::Lint, None).is_err());
        let temp = tempfile::tempdir().unwrap();
        assert!(!has_xcode_project(temp.path()));
        std::fs::create_dir_all(temp.path().join("App.xcodeproj")).unwrap();
        assert!(has_xcode_project(temp.path()));
        std::fs::write(temp.path().join("Package.swift"), "").unwrap();
        assert!(!has_xcode_project(temp.path()));
    }

    #[test]
    fn parses_xctest_output() {
        let text = "Test Case '-[SignalTests.SignalTests testDoubles]' started.\n\
/srv/ws/Tests/SignalTests/SignalTests.swift:6: error: -[SignalTests.SignalTests testDoubles] : XCTAssertEqual failed: (\"42\") is not equal to (\"43\")\n\
Test Case '-[SignalTests.SignalTests testDoubles]' failed (0.411 seconds).\n\
Test Case 'OtherTests.testOk' passed (0.001 seconds).\n\
\t Executed 2 tests, with 1 failure (0 unexpected) in 0.4 (0.4) seconds\n\
✔ Test run with 0 tests in 0 suites passed after 0.001 seconds.\n";
        let (passed, failed, failures) = parse_xctest_text(text);
        assert_eq!((passed, failed), (1, 1));
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].name, "SignalTests.SignalTests.testDoubles");
        assert!(
            failures[0]
                .output
                .contains("SignalTests.swift:6: XCTAssertEqual failed")
        );
    }

    #[test]
    fn parses_ctest_output() {
        let text = "Test project /srv/ws/build\n\
    Start 1: adds\n\
1/2 Test #1: adds .............................   Passed    0.01 sec\n\
    Start 2: fails\n\
2/2 Test #2: fails ............................***Failed    0.02 sec\n\
expected 42, got 43\n\
\n\
50% tests passed, 1 tests failed out of 2\n";
        let (passed, failed, failures) = parse_ctest_text(text);
        assert_eq!((passed, failed), (1, 1));
        assert_eq!(failures[0].name, "fails");
        assert!(failures[0].output.contains("expected 42, got 43"));
    }

    #[test]
    fn relativizes_server_paths() {
        let mut diagnostics = vec![Diagnostic {
            level: "error".into(),
            code: None,
            message: "boom".into(),
            file: Some("/srv/ws/src/a.cpp".into()),
            line: Some(1),
            column: None,
        }];
        relativize_diagnostics(&mut diagnostics, "/srv/ws");
        assert_eq!(diagnostics[0].file.as_deref(), Some("src/a.cpp"));
    }

    #[test]
    fn cargo_json_diagnostic() {
        let line = r#"{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0425"},"message":"cannot find value `x` in this scope","spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":9,"is_primary":true}]}}"#;
        let d = parse_cargo_json_line(line).unwrap();
        assert_eq!(
            d.render(),
            "error: [E0425] cannot find value `x` in this scope (src/lib.rs:3:9)"
        );
        let summary = r#"{"reason":"compiler-message","message":{"level":"warning","message":"2 warnings emitted","spans":[]}}"#;
        assert!(parse_cargo_json_line(summary).is_none());
        assert!(parse_cargo_json_line(r#"{"reason":"build-finished","success":true}"#).is_none());
    }

    #[test]
    fn rustc_text_diagnostics() {
        let text = "error[E0308]: mismatched types\n  --> crates/a/src/lib.rs:12:5\n   |\nwarning: unused variable: `y`\n --> src/main.rs:4:9\nerror: aborting due to 1 previous error\n";
        let d = parse_rustc_text(text);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].code.as_deref(), Some("E0308"));
        assert_eq!(d[0].file.as_deref(), Some("crates/a/src/lib.rs"));
        assert_eq!((d[0].line, d[0].column), (Some(12), Some(5)));
        assert_eq!(d[1].level, "warning");
        assert_eq!(d[1].file.as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn cargo_test_text() {
        let text = "running 3 tests\ntest a::ok ... ok\ntest a::bad ... FAILED\ntest a::also ... ok\n\nfailures:\n\n---- a::bad stdout ----\nthread 'a::bad' panicked at src/lib.rs:5:9:\nassertion failed: 1 == 2\n\n\nfailures:\n    a::bad\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
        let (p, f, fails) = parse_cargo_test_text(text);
        assert_eq!((p, f), (2, 1));
        assert_eq!(fails.len(), 1);
        assert_eq!(fails[0].name, "a::bad");
        assert!(fails[0].output.contains("assertion failed: 1 == 2"));
    }

    #[test]
    fn go_text_and_json() {
        let d = parse_go_text("# prod/cmd\ncmd/main.go:10:2: undefined: foo\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].render(), "error: undefined: foo (cmd/main.go:10:2)");
        let events = r#"{"Action":"run","Package":"p","Test":"TestA"}
{"Action":"output","Package":"p","Test":"TestA","Output":"    a_test.go:7: boom\n"}
{"Action":"fail","Package":"p","Test":"TestA","Elapsed":0}
{"Action":"pass","Package":"p","Test":"TestB","Elapsed":0}
{"Action":"fail","Package":"p","Elapsed":0.1}"#;
        let (p, f, fails) = parse_go_test_json(events);
        assert_eq!((p, f), (1, 1));
        assert_eq!(fails[0].name, "p.TestA");
        assert!(fails[0].output.contains("boom"));
    }

    #[test]
    fn colon_tsc_pyright_pytest_parsers() {
        let d = parse_colon_diagnostics(
            "src/a.cpp:12:5: error: no member named 'x'\nsrc/b.cpp:3:1: warning: unused\nnote: ignored\n",
        );
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].render(), "error: no member named 'x' (src/a.cpp:12:5)");
        assert_eq!(d[1].level, "warning");
        let t = parse_tsc_text(
            "src/index.ts(7,3): error TS2322: Type 'string' is not assignable to type 'number'.\n",
        );
        assert_eq!(t.len(), 1);
        assert_eq!(
            t[0].render(),
            "error: [TS2322] Type 'string' is not assignable to type 'number'. (src/index.ts:7:3)"
        );
        let py = parse_pyright_json(
            r#"{"generalDiagnostics":[{"file":"/w/a.py","severity":"error","message":"boom","range":{"start":{"line":4,"character":2}},"rule":"reportGeneralTypeIssues"}],"summary":{}}"#,
        );
        assert_eq!(
            py[0].render(),
            "error: [reportGeneralTypeIssues] boom (/w/a.py:5:3)"
        );
        let (p, f, fails) = parse_pytest_text(
            "FAILED tests/test_a.py::test_x - AssertionError: nope\n===== 1 failed, 3 passed in 0.10s =====\n",
        );
        assert_eq!((p, f), (3, 1));
        assert_eq!(fails[0].name, "tests/test_a.py::test_x");
        assert!(
            plan_command("swift", VerifyKind::Test, Some("Foo"))
                .unwrap()
                .ends_with(&["--filter".to_string(), "Foo".to_string()])
        );
        // C++ lints with clang-tidy since #205.
        assert!(
            plan_command("cpp", VerifyKind::Lint, None)
                .unwrap()
                .join(" ")
                .contains("clang-tidy -p build")
        );
    }

    #[test]
    fn benchmark_results_are_read_from_criterion_libtest_and_go() {
        let criterion = "Benchmarking sum 1000\nBenchmarking sum 1000: Warming up for 3.0000 s\nsum 1000                time:   [2.3499 ns 2.3500 ns 2.3502 ns]\na_benchmark_with_a_very_long_name\n                        time:   [10.1 µs 10.2 µs 10.4 µs]\n                        change: [-1.0% +0.5% +2.0%]\n";
        let libtest = "test sort::big ... bench:       1,234 ns/iter (+/- 56)\n";
        let go = "BenchmarkSum-8   \t 1000000\t      1234 ns/op\nPASS\n";
        let found = parse_bench_text(&format!("{criterion}{libtest}{go}"));
        let rows: Vec<(&str, &str, Option<&str>)> = found
            .iter()
            .map(|b| (b.name.as_str(), b.estimate.as_str(), b.range.as_deref()))
            .collect();
        assert_eq!(
            rows,
            [
                ("sum 1000", "2.3500 ns", Some("2.3499 ns .. 2.3502 ns")),
                (
                    "a_benchmark_with_a_very_long_name",
                    "10.2 µs",
                    Some("10.1 µs .. 10.4 µs")
                ),
                ("sort::big", "1234 ns/iter", Some("+/- 56")),
                ("BenchmarkSum-8", "1234 ns/op", None),
            ]
        );
        assert_eq!(
            plan_command_basic("go", VerifyKind::Bench, None).unwrap(),
            ["go", "test", "-run", "^$", "-bench", ".", "./..."]
        );
        assert_eq!(
            plan_command_basic("rust", VerifyKind::Bench, Some("sum")).unwrap(),
            ["cargo", "bench", "--workspace", "sum"]
        );
        let report = VerifyReport {
            kind: VerifyKind::Bench,
            language: "rust".into(),
            command: vec!["cargo".into(), "bench".into()],
            exit_code: Some(0),
            timed_out: false,
            duration_ms: 9000,
            diagnostics: vec![],
            tests_passed: 0,
            tests_failed: 0,
            failures: vec![],
            tail: String::new(),
            fixes: vec![],
            benches: found,
            usage: None,
            platform: None,
        };
        let text = report.render(10);
        assert!(
            text.contains("rust bench: OK in 9.0s; 4 benchmark(s)"),
            "{text}"
        );
        assert!(
            text.contains("  sum 1000  2.3500 ns  [2.3499 ns .. 2.3502 ns]"),
            "{text}"
        );
    }

    #[test]
    fn plans_and_summary() {
        assert_eq!(
            plan_command("rust", VerifyKind::Test, Some("sync::"))
                .unwrap()
                .last()
                .unwrap(),
            "sync::"
        );
        assert_eq!(
            plan_command("go", VerifyKind::Test, Some("TestA")).unwrap()[3..],
            ["./...", "-run", "TestA"]
        );
        assert!(plan_command("ruby", VerifyKind::Check, None).is_err());
        let report = VerifyReport {
            kind: VerifyKind::Test,
            language: "rust".into(),
            command: vec!["cargo".into(), "test".into()],
            exit_code: Some(101),
            timed_out: false,
            duration_ms: 1500,
            diagnostics: vec![],
            tests_passed: 2,
            tests_failed: 1,
            failures: vec![TestFailure {
                name: "a::bad".into(),
                output: "boom\n".into(),
            }],
            tail: String::new(),
            fixes: vec![],
            benches: vec![],
            usage: None,
            platform: None,
        };
        assert_eq!(
            report.summary(),
            "rust test: FAILED (exit 101) in 1.5s; 2 passed, 1 failed"
        );
        assert!(report.render(10).contains("--- FAILED a::bad ---\nboom"));
    }

    #[test]
    fn each_linter_has_its_fix_mode_and_cpp_lints_with_clang_tidy() {
        let mut tools = ProjectTools::default();
        let py = fix_command(&tools, "python").unwrap().unwrap().join(" ");
        assert_eq!(py, "ruff check . --fix --output-format concise");
        assert!(fix_command(&tools, "go").unwrap().is_none());
        assert!(fix_command(&tools, "rust").unwrap().is_none());
        assert!(fix_command(&tools, "typescript").unwrap().is_none());
        tools.js_linter = Some("eslint");
        assert!(
            fix_command(&tools, "typescript")
                .unwrap()
                .unwrap()
                .join(" ")
                .ends_with("eslint . --fix -f unix")
        );
        tools.js_linter = Some("biome");
        assert!(
            fix_command(&tools, "typescript")
                .unwrap()
                .unwrap()
                .join(" ")
                .ends_with("biome lint --write .")
        );
        let cpp = fix_command(&tools, "cpp").unwrap().unwrap().join(" ");
        assert!(cpp.contains("clang-tidy -p build --quiet -fix"), "{cpp}");
        let lint = plan_command_with(&tools, "cpp", VerifyKind::Lint, None)
            .unwrap()
            .join(" ");
        assert!(
            lint.contains("-DCMAKE_EXPORT_COMPILE_COMMANDS=ON") && lint.ends_with("--quiet"),
            "{lint}"
        );
        tools.cpp = CppBuild::Meson;
        assert!(
            plan_command_with(&tools, "cpp", VerifyKind::Lint, None)
                .unwrap()
                .join(" ")
                .contains("meson setup build")
        );
        tools.cpp = CppBuild::Make;
        assert!(plan_command_with(&tools, "cpp", VerifyKind::Lint, None).is_err());
    }

    #[test]
    fn a_line_of_output_becomes_an_event_as_it_arrives() {
        let json = r#"{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0308"},"message":"mismatched types","spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":5,"is_primary":true}],"children":[]}}"#;
        assert!(matches!(
            event_of_line("rust", VerifyKind::Check, json),
            Some(RunEvent::Diagnostic(d)) if d.message == "mismatched types"
        ));
        assert_eq!(
            event_of_line("rust", VerifyKind::Test, "test tests::adds ... ok"),
            Some(RunEvent::Test {
                name: "tests::adds".into(),
                ok: true
            })
        );
        assert_eq!(
            event_of_line("rust", VerifyKind::Test, "test tests::fails ... FAILED"),
            Some(RunEvent::Test {
                name: "tests::fails".into(),
                ok: false
            })
        );
        assert_eq!(
            event_of_line("rust", VerifyKind::Test, "test x ... ignored"),
            None
        );
        assert_eq!(
            event_of_line(
                "go",
                VerifyKind::Test,
                r#"{"Action":"fail","Package":"p","Test":"TestX"}"#
            ),
            Some(RunEvent::Test {
                name: "TestX".into(),
                ok: false
            })
        );
        assert_eq!(
            event_of_line("go", VerifyKind::Test, r#"{"Action":"run","Test":"TestX"}"#),
            None
        );
        assert_eq!(event_of_line("python", VerifyKind::Test, "PASSED"), None);
        let text = serde_json::to_string(&RunEvent::Test {
            name: "t".into(),
            ok: true,
        })
        .unwrap();
        assert_eq!(text, r#"{"event":"test","name":"t","ok":true}"#);
    }
}
