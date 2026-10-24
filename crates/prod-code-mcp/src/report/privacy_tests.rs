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
use std::process::Command;

use super::{Outcome, ReportRequest, report};

const BODY: &str =
    "A neutral reproduction returned no results instead of the expected declaration.";

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn checkout() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/sample_record.rs"),
        "pub struct SampleRecord;\npub trait SampleTrait {}\npub fn sample_action() {}\nmod sample_module;\nconst SAMPLE_LIMIT: usize = 1;\nfn read() {}\nfn get() {}\n").unwrap();
    git(dir.path(), &["add", "src/sample_record.rs"]);
    dir
}

async fn attempt(
    root: &Path,
    title: &str,
    body: &str,
    dry_run: bool,
    allow: bool,
) -> anyhow::Result<Outcome> {
    report(
        None,
        ReportRequest {
            root,
            title,
            body,
            force: true,
            dry_run,
            allow_private_names: allow,
            private_ref: None,
            labels: &[],
        },
        &root.join("missing-gh"),
    )
    .await
}

#[tokio::test]
async fn report_private_names_are_refused_before_gh() {
    let dir = checkout();
    let absolute = dir
        .path()
        .join("src/sample_record.rs")
        .display()
        .to_string();
    for token in [
        "src/sample_record.rs",
        "sample_record.rs",
        "sample_record",
        "SampleRecord",
        "sample_action",
        "SampleTrait",
        "sample_module",
        "SAMPLE_LIMIT",
        &absolute,
    ] {
        for dry_run in [false, true] {
            let body = format!("{BODY} The command mentioned `{token}`.");
            let error = attempt(
                dir.path(),
                "A lookup returned no results",
                &body,
                dry_run,
                false,
            )
            .await
            .unwrap_err()
            .to_string();
            assert!(error.contains(token), "missing {token}: {error}");
            assert!(error.contains("neutral reproduction"), "{error}");
            assert!(error.contains("private_ref"), "{error}");
        }
    }
}

#[tokio::test]
async fn report_lists_all_tokens_checks_titles_and_allows_neutral_reports() {
    let dir = checkout();
    let error = attempt(
        dir.path(),
        "SampleRecord lookup fails",
        &format!("{BODY} sample_action src/sample_record.rs"),
        true,
        false,
    )
    .await
    .unwrap_err()
    .to_string();
    for token in [
        "SampleRecord",
        "sample_action",
        "src/sample_record.rs",
        "sample_record.rs",
        "sample_record",
    ] {
        assert!(error.contains(token), "missing {token}: {error}");
    }
    let error = attempt(
        dir.path(),
        "SampleRecord-based lookup fails",
        BODY,
        true,
        false,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("`SampleRecord`"), "{error}");
    let body = format!("{BODY} read get Result String default function return struct value.");
    assert!(matches!(
        attempt(
            dir.path(),
            "A lookup returned no results",
            &body,
            true,
            false
        )
        .await
        .unwrap(),
        Outcome::DryRun(_)
    ));
    // Longer identifiers must not be mistaken for one declared in the checkout.
    let body = format!("{BODY} SampleRecords sample_actions sample_record_backup.rs");
    assert!(
        attempt(
            dir.path(),
            "A lookup returned no results",
            &body,
            true,
            false
        )
        .await
        .is_ok()
    );
}

#[tokio::test]
async fn report_explicit_override_and_public_remote_keep_existing_behavior() {
    let dir = checkout();
    let body = format!("{BODY} SampleRecord src/sample_record.rs");
    assert!(
        attempt(
            dir.path(),
            "A lookup returned no results",
            &body,
            true,
            true
        )
        .await
        .is_ok()
    );
    for remote in [
        "https://github.com/alex09x/prod-code.git",
        "git@github.com:alex09x/prod-code.git",
        "ssh://git@github.com/alex09x/prod-code",
        "ssh://git@github.com:22/alex09x/prod-code.git",
    ] {
        git(dir.path(), &["config", "remote.origin.url", remote]);
        assert!(
            attempt(
                dir.path(),
                "A lookup returned no results",
                &body,
                true,
                false
            )
            .await
            .is_ok(),
            "{remote}"
        );
    }
    for remote in [
        "https://github.com/example/prod-code.git",
        "https://github.com/alex09x/prod-code-extra.git",
        "https://example.com/alex09x/prod-code.git",
        "https://github.com/alex09x/prod-code.git.git",
    ] {
        git(dir.path(), &["config", "remote.origin.url", remote]);
        assert!(
            attempt(
                dir.path(),
                "A lookup returned no results",
                &body,
                true,
                false
            )
            .await
            .is_err(),
            "{remote}"
        );
    }
}

#[tokio::test]
async fn report_collects_declarations_across_languages_without_an_analyzer() {
    let dir = checkout();
    let sources = [
        (
            "sample_go.go",
            "type SampleGo struct {}\nfunc (s SampleGo) sampleGoAction() {}",
            "SampleGo",
            "sampleGoAction",
        ),
        (
            "sample_python.py",
            "class SamplePython:\n    def sample_python_action(self): pass",
            "SamplePython",
            "sample_python_action",
        ),
        (
            "sample_script.ts",
            "interface SampleScript {}\nconst sampleScriptAction = () => {};",
            "SampleScript",
            "sampleScriptAction",
        ),
        (
            "sample_swift.swift",
            "struct SampleSwift {}\nfunc sampleSwiftAction() {}",
            "SampleSwift",
            "sampleSwiftAction",
        ),
        (
            "sample_native.cpp",
            "class SampleNative {};\nvoid sampleNativeAction() {}",
            "SampleNative",
            "sampleNativeAction",
        ),
    ];
    for (file, source, _, _) in &sources {
        std::fs::write(dir.path().join(file), source).unwrap();
        git(dir.path(), &["add", file]);
    }
    for (_, _, kind, function) in sources {
        for name in [kind, function] {
            let error = attempt(
                dir.path(),
                "A lookup returned no results",
                &format!("{BODY} {name}"),
                true,
                false,
            )
            .await
            .unwrap_err()
            .to_string();
            assert!(error.contains(&format!("`{name}`")), "{error}");
        }
    }
}

#[tokio::test]
async fn report_checks_private_reference_and_schema_explains_the_policy() {
    let dir = checkout();
    let result = report(
        None,
        ReportRequest {
            root: dir.path(),
            title: "A lookup returned no results",
            body: BODY,
            force: false,
            dry_run: true,
            allow_private_names: false,
            private_ref: Some("sample_action"),
            labels: &[],
        },
        &dir.path().join("missing-gh"),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(result.contains("`sample_action`"), "{result}");
    let schema = crate::tools::schemas::utility::utility_tools()
        .into_iter()
        .find(|tool| tool.name == "code_report_issue")
        .unwrap();
    assert!(schema.description.starts_with("PUBLIC REPORT:"));
    assert!(schema.description.contains("comments"));
    assert_eq!(
        schema.input_schema["properties"]["allow_private_names"]["default"],
        false
    );
}

#[tokio::test]
async fn report_directory_name_is_not_a_public_repository_identity() {
    let dir = checkout();
    let nested = dir.path().join("prod-code");
    std::fs::create_dir(&nested).unwrap();
    git(&nested, &["init", "-q"]);
    git(
        &nested,
        &[
            "config",
            "remote.origin.url",
            "https://github.com/example/prod-code.git",
        ],
    );
    std::fs::write(nested.join("sample_record.rs"), "struct SampleRecord;").unwrap();
    git(&nested, &["add", "sample_record.rs"]);
    let error = attempt(
        &nested,
        "SampleRecord lookup returned no results",
        BODY,
        true,
        false,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("`SampleRecord`"), "{error}");
}

#[tokio::test]
async fn report_only_scans_tracked_sources_and_caps_individual_reads() {
    let dir = checkout();
    std::fs::write(dir.path().join("untracked.rs"), "struct UntrackedRecord;").unwrap();
    std::fs::write(
        dir.path().join("oversized.rs"),
        " ".repeat(256 * 1024 + 1) + "\nstruct OversizedRecord;",
    )
    .unwrap();
    git(dir.path(), &["add", "oversized.rs"]);
    let body = format!("{BODY} UntrackedRecord OversizedRecord");
    assert!(
        attempt(
            dir.path(),
            "A lookup returned no results",
            &body,
            true,
            false
        )
        .await
        .is_ok()
    );
    // Even when contents are oversized, its filename is still protected.
    let body = format!("{BODY} oversized.rs");
    let error = attempt(
        dir.path(),
        "A lookup returned no results",
        &body,
        true,
        false,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("`oversized.rs`"), "{error}");
}
