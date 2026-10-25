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

const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

fn abc_binary(dir: &Path) -> PathBuf {
    let exe = dir.join("prod-code");
    std::fs::write(&exe, "abc").unwrap();
    exe
}

#[test]
fn status_output_json_reports_local_facts_and_latest_release() {
    let exe = std::env::current_exe().unwrap();
    let release = json!({"tag_name": "v99.0.0"});
    let out = status_output(&exe, Some(&release), true).unwrap();
    assert!(out.ends_with("}\n"));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["executable_path"], exe.display().to_string());
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["latest_release"], "v99.0.0");
    assert_eq!(v["is_up_to_date"], false);
    assert_eq!(v["target"], current_target().unwrap());
    assert_eq!(v["sha256"], compute_sha256(&exe).unwrap());
}

#[test]
fn status_output_without_release_or_hashable_exe_degrades() {
    let ghost = Path::new("/nonexistent-dir/prod-code");
    let out = status_output(ghost, None, true).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["latest_release"], "unknown");
    assert_eq!(v["sha256"], "unknown");
    assert_eq!(v["package_type"], "Standalone Binary");
    assert_eq!(v["is_up_to_date"], true);

    let text = status_output(ghost, Some(&json!({"tag_name": "v0.0.1"})), false).unwrap();
    assert!(text.starts_with("⚡ prod-code Package Status\n"));
    assert!(text.contains("  Path:          /nonexistent-dir/prod-code\n"));
    assert!(text.contains(&format!(
        "  Version:       v{} (up to date)\n",
        env!("CARGO_PKG_VERSION")
    )));
    assert!(text.contains("  SHA-256:       unknown\n  Latest Tag:    v0.0.1\n"));
}

#[tokio::test]
async fn verify_flow_reports_every_step_and_passes_the_tag_to_the_fetcher() {
    let dir = tempfile::tempdir().unwrap();
    let exe = abc_binary(dir.path());
    let mut lines = Vec::new();
    let mut seen_tag = String::new();
    verify_flow(&mut |l| lines.push(l), &exe, "t-x", "1.2.3", |tag| {
        seen_tag = tag;
        async {
            Ok(HashMap::from([(
                "prod-code-t-x".to_string(),
                ABC_SHA256.to_string(),
            )]))
        }
    })
    .await
    .unwrap();
    assert_eq!(seen_tag, "v1.2.3");
    assert_eq!(
        lines,
        vec![
            "Verifying prod-code v1.2.3 integrity...".to_string(),
            "Computing local binary SHA-256 hash...".to_string(),
            format!("  Local:  {ABC_SHA256} ({})", exe.display()),
            "Fetching official release manifest from GitHub (v1.2.3)...".to_string(),
            format!(
                "  Remote: {ABC_SHA256} (prod-code-t-x)\n\n✓ Package integrity verified: binary matches official release checksum."
            ),
        ]
    );
}

#[tokio::test]
async fn verify_flow_propagates_hash_and_fetch_errors() {
    let dir = tempfile::tempdir().unwrap();
    let exe = abc_binary(dir.path());
    let mut lines = Vec::new();
    let err = verify_flow(&mut |l| lines.push(l), &exe, "t", "1.0.0", |_| async {
        Err(anyhow::anyhow!("offline"))
    })
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "offline");
    assert_eq!(lines.len(), 4);

    let ghost = dir.path().join("ghost");
    let mut lines = Vec::new();
    let err = verify_flow(&mut |l| lines.push(l), &ghost, "t", "1.0.0", |_| async {
        Ok(HashMap::new())
    })
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("sha256 command failed on {}", ghost.display())
    );
    assert_eq!(lines.len(), 2);
}

#[cfg(unix)]
#[test]
fn install_flow_decides_between_skip_install_and_refusal() {
    let target = current_target().unwrap();
    let assets = tempfile::tempdir().unwrap();
    for tag in ["v1.0.0", "v2.0.0"] {
        let tag_dir = assets.path().join(tag);
        std::fs::create_dir_all(&tag_dir).unwrap();
        std::fs::write(
            tag_dir.join(format!("prod-code-{target}")),
            format!("#!/bin/sh\necho fake-{tag}\n"),
        )
        .unwrap();
    }
    let base = format!("file://{}", assets.path().display());
    let run = |current: &str, tag: serde_json::Value, flags: (bool, bool), dir: &Path| {
        let mut lines = Vec::new();
        let res = install_flow(
            &mut |l| lines.push(l),
            current,
            target,
            &tag,
            flags,
            (dir, &base),
        );
        (res, lines)
    };
    let latest = "prod-code is already at latest version (v1.0.0). Use --force to reinstall.";

    let dest = tempfile::tempdir().unwrap();
    let bin = dest.path().join("prod-code");
    // Already current: nothing is downloaded.
    let (res, lines) = run(
        "1.0.0",
        json!({"tag_name": "v1.0.0"}),
        (false, false),
        dest.path(),
    );
    res.unwrap();
    assert_eq!(lines, vec![latest.to_string()]);
    assert!(!bin.exists());
    // An older release than the running one is also "latest" for a plain install.
    let (res, lines) = run(
        "3.0.0",
        json!({"tag_name": "v1.0.0"}),
        (false, false),
        dest.path(),
    );
    res.unwrap();
    assert_eq!(lines, vec![latest.to_string()]);

    // --force reinstalls the same version and reports each step.
    let (res, lines) = run(
        "1.0.0",
        json!({"tag_name": "v1.0.0"}),
        (true, false),
        dest.path(),
    );
    res.unwrap();
    assert_eq!(
        lines,
        vec![
            format!("Installing prod-code v1.0.0 (target: {target})..."),
            format!("Downloading release asset from {base}/v1.0.0/prod-code-{target}..."),
            format!("\n✓ Package installed successfully to {}!", bin.display()),
            "Running MCP server instances will hot-reload automatically.".to_string(),
        ]
    );
    assert_eq!(
        std::fs::read_to_string(&bin).unwrap(),
        "#!/bin/sh\necho fake-v1.0.0\n"
    );

    // An explicit version installs even when it is not newer; a newer release installs by default.
    let (res, _) = run(
        "3.0.0",
        json!({"tag_name": "v1.0.0"}),
        (false, true),
        dest.path(),
    );
    res.unwrap();
    let (res, _) = run(
        "1.0.0",
        json!({"tag_name": "v2.0.0"}),
        (false, false),
        dest.path(),
    );
    res.unwrap();
    assert_eq!(
        std::fs::read_to_string(&bin).unwrap(),
        "#!/bin/sh\necho fake-v2.0.0\n"
    );

    // Refusals: no tag in the release payload, and a tag without a published asset.
    let (res, lines) = run("1.0.0", json!({"name": "x"}), (true, false), dest.path());
    assert_eq!(res.unwrap_err().to_string(), "missing tag_name in release");
    assert!(lines.is_empty());
    let (res, lines) = run(
        "1.0.0",
        json!({"tag_name": "v9.9.9"}),
        (false, false),
        dest.path(),
    );
    assert!(
        res.unwrap_err()
            .to_string()
            .starts_with("download failed with status")
    );
    assert_eq!(lines.len(), 2);
}

#[test]
fn release_download_base_points_at_the_repository_releases() {
    assert_eq!(
        release_download_base(),
        "https://github.com/alex09x/prod-code/releases/download"
    );
}

#[test]
fn probe_cluster_passes_remote_and_cluster_arguments() {
    let remote: SocketAddr = "127.0.0.1:9".parse().unwrap();
    assert_eq!(
        probe_cluster("echo", None),
        Some(b"cluster --json\n".to_vec())
    );
    assert_eq!(
        probe_cluster("echo", Some(remote)),
        Some(b"--remote 127.0.0.1:9 cluster --json\n".to_vec())
    );
    assert_eq!(probe_cluster("false", None), None);
    assert_eq!(
        probe_cluster("prod-code-definitely-missing-program", None),
        None
    );
}

fn sync_lines(probe: Option<&[u8]>) -> (Result<()>, Vec<String>) {
    let mut lines = Vec::new();
    let res = sync_flow(&mut |l| lines.push(l), "1.2.3", || {
        probe.map(<[u8]>::to_vec)
    });
    (res, lines)
}

#[test]
fn sync_flow_prints_header_then_nodes_then_guidance() {
    let cluster = br#"{"nodes": [
        {"remote": "a:1", "up": true, "platform": "linux", "active_sessions": 2},
        {"remote": "b:2", "up": false}]}"#;
    let (res, lines) = sync_lines(Some(cluster));
    res.unwrap();
    assert_eq!(lines.len(), 4 + 2 + 1);
    assert_eq!(lines[0], "⚡ prod-code Cluster Fleet Sync Inspection");
    assert_eq!(lines[2], "Local client version: v1.2.3");
    assert_eq!(
        lines[3],
        "Scanning cluster nodes for version parity and pending commands...\n"
    );
    assert_eq!(
        lines[4],
        render_node(
            &json!({"remote": "a:1", "up": true, "platform": "linux", "active_sessions": 2})
        )
    );
    assert_eq!(lines[5], "  Node: b:2                    🔴 DOWN (-)");
    assert_eq!(lines[6], deploy_guidance("1.2.3"));

    // Valid JSON without a node list still ends with the guidance.
    let (res, lines) = sync_lines(Some(b"{}"));
    res.unwrap();
    assert_eq!(lines.len(), 5);
}

#[test]
fn sync_flow_degrades_when_probe_fails_and_errors_on_garbage() {
    let (res, lines) = sync_lines(None);
    res.unwrap();
    assert_eq!(lines.len(), 5);
    assert_eq!(
        lines[4],
        "Unable to probe cluster via prod-code cluster --json."
    );

    let (res, lines) = sync_lines(Some(b"cluster --json\n"));
    assert!(
        res.unwrap_err()
            .downcast_ref::<serde_json::Error>()
            .is_some()
    );
    assert_eq!(lines.len(), 4);
}

#[test]
fn deploy_guidance_names_the_version_in_every_package_command() {
    let g = deploy_guidance("1.2.3");
    assert!(g.starts_with("\nFleet Package Deploy Guidance:\n  • Ubuntu/Debian nodes:\n"));
    assert!(g.contains("\n      or: sudo dpkg -i prod-code_1.2.3_amd64.deb && systemctl --user restart prod-code-gateway\n"));
    assert!(g.contains("\n      sudo rpm -Uvh prod-code-1.2.3-1.x86_64.rpm && systemctl --user restart prod-code-gateway\n"));
    assert!(g.contains("\n      sudo pacman -U prod-code-1.2.3-1-x86_64.pkg.tar.gz && systemctl --user restart prod-code-gateway\n"));
    assert!(g.ends_with("\n  • macOS nodes:\n      scripts/deploy-mac-node.sh\n  • Rule 11 Reminder: Never restart a node while running commands exist."));
}
