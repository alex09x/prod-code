/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::checksum::{compute_sha256, fetch_official_checksums};
use super::types::detect_package_type;
use crate::update::{REPOSITORY, current_target, fetch_release_info, is_newer_version};
use anyhow::{Context, Result, bail};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;

/// Implements `prod-code package status`.
pub async fn run_package_status(json: bool) -> Result<()> {
    let current_exe = std::env::current_exe().context("failed to locate current executable")?;
    let pkg_type = detect_package_type(&current_exe);
    let current_version = env!("CARGO_PKG_VERSION");
    let target = current_target()?;
    let local_sha = compute_sha256(&current_exe).unwrap_or_else(|_| "unknown".to_string());

    let release_val = fetch_release_info(None).await.ok();
    let latest_tag = release_val
        .as_ref()
        .and_then(|v| v.get("tag_name").and_then(|t| t.as_str()))
        .unwrap_or("unknown");
    let is_up_to_date = !is_newer_version(current_version, latest_tag);

    if json {
        let status = serde_json::json!({
            "executable_path": current_exe.display().to_string(),
            "package_type": pkg_type.to_string(),
            "version": current_version,
            "latest_release": latest_tag,
            "is_up_to_date": is_up_to_date,
            "target": target,
            "sha256": local_sha,
        });
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    println!("⚡ prod-code Package Status");
    println!("────────────────────────────────────────────────────");
    println!("Local Client:");
    println!("  Path:          {}", current_exe.display());
    println!("  Package Type:  {pkg_type}");
    println!(
        "  Version:       v{current_version} ({})",
        if is_up_to_date {
            "up to date"
        } else {
            "update available"
        }
    );
    println!("  Platform:      {target}");
    println!("  SHA-256:       {local_sha}");
    println!("  Latest Tag:    {latest_tag}");
    println!();
    println!("Fleet Orchestration:");
    println!("  To verify release cryptographic integrity: prod-code package verify");
    println!("  To update local packages to latest:        prod-code package install");
    println!("  To inspect cluster node versions:          prod-code package sync");
    Ok(())
}

/// Implements `prod-code package verify`.
pub async fn run_package_verify() -> Result<()> {
    let current_exe = std::env::current_exe().context("failed to locate current executable")?;
    let target = current_target()?;
    let current_version = env!("CARGO_PKG_VERSION");
    let tag = format!("v{current_version}");

    println!("Verifying prod-code v{current_version} integrity...");
    println!("Computing local binary SHA-256 hash...");
    let local_hash = compute_sha256(&current_exe)?;
    println!("  Local:  {local_hash} ({})", current_exe.display());

    println!("Fetching official release manifest from GitHub ({tag})...");
    let checksums = fetch_official_checksums(Some(&tag)).await?;

    let expected_asset = format!("prod-code-{target}");
    let expected_hash = checksums.get(&expected_asset);

    match expected_hash {
        Some(expected) => {
            println!("  Remote: {expected} ({expected_asset})");
            if local_hash.eq_ignore_ascii_case(expected) {
                println!(
                    "\n✓ Package integrity verified: binary matches official release checksum."
                );
                Ok(())
            } else {
                println!("\n⚠ Checksum mismatch: binary has been modified or locally built.");
                Ok(())
            }
        }
        None => {
            println!(
                "Note: Official release manifest contains {} assets.",
                checksums.len()
            );
            println!("✓ Local hash computed: {local_hash}");
            Ok(())
        }
    }
}

/// Helper to get user's home directory without heavy deps.
fn dirs_next_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Implements `prod-code package install`.
pub async fn run_package_install(force: bool, version: Option<String>, system: bool) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    let target = current_target()?;

    println!("Checking release packages...");
    let release_val = fetch_release_info(version.as_deref()).await?;
    let tag = release_val
        .get("tag_name")
        .and_then(|t| t.as_str())
        .context("missing tag_name in release")?;

    if !is_newer_version(current_version, tag) && !force && version.is_none() {
        println!("prod-code is already at latest version ({tag}). Use --force to reinstall.");
        return Ok(());
    }

    println!("Installing prod-code {tag} (target: {target})...");

    // Determine target installation directory
    let install_dir: PathBuf = if system {
        PathBuf::from("/usr/local/bin")
    } else {
        match dirs_next_home() {
            Some(home) => {
                let local_bin = home.join(".local/bin");
                let cargo_bin = home.join(".cargo/bin");
                if local_bin.exists() {
                    local_bin
                } else if cargo_bin.exists() {
                    cargo_bin
                } else {
                    local_bin
                }
            }
            None => PathBuf::from("/usr/local/bin"),
        }
    };

    std::fs::create_dir_all(&install_dir)?;
    let target_bin = install_dir.join("prod-code");

    let asset_url =
        format!("https://github.com/{REPOSITORY}/releases/download/{tag}/prod-code-{target}");

    println!("Downloading release asset from {asset_url}...");
    let temp_file = install_dir.join(format!(".prod-code-pkg-tmp-{}", std::process::id()));

    let dl_status = Command::new("curl")
        .args([
            "-fL",
            "--progress-bar",
            "-o",
            temp_file.to_str().unwrap(),
            &asset_url,
        ])
        .status()
        .with_context(|| "failed to download asset via curl")?;

    if !dl_status.success() {
        let _ = std::fs::remove_file(&temp_file);
        bail!("download failed with status {dl_status}");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp_file, std::fs::Permissions::from_mode(0o755))?;
    }

    #[cfg(target_os = "macos")]
    {
        let identity = std::env::var("PROD_CODE_SIGN_IDENTITY").unwrap_or_else(|_| {
            "Apple Development: Alexander Panasenko (alex@prod.codes)".to_string()
        });
        let res = Command::new("codesign")
            .args(["-s", &identity, "-f", temp_file.to_str().unwrap()])
            .output();
        if res.is_err() || !res.unwrap().status.success() {
            let _ = Command::new("codesign")
                .args(["-s", "-", "-f", temp_file.to_str().unwrap()])
                .output();
        }
    }

    std::fs::rename(&temp_file, &target_bin)
        .with_context(|| format!("failed to move binary into {}", target_bin.display()))?;

    println!(
        "\n✓ Package installed successfully to {}!",
        target_bin.display()
    );
    let _ = Command::new(&target_bin).arg("--version").status();
    println!("Running MCP server instances will hot-reload automatically.");
    Ok(())
}

/// Implements `prod-code package sync`.
pub async fn run_package_sync(remote: Option<SocketAddr>) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    println!("⚡ prod-code Cluster Fleet Sync Inspection");
    println!("────────────────────────────────────────────────────");
    println!("Local client version: v{current_version}");
    println!("Scanning cluster nodes for version parity and pending commands...\n");

    let mut cmd = Command::new("prod-code");
    if let Some(r) = remote {
        cmd.args(["--remote", &r.to_string()]);
    }
    cmd.args(["cluster", "--json"]);

    let output = match cmd.output() {
        Ok(out) if out.status.success() => out,
        _ => {
            println!("Unable to probe cluster via prod-code cluster --json.");
            return Ok(());
        }
    };

    let val: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    if let Some(nodes) = val.get("nodes").and_then(|n| n.as_array()) {
        for node in nodes {
            let remote = node
                .get("remote")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown");
            let up = node.get("up").and_then(|u| u.as_bool()).unwrap_or(false);
            let platform = node.get("platform").and_then(|p| p.as_str()).unwrap_or("-");
            let sessions = node
                .get("active_sessions")
                .and_then(|s| s.as_u64())
                .unwrap_or(0);
            let commands = node
                .get("running_commands")
                .and_then(|c| c.as_array())
                .map(|a| a.len())
                .unwrap_or(0);

            if !up {
                println!("  Node: {remote:<22} 🔴 DOWN ({platform})");
                continue;
            }

            let status_indicator = if commands > 0 {
                "🟡 BUSY (commands in-flight)"
            } else if sessions > 0 {
                "🟢 ACTIVE (sessions connected)"
            } else {
                "⚪ IDLE (ready for upgrade)"
            };

            println!(
                "  Node: {remote:<22} {status_indicator}\n        Platform: {platform} | Sessions: {sessions} | Running commands: {commands}"
            );
        }
    }

    println!("\nFleet Package Deploy Guidance:");
    println!("  • Ubuntu/Debian nodes:");
    println!("      curl -fsSL https://prod.codes/install.sh | sh");
    println!(
        "      or: sudo dpkg -i prod-code_{current_version}_amd64.deb && systemctl --user restart prod-code-gateway"
    );
    println!("  • RHEL/Fedora/CentOS nodes:");
    println!(
        "      sudo rpm -Uvh prod-code-{current_version}-1.x86_64.rpm && systemctl --user restart prod-code-gateway"
    );
    println!("  • Arch Linux nodes:");
    println!(
        "      sudo pacman -U prod-code-{current_version}-1-x86_64.pkg.tar.gz && systemctl --user restart prod-code-gateway"
    );
    println!("  • macOS nodes:");
    println!("      scripts/deploy-mac-node.sh");
    println!("  • Rule 11 Reminder: Never restart a node while running commands exist.");
    Ok(())
}
