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
use serde_json::json;
use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Output};

fn reply(code: i32, text: &str) -> Output {
    Output {
        status: ExitStatus::from_raw(code << 8),
        stdout: text.as_bytes().to_vec(),
        stderr: b"fixture error".to_vec(),
    }
}

fn release(tag: &str) -> String {
    json!({"tag_name":tag,"assets":[{"name":format!("prod-code-{}", current_target().unwrap()),"url":"https://example.invalid/binary"}]}).to_string()
}

#[test]
fn fetch_normalizes_gh_and_uses_rest_when_gh_is_unusable() {
    let mut commands = vec![];
    let result = fetch_release_info_with(Some("v2.0.0"), &mut |cmd| {
        commands.push((
            cmd.get_program().to_string_lossy().into_owned(),
            cmd.get_args()
                .map(|s| s.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        ));
        Ok(reply(0, r#"{"tagName":"v2.0.0","assets":[]}"#))
    })
    .unwrap();
    assert_eq!(
        result,
        json!({"tagName":"v2.0.0","tag_name":"v2.0.0","assets":[]})
    );
    assert_eq!(
        commands,
        vec![(
            "gh".into(),
            vec![
                "release",
                "view",
                "v2.0.0",
                "--repo",
                REPOSITORY,
                "--json",
                "tagName,assets"
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        )]
    );
    for (tag, first) in [
        (None, Some(reply(1, ""))),
        (Some("v2"), Some(reply(0, "invalid"))),
        (None, None),
    ] {
        let mut first = first;
        let mut seen = vec![];
        let actual = fetch_release_info_with(tag, &mut |cmd| {
            seen.push(cmd.get_program().to_string_lossy().into_owned());
            if seen.len() == 1 {
                return first
                    .take()
                    .ok_or_else(|| std::io::Error::other("missing gh"));
            }
            let expected_url = format!(
                "https://api.github.com/repos/{REPOSITORY}/releases/{}",
                tag.map(|t| format!("tags/{t}")).unwrap_or("latest".into())
            );
            assert_eq!(cmd.get_args().last().unwrap(), expected_url.as_str());
            Ok(reply(0, r#"{"tag_name":"v2","assets":[]}"#))
        })
        .unwrap();
        assert_eq!(seen, vec!["gh", "curl"]);
        assert_eq!(actual, json!({"tag_name":"v2","assets":[]}));
    }
}

#[test]
fn fetch_reports_curl_spawn_http_and_json_failures() {
    for (kind, expected) in [
        (0, "failed to run curl to check releases"),
        (
            1,
            "failed to fetch release from https://api.github.com/repos/alex09x/prod-code/releases/latest: fixture error",
        ),
        (2, "failed to parse GitHub release JSON"),
    ] {
        let error = fetch_release_info_with(None, &mut |cmd| {
            if cmd.get_program() == "gh" {
                return Ok(reply(1, ""));
            }
            match kind {
                0 => Err(std::io::Error::other("spawn error")),
                1 => Ok(reply(1, "")),
                _ => Ok(reply(0, "invalid")),
            }
        })
        .unwrap_err();
        assert_eq!(error.to_string(), expected);
    }
}

#[test]
fn release_parser_refuses_malformed_fields_and_accepts_api_url() {
    for (input, error) in [
        (json!({}), "missing tag_name in release"),
        (json!({"tag_name":"v2"}), "missing assets list in release"),
        (
            json!({"tag_name":"v2","assets":[]}),
            "asset `prod-code-target` not found in release `v2`",
        ),
        (
            json!({"tag_name":"v2","assets":[{"name":"prod-code-target"}]}),
            "missing download url for asset",
        ),
    ] {
        assert_eq!(
            parse_release_asset(&input, "target")
                .unwrap_err()
                .to_string(),
            error
        );
    }
    assert_eq!(parse_release_asset(&json!({"tag_name":"v2","assets":[{"name":"other"},{"name":"prod-code-target","url":"fixture"}]}),"target").unwrap(), ("v2".into(), "fixture".into()));
}

#[test]
fn update_check_skip_force_and_install_use_only_the_supplied_executable() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = tmp.path().join("prod-code");
    std::fs::write(&exe, b"original").unwrap();
    let mut no_download =
        |_: &mut Command| -> std::io::Result<ExitStatus> { panic!("must not install") };
    for (check, tag, forced, specific) in [
        (true, "v99.0.0", false, None),
        (false, "v0.0.0", false, None),
        (true, "v0.0.0", true, None),
        (true, "v0.0.0", false, Some("v0.0.0".into())),
    ] {
        run_update_with(
            check,
            forced,
            specific,
            &exe,
            &mut |_| Ok(reply(0, &release(tag))),
            &mut no_download,
        )
        .unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"original");
    }
    run_update_with(
        false,
        false,
        None,
        Path::new("/opt/homebrew/bin/prod-code"),
        &mut |_| panic!("managed binary must skip fetch"),
        &mut no_download,
    )
    .unwrap();
    run_update_with(
        false,
        false,
        None,
        &exe,
        &mut |_| Ok(reply(0, &release("v99.0.0"))),
        &mut |cmd| {
            assert_eq!(cmd.get_program(), "curl");
            let args: Vec<_> = cmd.get_args().collect();
            assert_eq!(&args[..3], &["-fL", "--progress-bar", "-o"]);
            assert_eq!(args[4], "https://example.invalid/binary");
            std::fs::write(args[3], b"replacement")?;
            Ok(ExitStatus::from_raw(0))
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), b"replacement");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
}

#[test]
fn failed_download_is_removed_and_preserves_the_executable() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = tmp.path().join("prod-code");
    std::fs::write(&exe, b"original").unwrap();
    for spawn_error in [false, true] {
        let error = run_update_with(
            false,
            false,
            None,
            &exe,
            &mut |_| Ok(reply(0, &release("v99.0.0"))),
            &mut |cmd| {
                if spawn_error {
                    return Err(std::io::Error::other("no curl"));
                }
                std::fs::write(cmd.get_args().nth(3).unwrap(), b"partial")?;
                Ok(ExitStatus::from_raw(22 << 8))
            },
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            if spawn_error {
                "failed to download update binary with curl"
            } else {
                "download failed with status exit status: 22"
            }
        );
        assert_eq!(std::fs::read(&exe).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }
}
