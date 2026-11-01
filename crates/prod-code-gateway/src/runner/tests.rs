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
use std::path::Path;

#[test]
fn test_external_volume_path_detection() {
    assert!(is_external_volume_path(Path::new("/Volumes/external/repo")));
    assert!(is_external_volume_path(Path::new("/Volumes")));
    assert!(is_external_volume_path(Path::new(
        "/Volumes/worktrees/workspace-165"
    )));
    assert!(is_external_volume_path(Path::new("Volumes/external/repo")));
    assert!(is_external_volume_path(Path::new("/volumes/external/repo")));
    assert!(is_external_volume_path(Path::new("volumes/external/repo")));

    assert!(!is_external_volume_path(Path::new("/Users/alex/storage")));
    assert!(!is_external_volume_path(Path::new("/tmp/test")));
    assert!(!is_external_volume_path(Path::new("/var/folders/xyz")));
    assert!(!is_external_volume_path(Path::new("/home/user/Volumes")));
}

#[test]
fn test_eperm_error_detection() {
    let eperm_raw = std::io::Error::from_raw_os_error(1);
    assert!(eperm_raw.is_eperm());

    let perm_denied =
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Permission denied");
    assert!(perm_denied.is_eperm());

    let op_not_perm = std::io::Error::new(std::io::ErrorKind::Other, "Operation not permitted");
    assert!(op_not_perm.is_eperm());

    assert!(1i32.is_eperm());
    assert!(libc::EPERM.is_eperm());
    assert!(13i32.is_eperm());
    assert!(Some(1i32).is_eperm());

    let not_found = std::io::Error::from_raw_os_error(2);
    assert!(!not_found.is_eperm());
    assert!(!2i32.is_eperm());
    assert!(!None::<i32>.is_eperm());
}

#[test]
fn test_diagnostic_formatting_on_external_volume_with_eperm() {
    let path = Path::new("/Volumes/prod-worktrees/workspace-165/task-4476");
    let err = std::io::Error::from_raw_os_error(1);

    let diag = format_volume_access_diagnostic(path, &err).expect("diagnostic expected");

    assert!(diag.contains("macOS TCC (Privacy and Security -> Full Disk Access or Removable Volumes for prod-code-server)"));
    assert!(diag.contains("blocked access to the external volume"));
    assert!(diag.contains("/Volumes/prod-worktrees/workspace-165/task-4476"));
    assert!(diag.contains("recommends keeping gateway storage on the internal APFS volume or granting Full Disk Access to the gateway binary"));
    assert!(diag.contains("System Settings > Privacy & Security > Full Disk Access"));
    assert!(diag.contains("EPERM"));
    assert!(diag.contains("os error 1"));
}

#[test]
fn test_diagnostic_formatting_with_raw_os_error_code() {
    let path = Path::new("/Volumes/BackupDisk/projects");
    let diag = format_volume_access_diagnostic(path, 1).expect("diagnostic expected");

    assert!(diag.contains("macOS TCC (Privacy and Security -> Full Disk Access or Removable Volumes for prod-code-server)"));
    assert!(diag.contains("blocked access to the external volume"));
    assert!(diag.contains("/Volumes/BackupDisk/projects"));
    assert!(diag.contains("recommends keeping gateway storage on the internal APFS volume or granting Full Disk Access to the gateway binary"));
}

#[test]
fn test_format_volume_access_error_alias() {
    let path = Path::new("/Volumes/BackupDisk/projects");
    let diag = format_volume_access_error(path, 1).expect("diagnostic expected");
    assert!(diag.contains("macOS TCC"));
}

#[test]
fn test_diagnostic_not_formatted_for_internal_volume() {
    let path = Path::new("/Users/alex/projects/my-repo");
    let err = std::io::Error::from_raw_os_error(1);

    assert_eq!(format_volume_access_diagnostic(path, &err), None);
    assert_eq!(format_volume_access_diagnostic(path, 1), None);
}

#[test]
fn test_diagnostic_not_formatted_for_non_eperm_error() {
    let path = Path::new("/Volumes/BackupDisk/projects");
    let err = std::io::Error::from_raw_os_error(2); // ENOENT

    assert_eq!(format_volume_access_diagnostic(path, &err), None);
    assert_eq!(format_volume_access_diagnostic(path, 2), None);
}

#[test]
fn test_format_spawn_error_on_external_volume() {
    let run_dir = Path::new("/Volumes/external/workspace");
    let err = std::io::Error::from_raw_os_error(1);

    let msg = format_spawn_error(run_dir, "cargo", &err);
    assert!(msg.starts_with("failed to start cargo:"));
    assert!(msg.contains("macOS TCC"));
    assert!(msg.contains("Full Disk Access"));
}

#[test]
fn test_format_spawn_error_on_internal_volume() {
    let run_dir = Path::new("/Users/alex/workspace");
    let err = std::io::Error::from_raw_os_error(1);

    let msg = format_spawn_error(run_dir, "cargo", &err);
    assert!(msg.starts_with("failed to start cargo:"));
    assert!(!msg.contains("macOS TCC"));
}
