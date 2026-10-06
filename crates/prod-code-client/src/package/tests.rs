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
use super::types::{PackageType, detect_package_type, detect_package_type_with_fs};
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
