/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Self-update support for prod-code from GitHub releases.
//!
//! Checks the latest release on GitHub, downloads the matching binary for the
//! current architecture, verifies it, and replaces the running executable atomically.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

/// The GitHub repository where releases are published.
pub const REPOSITORY: &str = "alex09x/prod-code";

/// The current architecture and OS as represented in release asset names.
pub fn current_target() -> Result<&'static str> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Ok("aarch64-apple-darwin");

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Ok("x86_64-apple-darwin");

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Ok("x86_64-unknown-linux-gnu");

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Ok("aarch64-unknown-linux-gnu");

    #[cfg(not(any(
        all(target_os = "macos", any(target_arch = "aarch64", target_arch = "x86_64")),
        all(target_os = "linux", any(target_arch = "aarch64", target_arch = "x86_64"))
    )))]
    bail!("unsupported platform for binary self-update");
}

/// Checks if the executable appears to be managed by a package manager (Homebrew, apt, rpm).
pub fn is_package_managed(exe: &Path) -> Option<String> {
    let path_str = exe.to_string_lossy();
    if path_str.contains("/Cellar/")
        || path_str.contains("/opt/homebrew/")
        || path_str.starts_with("/usr/local/Cellar/")
    {
        return Some("Homebrew. Please run `brew upgrade prod-code` instead.".to_string());
    }
    if path_str == "/usr/bin/prod-code" && Path::new("/etc/debian_version").exists() {
        return Some(
            "Debian/Ubuntu package manager. Please run `apt update && apt upgrade prod-code` instead."
                .to_string(),
        );
    }
    if path_str == "/usr/bin/prod-code"
        && (Path::new("/etc/redhat-release").exists() || Path::new("/etc/fedora-release").exists())
    {
        return Some("RPM package manager. Please run `dnf upgrade prod-code` instead.".to_string());
    }
    None
}

/// Compares semver strings (with or without 'v' prefix).
/// Returns true if `remote` is strictly newer than `current`.
pub fn is_newer_version(current: &str, remote: &str) -> bool {
    let cur_clean = current.trim().trim_start_matches('v');
    let rem_clean = remote.trim().trim_start_matches('v');

    let cur_parts: Vec<u64> = cur_clean
        .split('.')
        .filter_map(|p| p.split('-').next().unwrap_or(p).parse().ok())
        .collect();
    let rem_parts: Vec<u64> = rem_clean
        .split('.')
        .filter_map(|p| p.split('-').next().unwrap_or(p).parse().ok())
        .collect();

    for (c, r) in cur_parts.iter().zip(rem_parts.iter()) {
        if r > c {
            return true;
        }
        if r < c {
            return false;
        }
    }
    rem_parts.len() > cur_parts.len()
}

/// Parses the release JSON payload from GitHub and finds the asset URL for the target.
pub fn parse_release_asset(json_val: &serde_json::Value, target: &str) -> Result<(String, String)> {
    let tag = json_val
        .get("tag_name")
        .and_then(|v| v.as_str())
        .context("missing tag_name in release")?
        .to_string();

    let asset_name = format!("prod-code-{target}");
    let assets = json_val
        .get("assets")
        .and_then(|v| v.as_array())
        .context("missing assets list in release")?;

    for asset in assets {
        if asset.get("name").and_then(|n| n.as_str()) == Some(&asset_name) {
            let download_url = asset
                .get("browser_download_url")
                .or_else(|| asset.get("url"))
                .and_then(|u| u.as_str())
                .context("missing download url for asset")?
                .to_string();
            return Ok((tag, download_url));
        }
    }

    bail!("asset `{asset_name}` not found in release `{tag}`");
}

/// Fetches release info via `gh` CLI or `curl`.
pub async fn fetch_release_info(specific_tag: Option<&str>) -> Result<serde_json::Value> {
    // 1. Try `gh release view` if gh is present
    let mut cmd = Command::new("gh");
    cmd.args(["release", "view"]);
    if let Some(tag) = specific_tag {
        cmd.arg(tag);
    }
    cmd.args(["--repo", REPOSITORY, "--json", "tagName,assets"]);

    if let Ok(output) = cmd.output()
        && output.status.success()
        && let Ok(mut val) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
    {
        // Normalize tagName -> tag_name
        if let Some(tag_name) = val.get("tagName").cloned() {
            val["tag_name"] = tag_name;
        }
        return Ok(val);
    }

    // 2. Fallback: curl against GitHub REST API
    let url = match specific_tag {
        Some(tag) => format!("https://api.github.com/repos/{REPOSITORY}/releases/tags/{tag}"),
        None => format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"),
    };

    let curl_output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "User-Agent: prod-code-updater",
            "-H",
            "Accept: application/vnd.github.v3+json",
            &url,
        ])
        .output()
        .with_context(|| "failed to run curl to check releases")?;

    if !curl_output.status.success() {
        let err_msg = String::from_utf8_lossy(&curl_output.stderr);
        bail!("failed to fetch release from {url}: {err_msg}");
    }

    let val = serde_json::from_slice(&curl_output.stdout)
        .with_context(|| "failed to parse GitHub release JSON")?;
    Ok(val)
}

/// Runs the update command.
pub async fn run_update(
    check_only: bool,
    force: bool,
    specific_tag: Option<String>,
) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    let target = current_target()?;
    let current_exe = std::env::current_exe().context("failed to locate current executable")?;

    if let Some(pm_hint) = is_package_managed(&current_exe) {
        println!("Note: prod-code binary is managed by {pm_hint}");
        if !force {
            return Ok(());
        }
    }

    println!("Checking for updates (current version: v{current_version}, target: {target})...");
    let release_val = fetch_release_info(specific_tag.as_deref()).await?;
    let (tag, download_url) = parse_release_asset(&release_val, target)?;

    let is_newer = is_newer_version(current_version, &tag);

    if !is_newer && !force && specific_tag.is_none() {
        println!("prod-code is already up to date ({tag}).");
        return Ok(());
    }

    if check_only {
        println!("An update is available: v{current_version} -> {tag}");
        println!("Download URL: {download_url}");
        println!("Run `prod-code update` to install it.");
        return Ok(());
    }

    println!("Updating prod-code: v{current_version} -> {tag}...");
    println!("Downloading from {download_url}...");

    let exe_dir = current_exe
        .parent()
        .context("failed to get directory of current executable")?;
    let temp_download = exe_dir.join(format!(
        ".prod-code-update-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    // Download via curl
    let dl_status = Command::new("curl")
        .args([
            "-fL",
            "--progress-bar",
            "-o",
            temp_download.to_str().unwrap(),
            &download_url,
        ])
        .status()
        .with_context(|| "failed to download update binary with curl")?;

    if !dl_status.success() {
        let _ = std::fs::remove_file(&temp_download);
        bail!("download failed with status {dl_status}");
    }

    // Set executable permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&temp_download, perms)?;
    }

    // On macOS, codesign the binary with official identity or fallback to ad-hoc
    #[cfg(target_os = "macos")]
    {
        let identity = std::env::var("PROD_CODE_SIGN_IDENTITY")
            .unwrap_or_else(|_| "Apple Development: Alexander Panasenko (alex@prod.codes)".to_string());
        let res = Command::new("codesign")
            .args(["-s", &identity, "-f", temp_download.to_str().unwrap()])
            .output();
        if res.is_err() || !res.unwrap().status.success() {
            let _ = Command::new("codesign")
                .args(["-s", "-", "-f", temp_download.to_str().unwrap()])
                .output();
        }
    }

    // Atomic replace via rename
    std::fs::rename(&temp_download, &current_exe).with_context(|| {
        format!(
            "failed to replace {} with downloaded update",
            current_exe.display()
        )
    })?;

    println!("Successfully updated prod-code to {tag} at {}!", current_exe.display());
    println!("Running MCP server sessions will hot-reload automatically.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_target_is_valid() {
        let target = current_target().unwrap();
        assert!(
            target == "aarch64-apple-darwin"
                || target == "x86_64-apple-darwin"
                || target == "x86_64-unknown-linux-gnu"
                || target == "aarch64-unknown-linux-gnu"
        );
    }

    #[test]
    fn test_is_newer_version() {
        assert!(is_newer_version("0.3.18", "0.3.19"));
        assert!(is_newer_version("v0.3.18", "v0.3.19"));
        assert!(is_newer_version("0.3.18", "v0.4.0"));
        assert!(is_newer_version("0.3.18", "v1.0.0"));
        assert!(!is_newer_version("0.3.19", "0.3.19"));
        assert!(!is_newer_version("v0.3.19", "v0.3.19"));
        assert!(!is_newer_version("0.3.19", "0.3.18"));
        assert!(!is_newer_version("1.0.0", "0.9.9"));
    }

    #[test]
    fn test_is_package_managed() {
        assert!(is_package_managed(Path::new("/opt/homebrew/bin/prod-code")).is_some());
        assert!(is_package_managed(Path::new("/usr/local/Cellar/prod-code/0.3.19/bin/prod-code")).is_some());
        assert!(is_package_managed(Path::new("/Users/alex/.cargo/bin/prod-code")).is_none());
        assert!(is_package_managed(Path::new("/home/user/.local/bin/prod-code")).is_none());
    }

    #[test]
    fn test_parse_release_asset() {
        let json_val = serde_json::json!({
            "tag_name": "v0.3.19",
            "assets": [
                {
                    "name": "prod-code-aarch64-apple-darwin",
                    "browser_download_url": "https://github.com/alex09x/prod-code/releases/download/v0.3.19/prod-code-aarch64-apple-darwin"
                },
                {
                    "name": "prod-code-x86_64-unknown-linux-gnu",
                    "browser_download_url": "https://github.com/alex09x/prod-code/releases/download/v0.3.19/prod-code-x86_64-unknown-linux-gnu"
                }
            ]
        });

        let (tag, url) = parse_release_asset(&json_val, "aarch64-apple-darwin").unwrap();
        assert_eq!(tag, "v0.3.19");
        assert_eq!(
            url,
            "https://github.com/alex09x/prod-code/releases/download/v0.3.19/prod-code-aarch64-apple-darwin"
        );

        let err = parse_release_asset(&json_val, "nonexistent-target");
        assert!(err.is_err());
    }
}
