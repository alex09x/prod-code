/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::checksum::parse_checksums_file;
use super::types::{
    PackageSubcommands, PackageType, detect_package_type, detect_package_type_with_fs,
};
use clap::Parser;
use std::path::Path;

#[test]
fn test_detect_package_type() {
    assert_eq!(
        detect_package_type(Path::new(
            "/opt/homebrew/Cellar/prod-code/0.3.19/bin/prod-code"
        )),
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
fn test_detect_package_type_rpm_and_arch() {
    assert_eq!(
        detect_package_type_with_fs(Path::new("/usr/bin/prod-code"), |p| p
            == "/etc/redhat-release"),
        PackageType::Rpm
    );
    assert_eq!(
        detect_package_type_with_fs(Path::new("/usr/bin/prod-code"), |p| p
            == "/etc/fedora-release"),
        PackageType::Rpm
    );
    assert_eq!(
        detect_package_type_with_fs(Path::new("/usr/bin/prod-code"), |p| p
            == "/etc/arch-release"),
        PackageType::ArchLinux
    );
    assert_eq!(
        detect_package_type_with_fs(Path::new("/usr/bin/prod-code"), |p| p
            == "/etc/debian_version"),
        PackageType::Debian
    );
}

#[test]
fn test_parse_checksums_file() {
    let text = r#"
# Release SHA256 checksums
d23d1f9d01dae8d11bf4cc6582a33c13c9f47bcb193b0e06bcbbc527e2dd3b55  prod-code-0.3.19-macOS.dmg
3c4bb48bd3dcb0361ba3e26666a732cc4c1c6b8e017bba62de80d1016302c484  prod-code-0.3.19-macOS.pkg
d05c83197facae2d0615fc834c04b9dc149ecb0e6abe120573ea1d8ad191be44  prod-code_0.3.19_amd64.deb
8f5a1e2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f  prod-code-0.3.19-1.x86_64.rpm
1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b  prod-code-0.3.19-1-x86_64.pkg.tar.gz
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
    assert_eq!(
        map.get("prod-code-0.3.19-1.x86_64.rpm").unwrap(),
        "8f5a1e2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f"
    );
    assert_eq!(
        map.get("prod-code-0.3.19-1-x86_64.pkg.tar.gz").unwrap(),
        "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b"
    );
}

#[test]
fn test_package_scripts_exist_and_executable() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let rpm_script = root.join("scripts/packaging/package-rpm.py");
    let arch_script = root.join("scripts/packaging/package-arch.sh");
    let deb_script = root.join("scripts/packaging/package-deb.py");
    let build_all = root.join("scripts/packaging/build-all-packages.sh");

    assert!(rpm_script.exists(), "package-rpm.py must exist");
    assert!(arch_script.exists(), "package-arch.sh must exist");
    assert!(deb_script.exists(), "package-deb.py must exist");
    assert!(build_all.exists(), "build-all-packages.sh must exist");
}

#[test]
fn package_type_display_names() {
    let cases = [
        (PackageType::MacosPkg, "macOS Package (.pkg)"),
        (PackageType::Debian, "Debian Package (.deb)"),
        (PackageType::Rpm, "RPM Package (.rpm)"),
        (
            PackageType::ArchLinux,
            "Arch Linux Package (pacman/PKGBUILD)",
        ),
        (PackageType::Homebrew, "Homebrew Formula"),
        (
            PackageType::LocalUserBinary,
            "Local User Binary (~/.local/bin)",
        ),
        (PackageType::CargoBin, "Cargo Binary (~/.cargo/bin)"),
        (PackageType::Unknown, "Standalone Binary"),
    ];
    for (ty, text) in cases {
        assert_eq!(ty.to_string(), text);
    }
}

#[test]
fn detect_package_type_table() {
    let none = |_: &str| false;
    let only = |marker: &'static str| move |p: &str| p == marker;
    // Homebrew wins over everything, including system paths.
    for exe in [
        "/opt/homebrew/bin/prod-code",
        "/usr/local/Cellar/prod-code/1/bin/prod-code",
        "/home/u/Cellar/prod-code",
    ] {
        assert_eq!(
            detect_package_type_with_fs(Path::new(exe), only("/etc/debian_version")),
            PackageType::Homebrew,
            "{exe}"
        );
    }
    // Distribution markers for both system install locations.
    for exe in ["/usr/bin/prod-code", "/usr/local/bin/prod-code"] {
        if cfg!(target_os = "macos") && exe == "/usr/local/bin/prod-code" {
            assert_eq!(
                detect_package_type_with_fs(Path::new(exe), none),
                PackageType::MacosPkg
            );
            continue;
        }
        for (marker, want) in [
            ("/etc/debian_version", PackageType::Debian),
            ("/etc/debian-release", PackageType::Debian),
            ("/etc/redhat-release", PackageType::Rpm),
            ("/etc/fedora-release", PackageType::Rpm),
            ("/etc/almalinux-release", PackageType::Rpm),
            ("/etc/rocky-release", PackageType::Rpm),
            ("/etc/centos-release", PackageType::Rpm),
            ("/etc/arch-release", PackageType::ArchLinux),
        ] {
            assert_eq!(
                detect_package_type_with_fs(Path::new(exe), only(marker)),
                want,
                "{exe} with {marker}"
            );
        }
        // System path without any distribution marker is a standalone binary.
        assert_eq!(
            detect_package_type_with_fs(Path::new(exe), none),
            PackageType::Unknown
        );
    }
    // Debian is checked before RPM when both markers exist.
    assert_eq!(
        detect_package_type_with_fs(Path::new("/usr/bin/prod-code"), |_| true),
        PackageType::Debian
    );
    // Distribution markers do not matter outside the system paths.
    assert_eq!(
        detect_package_type_with_fs(Path::new("/tmp/prod-code"), |_| true),
        PackageType::Unknown
    );
    assert_eq!(
        detect_package_type_with_fs(Path::new("/home/u/.local/bin/prod-code"), none),
        PackageType::LocalUserBinary
    );
    assert_eq!(
        detect_package_type_with_fs(Path::new("/home/u/.cargo/bin/prod-code"), none),
        PackageType::CargoBin
    );
}

#[derive(Parser)]
struct PackageCli {
    #[command(subcommand)]
    cmd: PackageSubcommands,
}

#[test]
fn package_subcommands_parse_flags() {
    let parse = |args: &[&str]| {
        let mut argv = vec!["pkg"];
        argv.extend_from_slice(args);
        PackageCli::try_parse_from(argv).map(|c| c.cmd)
    };
    assert!(matches!(
        parse(&["status", "--json"]).unwrap(),
        PackageSubcommands::Status { json: true }
    ));
    assert!(matches!(
        parse(&["status"]).unwrap(),
        PackageSubcommands::Status { json: false }
    ));
    match parse(&["install", "--force", "--tag", "v0.3.19", "--system"]).unwrap() {
        PackageSubcommands::Install { force, tag, system } => {
            assert!(force && system);
            assert_eq!(tag.as_deref(), Some("v0.3.19"));
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(
        parse(&["verify"]).unwrap(),
        PackageSubcommands::Verify
    ));
    match parse(&["sync", "--node", "127.0.0.1:9400"]).unwrap() {
        PackageSubcommands::Sync { node } => {
            assert_eq!(node, Some("127.0.0.1:9400".parse().unwrap()));
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(parse(&["sync", "--node", "not-an-addr"]).is_err());
    assert!(parse(&["bogus"]).is_err());
}

#[test]
fn package_type_serializes_as_variant_name() {
    assert_eq!(
        serde_json::to_string(&PackageType::ArchLinux).unwrap(),
        "\"ArchLinux\""
    );
}
