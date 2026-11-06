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

fn parse(text: &str) -> RunConfig {
    toml::from_str(text).expect("the settings parse")
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn no_file_and_no_keys_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(RunConfig::load(dir.path()), RunConfig::default());
    // The Rust analysis options in the same file are not run settings.
    std::fs::write(
        dir.path().join("prod-code.toml"),
        "[rust]\nfeatures = \"all\"\nsysroot = true\n[cluster]\npin = \"auto\"\n",
    )
    .unwrap();
    assert_eq!(RunConfig::load(dir.path()), RunConfig::default());
    std::fs::write(dir.path().join("prod-code.toml"), "this is = not [toml").unwrap();
    assert_eq!(RunConfig::load(dir.path()), RunConfig::default());
}

#[test]
fn a_project_names_its_command_and_where_the_filter_goes() {
    let config = parse(
        "[commands.python]\ntest = [\"python3\", \"-m\", \"pytest\", \"-q\", \"{filter}\"]\ncheck = [\"mypy\", \".\"]\n",
    );
    assert!(config.has_command("python", VerifyKind::Test));
    assert!(!config.has_command("python", VerifyKind::Lint));
    assert!(!config.has_command("rust", VerifyKind::Test));
    assert_eq!(
        config
            .command_for("python", VerifyKind::Test, Some("price"))
            .unwrap(),
        Some(words(&["python3", "-m", "pytest", "-q", "price"]))
    );
    // With no filter the argument that places it is left out.
    assert_eq!(
        config
            .command_for("python", VerifyKind::Test, None)
            .unwrap(),
        Some(words(&["python3", "-m", "pytest", "-q"]))
    );
    assert_eq!(
        config
            .command_for("python", VerifyKind::Check, None)
            .unwrap(),
        Some(words(&["mypy", "."]))
    );
    assert_eq!(
        config
            .command_for("python", VerifyKind::Lint, None)
            .unwrap(),
        None
    );
}

#[test]
fn a_filter_the_command_has_no_place_for_is_refused_not_dropped() {
    let config = parse("[commands.python]\ncheck = [\"mypy\", \".\"]\ntest = []\n");
    let error = config
        .command_for("python", VerifyKind::Check, Some("price"))
        .unwrap_err();
    assert!(error.to_string().contains("no {filter}"), "{error:#}");
    let error = config
        .command_for("python", VerifyKind::Test, None)
        .unwrap_err();
    assert!(error.to_string().contains("is empty"), "{error:#}");
}

#[test]
fn the_rust_runner_turns_cargo_commands_into_mitts_ones() {
    let config = parse("[rust]\nrunner = \"mitts\"\n");
    let plan = |args: &[&str]| config.finish("rust", false, words(args)).unwrap();
    assert_eq!(
        plan(&[
            "cargo",
            "check",
            "--workspace",
            "--all-targets",
            "--message-format=json"
        ]),
        words(&[
            "mitts",
            "check",
            "--workspace",
            "--all-targets",
            "--message-format=json"
        ])
    );
    assert_eq!(
        plan(&["cargo", "clippy", "--workspace", "--", "-D", "warnings"]),
        words(&["mitts", "clippy", "--workspace", "--", "-D", "warnings"])
    );
    assert_eq!(
        plan(&["cargo", "test", "--workspace"]),
        words(&["mitts", "test", "--workspace"])
    );
    // The filter moves from after `--` to a test name.
    assert_eq!(
        plan(&["cargo", "test", "-p", "demo", "--", "price"]),
        words(&["mitts", "test", "-p", "demo", "price"])
    );
    // Commands Mitts is not asked to run stay as they are.
    assert_eq!(
        plan(&["cargo", "bench", "--workspace"]),
        words(&["cargo", "bench", "--workspace"])
    );
    assert_eq!(
        plan(&["go", "test", "./..."]),
        words(&["go", "test", "./..."])
    );
}

#[test]
fn a_test_argument_mitts_would_drop_is_refused() {
    let config = parse("[rust]\nrunner = \"mitts\"\n");
    for tail in [
        &["--exact", "price"][..],
        &["price", "cost"][..],
        &["--nocapture"][..],
    ] {
        let mut command = words(&["cargo", "test", "--workspace", "--"]);
        command.extend(tail.iter().map(|item| item.to_string()));
        let error = config.finish("rust", false, command).unwrap_err();
        assert!(
            error.to_string().contains("takes one test name"),
            "{error:#}"
        );
    }
}

#[test]
fn cargo_is_the_default_and_a_wrong_runner_is_named() {
    let planned = words(&["cargo", "test", "--workspace"]);
    for config in [RunConfig::default(), parse("[rust]\nrunner = \"cargo\"\n")] {
        assert_eq!(
            config.finish("rust", false, planned.clone()).unwrap(),
            planned
        );
    }
    let error = parse("[rust]\nrunner = \"mits\"\n")
        .finish("rust", false, planned)
        .unwrap_err();
    assert!(
        error.to_string().contains("\"mits\" is not a runner"),
        "{error:#}"
    );
}

#[test]
fn the_toolchain_is_set_for_rust_commands_only_and_a_custom_command_keeps_it() {
    let config = parse("[rust]\ntoolchain = \"1.97.1\"\n");
    assert_eq!(
        config
            .finish("rust", false, words(&["cargo", "test"]))
            .unwrap(),
        words(&["env", "RUSTUP_TOOLCHAIN=1.97.1", "cargo", "test"])
    );
    // The project's own command is run as written, under the toolchain it asked for.
    let both = parse("[rust]\ntoolchain = \"nightly\"\nrunner = \"mitts\"\n");
    assert_eq!(
        both.finish("rust", true, words(&["cargo", "test", "--", "x", "y"]))
            .unwrap(),
        words(&[
            "env",
            "RUSTUP_TOOLCHAIN=nightly",
            "cargo",
            "test",
            "--",
            "x",
            "y"
        ])
    );
    assert_eq!(
        config.finish("go", false, words(&["go", "test"])).unwrap(),
        words(&["go", "test"])
    );
    let error = parse("[rust]\ntoolchain = \"  \"\n")
        .finish("rust", false, words(&["cargo", "test"]))
        .unwrap_err();
    assert!(error.to_string().contains("toolchain"), "{error:#}");
}
