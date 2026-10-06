/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::path::Path;

use super::render::shell_words;
use super::rust_attr::rust_test_marker;
use super::types::Symbol;

/// What the text around a caller's declaration says about it being a test, beyond its name:
/// a test attribute above it (`#[test]`, `#[tokio::test]`, `@Test`), a test registration it
/// sits in (`TEST(Suite, Name)`, `TEST_F`, `TEST_CASE("…")`), or, for Python, a `test*` method
/// of a `unittest.TestCase` class. `Some(name)` is a test, under the name its runner selects
/// it by (`Suite.Name` for a gtest registration); `None` is not one as far as the text shows.
pub fn test_marker(language: &str, text: &str, line: u32, name: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let at = (line as usize).checked_sub(1)?;
    let here = *lines.get(at)?;
    let bare = name
        .split('(')
        .next()
        .unwrap_or(name)
        .rsplit(['.', ':'])
        .next()
        .unwrap_or(name);
    // The attribute lines right above the declaration, nearest first.
    let above = || {
        lines[..at].iter().rev().map(|l| l.trim()).take_while(|l| {
            let attribute = l.starts_with("#[") || l.starts_with('@') || l.starts_with("///");
            // `@Test func adds()` is a declaration of its own, not an attribute of the next.
            let declares = [" fn ", "func ", "def "].iter().any(|k| l.contains(k));
            attribute && !declares
        })
    };
    match language {
        "rust" => rust_test_marker(text, line, name)
            .ok()
            .flatten()
            .map(|_| name.to_string()),
        "swift" => (above().any(|l| l.starts_with("@Test")) || here.contains("@Test"))
            .then(|| name.to_string()),
        "cpp" => lines[at.saturating_sub(2)..=at]
            .iter()
            .rev()
            .find_map(|l| registration(l)),
        "python" => {
            if !bare.starts_with("test") {
                return None;
            }
            let indent = here.len() - here.trim_start().len();
            lines[..at]
                .iter()
                .rev()
                .find(|l| {
                    let t = l.trim_start();
                    t.starts_with("class ") && l.len() - t.len() < indent
                })
                .filter(|class| class.contains("TestCase"))
                .map(|_| name.to_string())
        }
        "typescript" => {
            let t = here.trim_start();
            if t.starts_with("it(")
                || t.starts_with("test(")
                || t.starts_with("it.only(")
                || t.starts_with("test.only(")
            {
                if let Some(rest) = t.split_once('(').map(|x| x.1.trim_start())
                    && let Some(quote) = rest
                        .chars()
                        .next()
                        .filter(|&c| c == '"' || c == '\'' || c == '`')
                    && let Some(desc) = rest[1..].split(quote).next()
                {
                    return Some(desc.to_string());
                }
                Some(name.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The test a gtest or Catch2 registration on `line` declares: `TEST(Suite, Name)` → `Suite.Name`,
/// `TEST_CASE("adds")` → `adds`.
pub fn registration(line: &str) -> Option<String> {
    let t = line.trim_start();
    for macro_name in ["TYPED_TEST_P", "TYPED_TEST", "TEST_F", "TEST_P", "TEST"] {
        if let Some(rest) = t.strip_prefix(macro_name)
            && let Some(args) = rest.trim_start().strip_prefix('(')
        {
            let args = args.split(')').next()?;
            let (suite, test) = args.split_once(',')?;
            let (suite, test) = (suite.trim(), test.trim());
            let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
            return (ok(suite) && ok(test)).then(|| format!("{suite}.{test}"));
        }
    }
    for macro_name in ["TEST_CASE", "SCENARIO"] {
        if let Some(rest) = t.strip_prefix(macro_name)
            && let Some(args) = rest.trim_start().strip_prefix('(')
        {
            let quoted = args.trim_start().strip_prefix('"')?;
            return quoted.split('"').next().map(str::to_string);
        }
    }
    None
}

/// Whether a caller is a test by name or file conventions of `language`.
pub fn looks_like_test(language: &str, name: &str, file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    // `SignalTests.testDoubles()` / `pkg.TestX`: judge the unqualified name.
    let name = name
        .split('(')
        .next()
        .unwrap_or(name)
        .rsplit('.')
        .next()
        .unwrap_or(name);
    match language {
        // Rust's test directories and conventional names contain ordinary helpers too. Rust
        // needs an analyzer flag or an outer test attribute; see `rust_test_marker`.
        "rust" => false,
        "go" => name.starts_with("Test") && lower.ends_with("_test.go"),
        "python" => {
            let base = lower.rsplit('/').next().unwrap_or("");
            name.starts_with("test")
                && (base.starts_with("test_")
                    || base.ends_with("_test.py")
                    || lower.contains("/tests/"))
        }
        // A test filename contains helpers that are not test-runner entry points. TypeScript
        // registrations are established by the analyzer's test flag or `test_marker` below.
        "typescript" => false,
        "swift" => name.starts_with("test") && lower.ends_with("tests.swift"),
        // A path containing `test` does not make every C++ helper a runnable test. Registered
        // gtest/Catch2 tests are recognized from their macros by `test_marker`.
        "cpp" => false,
        _ => name.to_ascii_lowercase().contains("test"),
    }
}

/// The command that runs only `tests` for `language`, when selection is possible.
pub fn test_command(
    language: &str,
    tools: &crate::verify::ProjectTools,
    tests: &[Symbol],
) -> Option<Vec<String>> {
    if tests.is_empty() {
        return None;
    }
    let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
    let go_name = |n: &str| n.split('.').next_back().unwrap_or(n).to_string();
    Some(match language {
        "rust" => {
            // rust-analyzer may qualify an integration test; libtest's filter sees the runnable
            // leaf name, not that analyzer container.
            let mut leaf_names: Vec<String> = names
                .iter()
                .map(|n| n.rsplit("::").next().unwrap_or(n).to_string())
                .collect();
            leaf_names.sort();
            leaf_names.dedup();
            if leaf_names.len() == 1 {
                vec![
                    "cargo".to_string(),
                    "test".to_string(),
                    "--workspace".to_string(),
                    "--".to_string(),
                    leaf_names.remove(0),
                ]
            } else if leaf_names.len() > 25 {
                vec![
                    "cargo".to_string(),
                    "test".to_string(),
                    "--workspace".to_string(),
                ]
            } else {
                let mut command = vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "for test_name in \"$@\"; do cargo test --workspace -- \"$test_name\" || exit; done"
                        .to_string(),
                    "prod-code-impact".to_string(),
                ];
                command.extend(leaf_names);
                command
            }
        }
        "go" => {
            // The packages that hold the tests, not `./...`: that builds every package's test
            // binary to run a filter most of them never match, 25 s against 0.7 s for one
            // package of a large module (#371).
            let mut packages: Vec<String> = tests
                .iter()
                .map(|t| {
                    let dir = std::path::Path::new(&t.file)
                        .parent()
                        .map(|d| d.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();
                    if dir.is_empty() {
                        ".".to_string()
                    } else {
                        format!("./{dir}")
                    }
                })
                .collect();
            packages.sort_unstable();
            packages.dedup();
            let mut c = vec!["go".to_string(), "test".to_string()];
            c.extend(packages);
            c.push("-run".to_string());
            c.push(format!(
                "^({})$",
                names
                    .iter()
                    .map(|n| go_name(n))
                    .collect::<Vec<_>>()
                    .join("|")
            ));
            c
        }
        "python" => {
            let mut c = crate::verify::plan_command_with(
                tools,
                "python",
                crate::verify::VerifyKind::Test,
                None,
            )
            .ok()?;
            // A test file pytest would not collect by its name (a `TestCase` in
            // `checks/check_price.py`) is collected when it is named on the command line.
            let mut files: Vec<&str> = tests.iter().map(|t| t.file.as_str()).collect();
            files.sort_unstable();
            files.dedup();
            c.extend(files.iter().map(|f| f.to_string()));
            c.push("-k".to_string());
            c.push(names.join(" or "));
            c
        }
        "typescript" => {
            let mut c = crate::verify::plan_command_with(
                tools,
                "typescript",
                crate::verify::VerifyKind::Test,
                None,
            )
            .ok()?;
            let selectable = c
                .iter()
                .any(|w| w == "vitest" || w == "jest" || w == "test");
            // Module-level test files are selected by path, named tests by pattern.
            let (files, named): (Vec<&str>, Vec<&str>) =
                names.iter().partition(|n| n.contains('/'));
            if selectable {
                c.extend(files.iter().map(|f| f.to_string()));
                if !named.is_empty() {
                    c.push("-t".to_string());
                    c.push(named.join("|"));
                }
            }
            c
        }
        "swift" => {
            let mut c = vec!["swift".to_string(), "test".to_string()];
            for n in &names {
                c.push("--filter".to_string());
                c.push(n.trim_end_matches("()").to_string());
            }
            c
        }
        // ctest selects by the registered name (`Suite.Name`, a Catch2 description).
        "cpp" => crate::verify::plan_command_with(
            tools,
            "cpp",
            crate::verify::VerifyKind::Test,
            Some(&format!("^({})$", names.join("|"))),
        )
        .ok()?,
        _ => return None,
    })
}

pub(crate) fn rel(root: &Path, uri: &str) -> String {
    let path = crate::remote_fs::uri_to_path(uri);
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    Path::new(&path)
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(path)
}

/// The language of a file in the workspace, judged by its extension or engine_project.
pub fn file_language(root: &Path, file: &str) -> Option<&'static str> {
    let p = Path::new(file);
    crate::sync::engine_for_file(p).or_else(|| {
        let abs = root.join(file);
        crate::sync::engine_project(root, &abs).1
    })
}

pub fn test_command_for_tests(
    root: &Path,
    default_language: &str,
    tools: &crate::verify::ProjectTools,
    tests: &[Symbol],
) -> Option<Vec<String>> {
    if tests.is_empty() {
        return None;
    }
    let root_language = crate::sync::expected_engine(root);
    let mut groups: BTreeMap<(String, String), Vec<Symbol>> = BTreeMap::new();
    for test in tests {
        let abs = root.join(&test.file);
        let (subpath, mut language) = crate::sync::engine_project(root, &abs);
        if let Some(own) = crate::sync::engine_for_file(&abs)
            && language.as_deref() == root_language.as_deref()
            && Some(own) != language.as_deref()
        {
            language = Some(own);
        }
        let language = language.unwrap_or(default_language).to_string();
        let subpath = subpath.unwrap_or_default();
        let mut test = test.clone();
        if !subpath.is_empty() {
            let project = Path::new(&subpath);
            if let Ok(relative) = Path::new(&test.file).strip_prefix(project) {
                test.file = relative.to_string_lossy().into_owned();
            }
        }
        groups.entry((subpath, language)).or_default().push(test);
    }

    let single_root_project = groups.len() == 1
        && groups
            .keys()
            .next()
            .is_some_and(|(subpath, _)| subpath.is_empty());
    let mut commands = Vec::with_capacity(groups.len());
    for ((subpath, language), project_tests) in groups {
        let project_tools = if subpath.is_empty() {
            None
        } else {
            Some(crate::verify::detect_tools(&root.join(&subpath)))
        };
        let project_tools = project_tools.as_ref().unwrap_or(tools);
        let command = test_command(&language, project_tools, &project_tests)?;
        if single_root_project {
            return Some(command);
        }
        let run = shell_words(&command);
        let dir = if subpath.is_empty() {
            ".".to_string()
        } else {
            subpath.clone()
        };
        commands.push(format!("(cd {} && {run})", shell_words(&[dir])));
    }
    Some(vec![
        "sh".to_string(),
        "-c".to_string(),
        commands.join(" && "),
        "prod-code-impact".to_string(),
    ])
}
