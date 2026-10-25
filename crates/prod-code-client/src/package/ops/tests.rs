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

fn status<'a>(exe: &'a Path, up_to_date: bool) -> LocalStatus<'a> {
    LocalStatus {
        exe,
        pkg_type: PackageType::CargoBin,
        version: "1.2.3",
        latest_tag: "v1.3.0",
        is_up_to_date: up_to_date,
        target: "x86_64-unknown-linux-gnu",
        sha256: "abc123",
    }
}

#[test]
fn status_json_has_every_field() {
    let exe = Path::new("/home/u/.cargo/bin/prod-code");
    assert_eq!(
        status(exe, false).to_json(),
        json!({
            "executable_path": "/home/u/.cargo/bin/prod-code",
            "package_type": "Cargo Binary (~/.cargo/bin)",
            "version": "1.2.3",
            "latest_release": "v1.3.0",
            "is_up_to_date": false,
            "target": "x86_64-unknown-linux-gnu",
            "sha256": "abc123",
        })
    );
}

#[test]
fn status_text_reports_update_available_or_up_to_date() {
    let exe = Path::new("/home/u/.cargo/bin/prod-code");
    let expected = "⚡ prod-code Package Status\n\
────────────────────────────────────────────────────\n\
Local Client:\n\
\x20 Path:          /home/u/.cargo/bin/prod-code\n\
\x20 Package Type:  Cargo Binary (~/.cargo/bin)\n\
\x20 Version:       v1.2.3 (update available)\n\
\x20 Platform:      x86_64-unknown-linux-gnu\n\
\x20 SHA-256:       abc123\n\
\x20 Latest Tag:    v1.3.0\n\
\n\
Fleet Orchestration:\n\
\x20 To verify release cryptographic integrity: prod-code package verify\n\
\x20 To update local packages to latest:        prod-code package install\n\
\x20 To inspect cluster node versions:          prod-code package sync\n";
    assert_eq!(status(exe, false).to_text(), expected);
    assert_eq!(
        status(exe, true).to_text(),
        expected.replace("(update available)", "(up to date)")
    );
}

#[test]
fn verification_report_covers_match_mismatch_and_missing_asset() {
    let mut sums = HashMap::new();
    sums.insert("prod-code-t".to_string(), "abcdef".to_string());
    sums.insert("other".to_string(), "00".to_string());
    let ok = "  Remote: abcdef (prod-code-t)\n\n✓ Package integrity verified: binary matches official release checksum.";
    assert_eq!(verification_report("abcdef", &sums, "prod-code-t"), ok);
    // Hex case does not matter when comparing.
    assert_eq!(verification_report("ABCDEF", &sums, "prod-code-t"), ok);
    assert_eq!(
        verification_report("123456", &sums, "prod-code-t"),
        "  Remote: abcdef (prod-code-t)\n\n⚠ Checksum mismatch: binary has been modified or locally built."
    );
    assert_eq!(
        verification_report("123456", &sums, "prod-code-missing"),
        "Note: Official release manifest contains 2 assets.\n✓ Local hash computed: 123456"
    );
    assert_eq!(
        verification_report("1", &HashMap::new(), "x"),
        "Note: Official release manifest contains 0 assets.\n✓ Local hash computed: 1"
    );
}

#[test]
fn install_dir_prefers_existing_local_bin_then_cargo_bin() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path().to_path_buf();
    // Neither exists: default to ~/.local/bin.
    assert_eq!(
        install_dir_for(false, Some(h.clone())),
        h.join(".local/bin")
    );
    std::fs::create_dir_all(h.join(".cargo/bin")).unwrap();
    assert_eq!(
        install_dir_for(false, Some(h.clone())),
        h.join(".cargo/bin")
    );
    std::fs::create_dir_all(h.join(".local/bin")).unwrap();
    assert_eq!(
        install_dir_for(false, Some(h.clone())),
        h.join(".local/bin")
    );
    // --system and a missing HOME both fall back to /usr/local/bin.
    assert_eq!(
        install_dir_for(true, Some(h)),
        PathBuf::from("/usr/local/bin")
    );
    assert_eq!(
        install_dir_for(false, None),
        PathBuf::from("/usr/local/bin")
    );
}

#[cfg(unix)]
#[test]
fn download_and_install_places_executable_and_replaces_old_binary() {
    use std::os::unix::fs::PermissionsExt;
    let src = tempfile::tempdir().unwrap();
    let asset = src.path().join("asset");
    std::fs::write(&asset, "#!/bin/sh\necho installed-fake\n").unwrap();
    let url = format!("file://{}", asset.display());

    let dest = tempfile::tempdir().unwrap();
    let install_dir = dest.path().join("nested/bin");
    let bin = download_and_install(&url, &install_dir).unwrap();
    assert_eq!(bin, install_dir.join("prod-code"));
    assert_eq!(
        std::fs::read_to_string(&bin).unwrap(),
        "#!/bin/sh\necho installed-fake\n"
    );
    assert_eq!(
        std::fs::metadata(&bin).unwrap().permissions().mode() & 0o777,
        0o755
    );

    // A second install replaces the first and leaves no temp file behind.
    std::fs::write(&asset, "#!/bin/sh\necho second\n").unwrap();
    download_and_install(&url, &install_dir).unwrap();
    assert_eq!(
        std::fs::read_to_string(&bin).unwrap(),
        "#!/bin/sh\necho second\n"
    );
    let names: Vec<_> = std::fs::read_dir(&install_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["prod-code".to_string()]);
}

#[test]
fn download_failure_is_reported_and_leaves_nothing_behind() {
    let dest = tempfile::tempdir().unwrap();
    let missing = dest.path().join("no-such-asset");
    let err =
        download_and_install(&format!("file://{}", missing.display()), dest.path()).unwrap_err();
    assert!(
        err.to_string().starts_with("download failed with status"),
        "{err}"
    );
    assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
}

#[test]
fn install_fails_with_context_when_target_is_a_non_empty_directory() {
    let src = tempfile::tempdir().unwrap();
    let asset = src.path().join("asset");
    std::fs::write(&asset, "x").unwrap();
    let dest = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dest.path().join("prod-code/keep")).unwrap();
    let err =
        download_and_install(&format!("file://{}", asset.display()), dest.path()).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "failed to move binary into {}",
            dest.path().join("prod-code").display()
        )
    );
}

#[test]
fn render_node_covers_down_busy_active_idle_and_defaults() {
    let cases = [
        (
            json!({"remote": "10.0.0.1:9400", "up": false, "platform": "linux"}),
            "  Node: 10.0.0.1:9400          🔴 DOWN (linux)",
        ),
        (
            json!({"remote": "10.0.0.2:9400", "up": true, "platform": "linux",
                   "active_sessions": 3, "running_commands": [{"id": 1}, {"id": 2}]}),
            "  Node: 10.0.0.2:9400          🟡 BUSY (commands in-flight)\n        Platform: linux | Sessions: 3 | Running commands: 2",
        ),
        (
            json!({"remote": "n3", "up": true, "platform": "macos", "active_sessions": 1,
                   "running_commands": []}),
            "  Node: n3                     🟢 ACTIVE (sessions connected)\n        Platform: macos | Sessions: 1 | Running commands: 0",
        ),
        (
            json!({"remote": "n4", "up": true, "platform": "linux"}),
            "  Node: n4                     ⚪ IDLE (ready for upgrade)\n        Platform: linux | Sessions: 0 | Running commands: 0",
        ),
        // Missing fields: unknown remote, not up, platform "-".
        (json!({}), "  Node: unknown                🔴 DOWN (-)"),
    ];
    for (node, expected) in cases {
        assert_eq!(render_node(&node), expected, "{node}");
    }
}

#[test]
fn render_nodes_lists_every_node_and_tolerates_missing_array() {
    let cluster = json!({"nodes": [{"remote": "a", "up": false}, {"remote": "b", "up": false}]});
    assert_eq!(
        render_nodes(&cluster),
        vec![
            "  Node: a                      🔴 DOWN (-)".to_string(),
            "  Node: b                      🔴 DOWN (-)".to_string(),
        ]
    );
    assert!(render_nodes(&json!({})).is_empty());
    assert!(render_nodes(&json!({"nodes": "none"})).is_empty());
}
