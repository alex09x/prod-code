/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{WarmupConfig, When};

fn parse(text: &str) -> Result<WarmupConfig, toml::de::Error> {
    toml::from_str(text)
}

#[test]
fn without_a_section_the_defaults_are_careful() {
    let config = WarmupConfig::default();
    assert!(config.after_restart);
    assert_eq!(config.max_workspaces, 4);
    assert_eq!(config.max_concurrent, 1);
    assert_eq!(config.max_age_hours, 24);
    assert_eq!(config.skip_above_load_percent, 80);
    assert!(config.validate().is_ok());
    assert!(config.loads("rust") && config.loads("swift"));
}

#[test]
fn a_section_sets_what_it_names_and_keeps_the_rest() {
    let config = parse(
        "max_workspaces = 8\nskip_above_load_percent = 150\n[languages.swift]\nwhen = \"never\"\n[languages.rust]\nwhen = \"restart\"\n",
    )
    .unwrap();
    assert_eq!(config.max_workspaces, 8);
    assert_eq!(config.max_concurrent, 1, "not named, so the default");
    assert!(config.after_restart);
    assert!(!config.loads("swift"));
    assert!(config.loads("rust"));
    assert!(config.loads("go"), "a language not listed is loaded");
    assert_eq!(config.languages["swift"].when, When::Never);
    assert!(config.validate().is_ok());
}

#[test]
fn a_key_that_is_not_known_is_an_error_naming_it() {
    let error = parse("max_workspace = 8\n").unwrap_err().to_string();
    assert!(error.contains("max_workspace"), "{error}");
    let error = parse("[languages.rust]\nwhen = \"restart\"\nwarm_command = [\"x\"]\n")
        .unwrap_err()
        .to_string();
    assert!(error.contains("warm_command"), "{error}");
    let error = parse("[languages.rust]\nwhen = \"sometimes\"\n")
        .unwrap_err()
        .to_string();
    assert!(error.contains("sometimes"), "{error}");
}

#[test]
fn values_out_of_range_are_refused_with_the_key() {
    for (text, key) in [
        (
            "skip_above_load_percent = 0\n",
            "warmup.skip_above_load_percent",
        ),
        (
            "skip_above_load_percent = 900\n",
            "warmup.skip_above_load_percent",
        ),
        ("max_concurrent = 0\n", "warmup.max_concurrent"),
        ("max_age_hours = 0\n", "warmup.max_age_hours"),
        ("[languages.\" \"]\nwhen = \"never\"\n", "warmup.languages"),
        (
            "[languages.swift]\nprebuild = [\"\"]\n",
            "warmup.languages.swift.prebuild",
        ),
        (
            "[languages.swift]\nprebuild = [\"swift\"]\nbudget_secs = 0\n",
            "warmup.languages.swift.budget_secs",
        ),
        (
            "[languages.swift]\nprebuild = [\"swift\"]\nbudget_secs = 7200\n",
            "warmup.languages.swift.budget_secs",
        ),
    ] {
        let error = parse(text).unwrap().validate().unwrap_err();
        assert!(error.contains(key), "{text}: {error}");
    }
}

/// The command run in a workspace after it was loaded again: an argument vector, no shell, with a
/// budget that defaults to 600 s (#1976).
#[test]
fn a_language_can_name_a_prebuild_command_and_its_budget() {
    let config = parse(
        "[languages.swift]\nprebuild = [\"swift\", \"build\", \"--build-tests\"]\nbudget_secs = 900\n[languages.rust]\nwhen = \"restart\"\n[languages.go]\nprebuild = [\"go\", \"build\", \"./...\"]\n",
    )
    .unwrap();
    assert!(config.validate().is_ok());
    let (command, budget) = config.prebuild("swift").expect("swift has one");
    assert_eq!(command, ["swift", "build", "--build-tests"]);
    assert_eq!(budget, std::time::Duration::from_secs(900));
    // The default budget, and a language with no command, and one that is not listed.
    assert_eq!(
        config.prebuild("go").unwrap().1,
        std::time::Duration::from_secs(600)
    );
    assert!(config.prebuild("rust").is_none());
    assert!(config.prebuild("python").is_none());
    // A key that is not known is still an error that names it.
    let error = parse("[languages.swift]\nprebuilt = [\"x\"]\n")
        .unwrap_err()
        .to_string();
    assert!(error.contains("prebuilt"), "{error}");
}
