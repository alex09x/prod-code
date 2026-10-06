/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use clap::Subcommand;
use std::net::SocketAddr;
use std::path::Path;

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
    Rpm,
    ArchLinux,
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
            Self::Rpm => write!(f, "RPM Package (.rpm)"),
            Self::ArchLinux => write!(f, "Arch Linux Package (pacman/PKGBUILD)"),
            Self::Homebrew => write!(f, "Homebrew Formula"),
            Self::LocalUserBinary => write!(f, "Local User Binary (~/.local/bin)"),
            Self::CargoBin => write!(f, "Cargo Binary (~/.cargo/bin)"),
            Self::Unknown => write!(f, "Standalone Binary"),
        }
    }
}

/// Detects the package type and origin from the executable path.
pub fn detect_package_type(exe_path: &Path) -> PackageType {
    detect_package_type_with_fs(exe_path, |p| Path::new(p).exists())
}

/// Detects package type given a filesystem existence checker (for testing).
pub fn detect_package_type_with_fs(
    exe_path: &Path,
    path_exists: impl Fn(&str) -> bool,
) -> PackageType {
    let s = exe_path.to_string_lossy();
    if s.contains("/opt/homebrew/") || s.contains("/usr/local/Cellar/") || s.contains("/Cellar/") {
        PackageType::Homebrew
    } else if s == "/usr/local/bin/prod-code" && cfg!(target_os = "macos") {
        PackageType::MacosPkg
    } else if (s == "/usr/bin/prod-code" || s == "/usr/local/bin/prod-code")
        && (path_exists("/etc/debian_version") || path_exists("/etc/debian-release"))
    {
        PackageType::Debian
    } else if (s == "/usr/bin/prod-code" || s == "/usr/local/bin/prod-code")
        && (path_exists("/etc/redhat-release")
            || path_exists("/etc/fedora-release")
            || path_exists("/etc/almalinux-release")
            || path_exists("/etc/rocky-release")
            || path_exists("/etc/centos-release"))
    {
        PackageType::Rpm
    } else if (s == "/usr/bin/prod-code" || s == "/usr/local/bin/prod-code")
        && path_exists("/etc/arch-release")
    {
        PackageType::ArchLinux
    } else if s.contains("/.local/bin/") {
        PackageType::LocalUserBinary
    } else if s.contains("/.cargo/bin/") {
        PackageType::CargoBin
    } else {
        PackageType::Unknown
    }
}
