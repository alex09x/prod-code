/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The command a project names in `prod-code.toml` reaches the plan of `code_test`,
//! `code_check` and `code_lint` (#1961).

use super::super::VerifyKind;
use super::super::plan::scoped::plan_scoped;

fn rust_project(config: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    if let Some(config) = config {
        std::fs::write(dir.path().join("prod-code.toml"), config).unwrap();
    }
    dir
}

fn plan(dir: &tempfile::TempDir, kind: VerifyKind, filter: Option<&str>) -> Vec<String> {
    plan_scoped(dir.path(), None, kind, filter)
        .expect("the plan is made")
        .command
}

#[test]
fn without_settings_the_plan_is_what_it_was() {
    let dir = rust_project(None);
    assert_eq!(
        plan(&dir, VerifyKind::Test, None),
        ["cargo", "test", "--workspace"]
    );
    // Settings for the engine alone are not run settings.
    let dir = rust_project(Some("[rust]\nfeatures = \"all\"\n"));
    assert_eq!(plan(&dir, VerifyKind::Check, None)[..2], ["cargo", "check"]);
}

#[test]
fn the_mitts_runner_and_the_toolchain_reach_check_lint_and_test() {
    let dir = rust_project(Some("[rust]\nrunner = \"mitts\"\ntoolchain = \"1.97.1\"\n"));
    assert_eq!(
        plan(&dir, VerifyKind::Test, None),
        [
            "env",
            "RUSTUP_TOOLCHAIN=1.97.1",
            "mitts",
            "test",
            "--workspace"
        ]
    );
    let check = plan(&dir, VerifyKind::Check, None);
    assert_eq!(
        check[..4],
        ["env", "RUSTUP_TOOLCHAIN=1.97.1", "mitts", "check"]
    );
    assert!(
        check.contains(&"--message-format=json".to_string()),
        "{check:?}"
    );
    let lint = plan(&dir, VerifyKind::Lint, None);
    assert_eq!(lint[2..4], ["mitts", "clippy"]);
    // A filter is a test name for Mitts.
    assert_eq!(
        plan(&dir, VerifyKind::Test, Some("price")).last().unwrap(),
        "price"
    );
    assert!(!plan(&dir, VerifyKind::Test, Some("price")).contains(&"--".to_string()));
}

#[test]
fn a_command_the_project_wrote_is_run_as_written() {
    let dir = rust_project(Some(
        "[commands.rust]\ntest = [\"make\", \"unit\", \"FILTER={filter}\"]\ncheck = [\"cargo\", \"check\", \"--workspace\", \"--features\", \"fast\"]\n",
    ));
    assert_eq!(
        plan(&dir, VerifyKind::Test, Some("price")),
        ["make", "unit", "FILTER=price"]
    );
    // No filter: the argument that places it is left out.
    assert_eq!(plan(&dir, VerifyKind::Test, None), ["make", "unit"]);
    // A scope hint does not rewrite the project's own `--workspace` into a package selection
    // (the planner does that to the Cargo commands it writes itself).
    let hint = dir.path().join("src/lib.rs");
    let hinted = plan_scoped(dir.path(), Some(&hint), VerifyKind::Check, None)
        .unwrap()
        .command;
    assert_eq!(
        hinted,
        ["cargo", "check", "--workspace", "--features", "fast"]
    );
    // The commands it does not name stay planned, and are narrowed as they always were.
    let lint = plan_scoped(dir.path(), Some(&hint), VerifyKind::Lint, None)
        .unwrap()
        .command;
    assert_eq!(lint[..2], ["cargo", "clippy"]);
    assert!(lint.contains(&"-p".to_string()) && !lint.contains(&"--workspace".to_string()));
}

#[test]
fn a_mistake_in_the_settings_is_an_error_not_a_silent_default() {
    let dir = rust_project(Some("[rust]\nrunner = \"mits\"\n"));
    let error = plan_scoped(dir.path(), None, VerifyKind::Test, None)
        .err()
        .expect("a runner that is not one is refused");
    assert!(
        format!("{error:#}").contains("is not a runner"),
        "{error:#}"
    );

    let dir = rust_project(Some("[commands.rust]\ncheck = [\"true\"]\n"));
    let error = plan_scoped(dir.path(), None, VerifyKind::Check, Some("x"))
        .err()
        .expect("a filter with no place is refused");
    assert!(format!("{error:#}").contains("no {filter}"), "{error:#}");
}

#[test]
fn a_python_project_names_its_test_command() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("prod-code.toml"),
        "[commands.python]\ntest = [\"python3\", \"-m\", \"pytest\", \"-q\", \"-k\", \"{filter}\"]\n",
    )
    .unwrap();
    assert_eq!(
        plan(&dir, VerifyKind::Test, Some("price")),
        ["python3", "-m", "pytest", "-q", "-k", "price"]
    );
}
