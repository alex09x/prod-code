/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod compiler_env;
pub(crate) mod exec_stream;
pub mod json_stream;
pub mod process_group;
pub mod ram_cache;
pub mod run_exec;
pub mod run_remote_exec;
pub mod snapshot;

pub use compiler_env::*;
pub use json_stream::*;
pub use process_group::*;
pub use ram_cache::*;
pub use run_exec::*;
pub use run_remote_exec::*;
pub use snapshot::*;

#[cfg(test)]
mod tests;

/// Trait to accept either `&std::io::Error`, `std::io::Error`, `i32` raw OS error code,
/// or `Option<i32>` when checking for EPERM / permission denial (#1776).
pub trait AsIoErrorOrRawCode {
    fn is_eperm(&self) -> bool;
    fn display_message(&self) -> String;
}

impl AsIoErrorOrRawCode for std::io::Error {
    fn is_eperm(&self) -> bool {
        self.raw_os_error() == Some(1)
            || self.raw_os_error() == Some(13)
            || self.kind() == std::io::ErrorKind::PermissionDenied
            || {
                let msg = self.to_string();
                let lower = msg.to_ascii_lowercase();
                lower.contains("operation not permitted")
                    || lower.contains("permission denied")
                    || lower.contains("os error 1")
                    || lower.contains("eperm")
            }
    }

    fn display_message(&self) -> String {
        self.to_string()
    }
}

impl AsIoErrorOrRawCode for &std::io::Error {
    fn is_eperm(&self) -> bool {
        (*self).is_eperm()
    }

    fn display_message(&self) -> String {
        (*self).display_message()
    }
}

impl AsIoErrorOrRawCode for i32 {
    fn is_eperm(&self) -> bool {
        *self == 1 || *self == 13 || *self == libc::EPERM
    }

    fn display_message(&self) -> String {
        if *self == 1 {
            "Operation not permitted (os error 1)".to_string()
        } else if *self == 13 {
            "Permission denied (os error 13)".to_string()
        } else {
            format!("os error {self}")
        }
    }
}

impl AsIoErrorOrRawCode for Option<i32> {
    fn is_eperm(&self) -> bool {
        match self {
            Some(code) => code.is_eperm(),
            None => false,
        }
    }

    fn display_message(&self) -> String {
        match self {
            Some(code) => code.display_message(),
            None => "unknown error".to_string(),
        }
    }
}

/// Returns true if `path` is located under the macOS external volumes mount point `/Volumes` (#1776).
pub fn is_external_volume_path(path: &std::path::Path) -> bool {
    if path.starts_with("/Volumes") || path.starts_with("Volumes") {
        return true;
    }
    let mut comps = path.components();
    if let Some(std::path::Component::RootDir) = comps.next() {
        if let Some(std::path::Component::Normal(c)) = comps.next() {
            return c.to_string_lossy().eq_ignore_ascii_case("Volumes");
        }
    } else if let Some(std::path::Component::Normal(c)) = path.components().next() {
        return c.to_string_lossy().eq_ignore_ascii_case("Volumes");
    }
    false
}

/// Formats an actionable diagnostic explaining macOS TCC restrictions when directory creation,
/// working directory access, or command spawn fails with EPERM on an external volume under `/Volumes` (#1776).
pub fn format_volume_access_diagnostic<E: AsIoErrorOrRawCode>(
    path: &std::path::Path,
    err: E,
) -> Option<String> {
    if !is_external_volume_path(path) || !err.is_eperm() {
        return None;
    }
    let err_str = err.display_message();
    Some(format!(
        "macOS TCC (Privacy and Security -> Full Disk Access or Removable Volumes for prod-code-server) \
         blocked access to the external volume at '{}': {} (EPERM, os error 1, PermissionDenied). \
         recommends keeping gateway storage on the internal APFS volume or granting Full Disk Access to the gateway binary \
         (System Settings > Privacy & Security > Full Disk Access).",
        path.display(),
        err_str,
    ))
}

/// Alias for `format_volume_access_diagnostic` taking a target path and error or raw OS error code (#1776).
pub fn format_volume_access_error<E: AsIoErrorOrRawCode>(
    path: &std::path::Path,
    err: E,
) -> Option<String> {
    format_volume_access_diagnostic(path, err)
}

/// Formats a command spawn failure message, enhancing it with macOS TCC remediation guidance if applicable (#1776).
pub fn format_spawn_error(
    run_dir: &std::path::Path,
    program: &str,
    err: &std::io::Error,
) -> String {
    if let Some(diag) = format_volume_access_diagnostic(run_dir, err) {
        format!("failed to start {program}: {diag}")
    } else {
        format!("failed to start {program}: {err}")
    }
}

/// Validates that a working directory is accessible, returning an actionable TCC diagnostic if blocked (#1776).
pub fn check_run_dir_access(run_dir: &std::path::Path) -> Result<(), String> {
    if let Err(e) = std::fs::metadata(run_dir) {
        if let Some(diag) = format_volume_access_diagnostic(run_dir, &e) {
            return Err(diag);
        }
    }
    Ok(())
}

/// Creates a directory and all parent directories, returning an actionable TCC diagnostic if blocked (#1776).
pub fn create_dir_all_checked(path: &std::path::Path) -> std::io::Result<()> {
    match std::fs::create_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) => {
            if let Some(diag) = format_volume_access_diagnostic(path, &e) {
                return Err(std::io::Error::new(e.kind(), diag));
            }
            Err(e)
        }
    }
}
