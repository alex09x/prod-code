/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use super::super::types::VerifyKind;

pub fn narrow_scope(command: &mut Vec<String>, language: &str, project_dir: &Path, hint: &Path) {
    let canon_dir =
        std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    // A hint that is not a path may be a crate / package name ("prod-code-gateway").
    let hint_owned;
    let hint = if hint.exists() {
        hint
    } else if let Some(dir) = member_dir_named(&canon_dir, hint) {
        hint_owned = dir;
        &hint_owned
    } else {
        return;
    };
    let target = std::fs::canonicalize(hint).unwrap_or_else(|_| hint.to_path_buf());
    let dir = if target.is_file() {
        target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| target.clone())
    } else {
        target.clone()
    };
    if !dir.starts_with(&canon_dir) || dir == canon_dir {
        return;
    }
    let rel_of = |p: &Path| -> String {
        p.strip_prefix(&canon_dir)
            .map(|r| {
                r.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default()
    };
    match language {
        "rust" => {
            let mut probe = dir.clone();
            while probe.starts_with(&canon_dir) && probe != canon_dir {
                if let Some(name) = cargo_package_name(&probe.join("Cargo.toml")) {
                    if let Some(i) = command.iter().position(|a| a == "--workspace") {
                        command.splice(i..=i, ["-p".to_string(), name]);
                    }
                    return;
                }
                match probe.parent() {
                    Some(parent) => probe = parent.to_path_buf(),
                    None => break,
                }
            }
        }
        "go" => {
            if let Some(i) = command.iter().position(|a| a == "./...") {
                command[i] = format!("./{}/...", rel_of(&dir));
            }
        }
        "python" if command.iter().any(|a| a == "pytest") => {
            command.push(rel_of(&target));
        }
        _ => {}
    }
}

/// The directory of a workspace member whose Cargo package name or directory name is the
/// last component of `hint` (two levels deep: `crates/x`, `x`).
pub fn member_dir_named(root: &Path, hint: &Path) -> Option<PathBuf> {
    let wanted = hint.file_name()?.to_string_lossy().into_owned();
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        candidates.push(path.clone());
        if let Ok(children) = std::fs::read_dir(&path) {
            candidates.extend(children.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
    }
    candidates.into_iter().find(|dir| {
        dir.file_name()
            .map(|n| n.to_string_lossy() == wanted)
            .unwrap_or(false)
            || cargo_package_name(&dir.join("Cargo.toml")).as_deref() == Some(wanted.as_str())
    })
}

/// `[package] name` of a Cargo manifest (None for a workspace-only or missing manifest).
pub fn cargo_package_name(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package
            && let Some(rest) = line.strip_prefix("name")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

pub fn has_xcode_project(root: &Path) -> bool {
    if root.join("Package.swift").exists() {
        return false;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".xcodeproj") || name.ends_with(".xcworkspace")
            })
        })
        .unwrap_or(false)
}

/// `xcodebuild build` / `xcodebuild test` for an Xcode project: the scheme is `filter` when
/// given, otherwise the first scheme `xcodebuild -list` reports; iOS targets run on the
/// first available iPhone simulator, everything else on the Mac.
pub fn plan_xcode_command(kind: VerifyKind, scheme: Option<&str>) -> Result<Vec<String>> {
    let action = match kind {
        VerifyKind::Check => "build",
        VerifyKind::Test => "test",
        VerifyKind::Lint => return Err(anyhow!("no lint command for Xcode projects")),
        VerifyKind::Bench => return Err(anyhow!("no bench command for Xcode projects")),
    };
    let scheme_expr = match scheme.filter(|s| !s.is_empty()) {
        Some(s) => format!("'{}'", s.replace('\'', "'\\''")),
        None => "\"$(xcodebuild -list -json 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); d=d.get(\"project\") or d.get(\"workspace\"); print(d[\"schemes\"][0])')\"".to_string(),
    };
    let script = format!(
        "scheme={scheme_expr}; \
if xcodebuild -showBuildSettings -scheme \"$scheme\" 2>/dev/null | grep -q 'SDKROOT.*iPhoneOS'; then \
  dest=\"platform=iOS Simulator,name=$(xcrun simctl list devices available | grep -m1 -o 'iPhone[^(]*' | sed 's/ *$//')\"; \
else dest='platform=macOS'; fi; \
xcodebuild {action} -scheme \"$scheme\" -destination \"$dest\" -quiet 2>&1"
    );
    Ok(vec!["sh".to_string(), "-c".to_string(), script])
}
