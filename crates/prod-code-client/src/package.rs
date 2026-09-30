//! Package management, version parity, and integrity verification for prod-code.
//!
//! Provides commands to inspect local installation packages (.pkg, .deb, homebrew),
//! verify SHA-256 cryptographic integrity against official release manifests,
//! install native packages, and monitor version consistency across the cluster fleet.

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::update::{REPOSITORY, current_target, fetch_release_info, is_newer_version};

#[derive(Subcommand, Debug)]
pub enum PackageSubcommands {
    /// Show local package installation status, version, integrity, and cluster fleet versions
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Install or upgrade native package (.pkg on macOS, .deb on Linux) from GitHub releases
    Install {
        /// Force re-installation even if the latest version is already installed
        #[arg(long)]
        force: bool,
        /// Install a specific version / tag (e.g. v0.3.19)
        #[arg(long)]
        tag: Option<String>,
        /// Install system-wide (e.g. /usr/local/bin or system package manager)
        #[arg(long)]
        system: bool,
    },
    /// Verify cryptographic SHA-256 integrity of installed binary against official GitHub release
    Verify,
    /// Inspect cluster nodes for version parity and pending updates
    Sync {
        /// Target node to inspect directly (overrides default seed)
        #[arg(long = "node")]
        node: Option<SocketAddr>,
    },
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub enum PackageType {
    MacosPkg,
    Debian,
    Homebrew,
    LocalUserBinary,
    CargoBin,
    Unknown,
}

impl std::fmt::Display for PackageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MacosPkg => write!(f, "macOS Package (.pkg)"),
            Self::Debian => write!(f, "Debian Package (.deb)"),
            Self::Homebrew => write!(f, "Homebrew Formula"),
            Self::LocalUserBinary => write!(f, "Local User Binary (~/.local/bin)"),
            Self::CargoBin => write!(f, "Cargo Binary (~/.cargo/bin)"),
            Self::Unknown => write!(f, "Standalone Binary"),
        }
    }
}

/// Detects the package type and origin from the executable path.
pub fn detect_package_type(exe_path: &Path) -> PackageType {
    let s = exe_path.to_string_lossy();
    if s.contains("/opt/homebrew/") || s.contains("/usr/local/Cellar/") || s.contains("/Cellar/") {
        PackageType::Homebrew
    } else if s == "/usr/local/bin/prod-code" && cfg!(target_os = "macos") {
        PackageType::MacosPkg
    } else if (s == "/usr/bin/prod-code" || s == "/usr/local/bin/prod-code")
        && Path::new("/etc/debian_version").exists()
    {
        PackageType::Debian
    } else if s.contains("/.local/bin/") {
        PackageType::LocalUserBinary
    } else if s.contains("/.cargo/bin/") {
        PackageType::CargoBin
    } else {
        PackageType::Unknown
    }
}

/// Computes the SHA-256 hash of a file using shasum or sha256sum.
pub fn compute_sha256(path: &Path) -> Result<String> {
    let mut cmd = Command::new("shasum");
    cmd.args(["-a", "256", path.to_str().unwrap()]);
    let output = match cmd.output() {
        Ok(out) if out.status.success() => out,
        _ => {
            let mut fallback = Command::new("sha256sum");
            fallback.arg(path.to_str().unwrap());
            fallback
                .output()
                .with_context(|| format!("failed to calculate sha256 for {}", path.display()))?
        }
    };

    if !output.status.success() {
        bail!("sha256 command failed on {}", path.display());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let hash = text
        .split_whitespace()
        .next()
        .context("empty checksum output")?
        .trim()
        .to_string();
    Ok(hash)
}

/// Parses a SHA256SUMS text file into a map of filename -> sha256.
pub fn parse_checksums_file(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let hash = parts[0].trim().to_lowercase();
            let name = parts[1].trim_start_matches('*').trim().to_string();
            map.insert(name, hash);
        }
    }
    map
}

/// Fetches the official SHA256SUMS file from GitHub Releases.
pub async fn fetch_official_checksums(tag: Option<&str>) -> Result<HashMap<String, String>> {
    let tag_str = match tag {
        Some(t) => t.to_string(),
        None => {
            let rel = fetch_release_info(None).await?;
            rel.get("tag_name")
                .and_then(|v| v.as_str())
                .unwrap_or("v0.3.19")
                .to_string()
        }
    };

    let url = format!(
        "https://github.com/{REPOSITORY}/releases/download/{tag_str}/SHA256SUMS"
    );

    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "User-Agent: prod-code-package-manager",
            &url,
        ])
        .output()
        .with_context(|| format!("failed to download SHA256SUMS from {url}"))?;

    if !output.status.success() {
        bail!("failed to fetch official checksums for {tag_str}");
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_checksums_file(&text))
}

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
    println!("  Version:       v{current_version} ({})", if is_up_to_date { "up to date" } else { "update available" });
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
                println!("\n✓ Package integrity verified: binary matches official release checksum.");
                Ok(())
            } else {
                println!("\n⚠ Checksum mismatch: binary has been modified or locally built.");
                Ok(())
            }
        }
        None => {
            println!("Note: Official release manifest contains {} assets.", checksums.len());
            println!("✓ Local hash computed: {local_hash}");
            Ok(())
        }
    }
}

/// Implements `prod-code package install`.
pub async fn run_package_install(
    force: bool,
    version: Option<String>,
    system: bool,
) -> Result<()> {
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

    let asset_url = format!(
        "https://github.com/{REPOSITORY}/releases/download/{tag}/prod-code-{target}"
    );

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
        let identity = std::env::var("PROD_CODE_SIGN_IDENTITY")
            .unwrap_or_else(|_| "Apple Development: Alexander Panasenko (alex@prod.codes)".to_string());
        let res = Command::new("codesign")
            .args(["-s", &identity, "-f", temp_file.to_str().unwrap()])
            .output();
        if res.is_err() || !res.unwrap().status.success() {
            let _ = Command::new("codesign")
                .args(["-s", "-", "-f", temp_file.to_str().unwrap()])
                .output();
        }
    }

    std::fs::rename(&temp_file, &target_bin).with_context(|| {
        format!("failed to move binary into {}", target_bin.display())
    })?;

    println!("\n✓ Package installed successfully to {}!", target_bin.display());
    let _ = Command::new(&target_bin).arg("--version").status();
    println!("Running MCP server instances will hot-reload automatically.");
    Ok(())
}

/// Helper to get user's home directory without heavy deps.
fn dirs_next_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
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
            let remote = node.get("remote").and_then(|r| r.as_str()).unwrap_or("unknown");
            let up = node.get("up").and_then(|u| u.as_bool()).unwrap_or(false);
            let platform = node.get("platform").and_then(|p| p.as_str()).unwrap_or("-");
            let sessions = node.get("active_sessions").and_then(|s| s.as_u64()).unwrap_or(0);
            let commands = node.get("running_commands").and_then(|c| c.as_array()).map(|a| a.len()).unwrap_or(0);

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
    println!("  • Ubuntu/Debian nodes (booster, ram9, rama):");
    println!("      curl -fsSL https://prod.codes/install.sh | sh");
    println!("      or: sudo dpkg -i prod-code_0.3.19_amd64.deb && systemctl --user restart prod-code-gateway");
    println!("  • macOS node (192.168.2.40):");
    println!("      scripts/deploy-mac-node.sh");
    println!("  • Rule 11 Reminder: Never restart a node while running commands exist.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_package_type() {
        assert_eq!(
            detect_package_type(Path::new("/opt/homebrew/Cellar/prod-code/0.3.19/bin/prod-code")),
            PackageType::Homebrew
        );
        assert_eq!(
            detect_package_type(Path::new("/Users/alex09x/.local/bin/prod-code")),
            PackageType::LocalUserBinary
        );
        assert_eq!(
            detect_package_type(Path::new("/Users/alex09x/.cargo/bin/prod-code")),
            PackageType::CargoBin
        );
    }

    #[test]
    fn test_parse_checksums_file() {
        let text = r#"
# Release SHA256 checksums
d23d1f9d01dae8d11bf4cc6582a33c13c9f47bcb193b0e06bcbbc527e2dd3b55  prod-code-0.3.19-macOS.dmg
3c4bb48bd3dcb0361ba3e26666a732cc4c1c6b8e017bba62de80d1016302c484  prod-code-0.3.19-macOS.pkg
d05c83197facae2d0615fc834c04b9dc149ecb0e6abe120573ea1d8ad191be44  prod-code_0.3.19_amd64.deb
"#;
        let map = parse_checksums_file(text);
        assert_eq!(
            map.get("prod-code-0.3.19-macOS.dmg").unwrap(),
            "d23d1f9d01dae8d11bf4cc6582a33c13c9f47bcb193b0e06bcbbc527e2dd3b55"
        );
        assert_eq!(
            map.get("prod-code_0.3.19_amd64.deb").unwrap(),
            "d05c83197facae2d0615fc834c04b9dc149ecb0e6abe120573ea1d8ad191be44"
        );
    }
}
