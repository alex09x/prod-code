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
use super::types::{PackageType, detect_package_type};
use crate::update::{REPOSITORY, current_target, fetch_release_info, is_newer_version};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Facts about the running client that `package status` reports.
struct LocalStatus<'a> {
    exe: &'a Path,
    pkg_type: PackageType,
    version: &'a str,
    latest_tag: &'a str,
    is_up_to_date: bool,
    target: &'a str,
    sha256: &'a str,
}

impl LocalStatus<'_> {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "executable_path": self.exe.display().to_string(),
            "package_type": self.pkg_type.to_string(),
            "version": self.version,
            "latest_release": self.latest_tag,
            "is_up_to_date": self.is_up_to_date,
            "target": self.target,
            "sha256": self.sha256,
        })
    }

    fn to_text(&self) -> String {
        let freshness = if self.is_up_to_date {
            "up to date"
        } else {
            "update available"
        };
        format!(
            "⚡ prod-code Package Status\n\
             ────────────────────────────────────────────────────\n\
             Local Client:\n  \
             Path:          {}\n  \
             Package Type:  {}\n  \
             Version:       v{} ({freshness})\n  \
             Platform:      {}\n  \
             SHA-256:       {}\n  \
             Latest Tag:    {}\n\
             \n\
             Fleet Orchestration:\n  \
             To verify release cryptographic integrity: prod-code package verify\n  \
             To update local packages to latest:        prod-code package install\n  \
             To inspect cluster node versions:          prod-code package sync\n",
            self.exe.display(),
            self.pkg_type,
            self.version,
            self.target,
            self.sha256,
            self.latest_tag,
        )
    }
}

/// Builds the `package status` output for `exe` given the latest release, if one was fetched.
fn status_output(exe: &Path, release: Option<&serde_json::Value>, json: bool) -> Result<String> {
    let current_version = env!("CARGO_PKG_VERSION");
    let local_sha = compute_sha256(exe).unwrap_or_else(|_| "unknown".to_string());
    let latest_tag = release
        .and_then(|v| v.get("tag_name").and_then(|t| t.as_str()))
        .unwrap_or("unknown");
    let status = LocalStatus {
        exe,
        pkg_type: detect_package_type(exe),
        version: current_version,
        latest_tag,
        is_up_to_date: !is_newer_version(current_version, latest_tag),
        target: current_target()?,
        sha256: &local_sha,
    };
    if json {
        Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&status.to_json())?
        ))
    } else {
        Ok(status.to_text())
    }
}

/// Implements `prod-code package status`.
pub async fn run_package_status(json: bool) -> Result<()> {
    let current_exe = std::env::current_exe().context("failed to locate current executable")?;
    let release_val = fetch_release_info(None).await.ok();
    print!(
        "{}",
        status_output(&current_exe, release_val.as_ref(), json)?
    );
    Ok(())
}

/// Compares the local hash with the release manifest entry for `expected_asset`.
fn verification_report(
    local_hash: &str,
    checksums: &HashMap<String, String>,
    expected_asset: &str,
) -> String {
    match checksums.get(expected_asset) {
        Some(expected) => {
            let verdict = if local_hash.eq_ignore_ascii_case(expected) {
                "✓ Package integrity verified: binary matches official release checksum."
            } else {
                "⚠ Checksum mismatch: binary has been modified or locally built."
            };
            format!("  Remote: {expected} ({expected_asset})\n\n{verdict}")
        }
        None => format!(
            "Note: Official release manifest contains {} assets.\n✓ Local hash computed: {local_hash}",
            checksums.len()
        ),
    }
}

/// `package verify`: hashes `exe`, fetches the manifest for `v{version}` through `fetch`
/// and reports each step through `emit`.
async fn verify_flow<F, Fut>(
    emit: &mut dyn FnMut(String),
    exe: &Path,
    target: &str,
    version: &str,
    fetch: F,
) -> Result<()>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<HashMap<String, String>>>,
{
    let tag = format!("v{version}");
    emit(format!("Verifying prod-code v{version} integrity..."));
    emit("Computing local binary SHA-256 hash...".to_string());
    let local_hash = compute_sha256(exe)?;
    emit(format!("  Local:  {local_hash} ({})", exe.display()));

    emit(format!(
        "Fetching official release manifest from GitHub ({tag})..."
    ));
    let checksums = fetch(tag).await?;
    let expected_asset = format!("prod-code-{target}");
    emit(verification_report(
        &local_hash,
        &checksums,
        &expected_asset,
    ));
    Ok(())
}

/// Implements `prod-code package verify`.
pub async fn run_package_verify() -> Result<()> {
    let current_exe = std::env::current_exe().context("failed to locate current executable")?;
    verify_flow(
        &mut |line| println!("{line}"),
        &current_exe,
        current_target()?,
        env!("CARGO_PKG_VERSION"),
        |tag| async move { fetch_official_checksums(Some(&tag)).await },
    )
    .await
}

/// Helper to get user's home directory without heavy deps.
fn dirs_next_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Picks where `package install` puts the binary: `/usr/local/bin` for `--system`,
/// otherwise the first existing of `~/.local/bin` and `~/.cargo/bin`, else `~/.local/bin`.
fn install_dir_for(system: bool, home: Option<PathBuf>) -> PathBuf {
    if system {
        return PathBuf::from("/usr/local/bin");
    }
    match home {
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
}

/// Downloads `asset_url` with curl into `install_dir`, marks it executable, signs it on
/// macOS and moves it to `install_dir/prod-code`. Returns the installed path.
fn download_and_install(asset_url: &str, install_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(install_dir)?;
    let target_bin = install_dir.join("prod-code");
    let temp_file = install_dir.join(format!(".prod-code-pkg-tmp-{}", std::process::id()));

    let dl_status = Command::new("curl")
        .args([
            "-fL",
            "--progress-bar",
            "-o",
            temp_file.to_str().unwrap(),
            asset_url,
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
    Ok(target_bin)
}

/// Where the release assets are published; `{base}/{tag}/prod-code-{target}` is one asset.
fn release_download_base() -> String {
    format!("https://github.com/{REPOSITORY}/releases/download")
}

/// `package install` once the release is known: skips when already current, otherwise
/// downloads `{asset_base}/{tag}/prod-code-{target}` into `install_dir`.
fn install_flow(
    emit: &mut dyn FnMut(String),
    current_version: &str,
    target: &str,
    release: &serde_json::Value,
    (force, pinned): (bool, bool),
    (install_dir, asset_base): (&Path, &str),
) -> Result<()> {
    let tag = release
        .get("tag_name")
        .and_then(|t| t.as_str())
        .context("missing tag_name in release")?;

    if !is_newer_version(current_version, tag) && !force && !pinned {
        emit(format!(
            "prod-code is already at latest version ({tag}). Use --force to reinstall."
        ));
        return Ok(());
    }

    emit(format!("Installing prod-code {tag} (target: {target})..."));
    let asset_url = format!("{asset_base}/{tag}/prod-code-{target}");
    emit(format!("Downloading release asset from {asset_url}..."));
    let target_bin = download_and_install(&asset_url, install_dir)?;

    emit(format!(
        "\n✓ Package installed successfully to {}!",
        target_bin.display()
    ));
    let _ = Command::new(&target_bin).arg("--version").status();
    emit("Running MCP server instances will hot-reload automatically.".to_string());
    Ok(())
}

/// Implements `prod-code package install`.
pub async fn run_package_install(force: bool, version: Option<String>, system: bool) -> Result<()> {
    let target = current_target()?;
    println!("Checking release packages...");
    let release_val = fetch_release_info(version.as_deref()).await?;
    install_flow(
        &mut |line| println!("{line}"),
        env!("CARGO_PKG_VERSION"),
        target,
        &release_val,
        (force, version.is_some()),
        (
            &install_dir_for(system, dirs_next_home()),
            &release_download_base(),
        ),
    )
}

/// Renders one `package sync` line block for a node of the `prod-code cluster --json` output.
fn render_node(node: &serde_json::Value) -> String {
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
        return format!("  Node: {remote:<22} 🔴 DOWN ({platform})");
    }

    let status_indicator = if commands > 0 {
        "🟡 BUSY (commands in-flight)"
    } else if sessions > 0 {
        "🟢 ACTIVE (sessions connected)"
    } else {
        "⚪ IDLE (ready for upgrade)"
    };

    format!(
        "  Node: {remote:<22} {status_indicator}\n        Platform: {platform} | Sessions: {sessions} | Running commands: {commands}"
    )
}

/// Renders the per-node section of `package sync` from the cluster JSON.
fn render_nodes(cluster: &serde_json::Value) -> Vec<String> {
    cluster
        .get("nodes")
        .and_then(|n| n.as_array())
        .map(|nodes| nodes.iter().map(render_node).collect())
        .unwrap_or_default()
}

/// Runs `program [--remote ADDR] cluster --json` and returns its stdout on success.
fn probe_cluster(program: &str, remote: Option<SocketAddr>) -> Option<Vec<u8>> {
    let mut cmd = Command::new(program);
    if let Some(r) = remote {
        cmd.args(["--remote", &r.to_string()]);
    }
    cmd.args(["cluster", "--json"]);
    match cmd.output() {
        Ok(out) if out.status.success() => Some(out.stdout),
        _ => None,
    }
}

/// The per-platform upgrade hints printed at the end of `package sync`.
fn deploy_guidance(version: &str) -> String {
    [
        "".to_string(),
        "Fleet Package Deploy Guidance:".to_string(),
        "  • Ubuntu/Debian nodes:".to_string(),
        "      curl -fsSL https://prod.codes/install.sh | sh".to_string(),
        format!(
            "      or: sudo dpkg -i prod-code_{version}_amd64.deb && systemctl --user restart prod-code-gateway"
        ),
        "  • RHEL/Fedora/CentOS nodes:".to_string(),
        format!(
            "      sudo rpm -Uvh prod-code-{version}-1.x86_64.rpm && systemctl --user restart prod-code-gateway"
        ),
        "  • Arch Linux nodes:".to_string(),
        format!(
            "      sudo pacman -U prod-code-{version}-1-x86_64.pkg.tar.gz && systemctl --user restart prod-code-gateway"
        ),
        "  • macOS nodes:".to_string(),
        "      scripts/deploy-mac-node.sh".to_string(),
        "  • Rule 11 Reminder: Never restart a node while running commands exist.".to_string(),
    ]
    .join("\n")
}

/// `package sync`: prints the header, then the nodes found in the probe's JSON output.
fn sync_flow(
    emit: &mut dyn FnMut(String),
    version: &str,
    probe: impl FnOnce() -> Option<Vec<u8>>,
) -> Result<()> {
    emit("⚡ prod-code Cluster Fleet Sync Inspection".to_string());
    emit("────────────────────────────────────────────────────".to_string());
    emit(format!("Local client version: v{version}"));
    emit("Scanning cluster nodes for version parity and pending commands...\n".to_string());

    let Some(stdout) = probe() else {
        emit("Unable to probe cluster via prod-code cluster --json.".to_string());
        return Ok(());
    };

    let val: serde_json::Value = serde_json::from_slice(&stdout)?;
    for line in render_nodes(&val) {
        emit(line);
    }
    emit(deploy_guidance(version));
    Ok(())
}

/// Implements `prod-code package sync`.
pub async fn run_package_sync(remote: Option<SocketAddr>) -> Result<()> {
    sync_flow(
        &mut |line| println!("{line}"),
        env!("CARGO_PKG_VERSION"),
        || probe_cluster("prod-code", remote),
    )
}

#[cfg(test)]
mod flow_tests;
#[cfg(test)]
mod tests;
