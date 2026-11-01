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

/// The project subdirectory and language to verify for `project_hint` under `root`.
///
/// `engine_project` keeps a root-level source file of another language (such as `sample.py`
/// alongside a root `tsconfig.json`) attached to the root engine so LSP session pooling does
/// not split the root directory key, whereas an explicitly scoped verification target must
/// run that file's own language checker (#1778).
pub fn verify_project(
    root: &Path,
    project_hint: Option<&Path>,
) -> (Option<String>, Option<&'static str>) {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let resolved_hint = project_hint.map(|hint| {
        if let Ok(rel) = hint.strip_prefix(root) {
            canonical_root.join(rel)
        } else if hint.is_absolute() {
            hint.to_path_buf()
        } else {
            canonical_root.join(hint)
        }
    });
    let (subdir, mut language) =
        crate::sync::engine_project(root, resolved_hint.as_deref().unwrap_or(root));
    if let Some(hint) = resolved_hint.as_deref()
        && subdir.is_none()
    {
        let target = std::fs::canonicalize(hint).unwrap_or_else(|_| hint.to_path_buf());
        let missing = matches!(
            std::fs::symlink_metadata(&target),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        );
        let confined = if missing {
            crate::sync::engine::workspace::missing_path_is_confined_to(&canonical_root, &target)
        } else {
            target.starts_with(&canonical_root) && target != canonical_root
        };
        if confined
            && !target.is_dir()
            && member_dir_named(&canonical_root, &target).is_none()
            && !crate::sync::is_in_dependency_dir(&target)
            && let Some(own) = crate::sync::engine_for_file(&target)
            && Some(own) != language
            && !(language == Some("swift") && own == "cpp")
            && !(matches!(language, Some("vue" | "svelte")) && own == "typescript")
            && !(language == Some("kotlin") && own == "java")
            && !matches!(own, "markdown" | "yaml" | "toml" | "json" | "html" | "css")
        {
            language = Some(own);
        }
    }
    (subdir, language)
}

pub fn narrow_scope(command: &mut Vec<String>, language: &str, project_dir: &Path, hint: &Path) {
    let canon_dir =
        std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    // A hint that is not a path may be a crate / package name ("prod-code-gateway").
    let hint_owned;
    let hint = if hint.exists() {
        hint
    } else if hint.is_relative() && canon_dir.join(hint).exists() {
        hint_owned = canon_dir.join(hint);
        &hint_owned
    } else if let Some(dir) = member_dir_named(&canon_dir, hint) {
        hint_owned = dir;
        &hint_owned
    } else {
        return;
    };
    let target = std::fs::canonicalize(hint).unwrap_or_else(|_| hint.to_path_buf());
    if !target.starts_with(&canon_dir) || target == canon_dir {
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
    if language == "python" {
        let rel = rel_of(&target);
        if !rel.is_empty() {
            if command.iter().any(|a| a == "pytest" || a == "basedpyright") {
                command.push(rel);
            } else if command.iter().any(|a| a == "ruff")
                && let Some(dot) = command.iter_mut().find(|a| a.as_str() == ".")
            {
                *dot = rel;
            }
        }
        return;
    }
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
    Ok(vec![
        "sh".to_string(),
        "-c".to_string(),
        xcode_script(kind, scheme, "")?,
    ])
}

/// An XcodeGen project not generated yet: a `project.yml` that declares `targets:`, and neither
/// a generated Xcode project nor a `Package.swift` beside it.
pub fn is_xcodegen_spec(root: &Path) -> bool {
    !root.join("Package.swift").exists()
        && !has_xcode_project(root)
        && std::fs::read_to_string(root.join("project.yml"))
            .is_ok_and(|text| text.lines().any(|line| line.starts_with("targets:")))
}

/// [`plan_xcode_command`] for an XcodeGen spec: the Xcode project is generated from
/// `project.yml` into a temporary directory first, so that the checkout is not written to, and
/// every `xcodebuild` call names it. A node without `xcodegen` fails with exit 127 and says so;
/// a spec it cannot generate fails with its error (#1481).
///
/// xcodegen writes the newest project format by default, which an older Xcode cannot open
/// (Xcode 15.4 refuses format 77). When the spec does not choose `projectFormat`, the node's
/// Xcode decides it, through a spec that includes `project.yml` and adds only that option.
pub fn plan_xcodegen_command(kind: VerifyKind, scheme: Option<&str>) -> Result<Vec<String>> {
    let build = xcode_script(kind, scheme, " -project \"$proj\"")?;
    let script = format!(
        "command -v xcodegen >/dev/null 2>&1 || {{ echo 'prod-code: project.yml is an XcodeGen \
spec and xcodegen is not installed on this node; install it (brew install xcodegen) or commit the \
generated Xcode project' >&2; exit 127; }}; \
gen=$(mktemp -d) || exit 1; trap 'rm -rf \"$gen\"' EXIT; spec=project.yml; \
case \"$(xcodebuild -version 2>/dev/null | sed -n '1s/^Xcode //p')\" in \
14.*) fmt=xcode14_0;; 15.0*|15.1*|15.2*) fmt=xcode15_0;; 15.*) fmt=xcode15_3;; *) fmt=;; esac; \
if [ -n \"$fmt\" ] && ! grep -q '^ *projectFormat:' project.yml; then spec=\"$gen/spec.yml\"; \
printf 'include:\\n  - path: %s\\noptions:\\n  projectFormat: %s\\n' \"$PWD/project.yml\" \"$fmt\" > \"$spec\"; fi; \
xcodegen generate --quiet --spec \"$spec\" --project \"$gen\" || {{ status=$?; echo \
'prod-code: xcodegen could not generate the Xcode project from project.yml' >&2; exit $status; }}; \
proj=$(ls -d \"$gen\"/*.xcodeproj | head -n 1); {build}"
    );
    Ok(vec!["sh".to_string(), "-c".to_string(), script])
}

/// The `sh -c` script of [`plan_xcode_command`]; `project` is put after every `xcodebuild`.
fn xcode_script(kind: VerifyKind, scheme: Option<&str>, project: &str) -> Result<String> {
    let action = match kind {
        VerifyKind::Check => "build",
        VerifyKind::Test => "test",
        VerifyKind::Lint => return Err(anyhow!("no lint command for Xcode projects")),
        VerifyKind::Bench => return Err(anyhow!("no bench command for Xcode projects")),
    };
    let scheme_expr = match scheme.filter(|s| !s.is_empty()) {
        Some(s) => format!("'{}'", s.replace('\'', "'\\''")),
        None => format!(
            "\"$(xcodebuild{project} -list -json 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); d=d.get(\"project\") or d.get(\"workspace\"); print(d[\"schemes\"][0])')\""
        ),
    };
    Ok(format!(
        "scheme={scheme_expr}; \
if xcodebuild{project} -showBuildSettings -scheme \"$scheme\" 2>/dev/null | grep -q 'SDKROOT.*iPhoneOS'; then \
  dest=\"platform=iOS Simulator,name=$(xcrun simctl list devices available | grep -m1 -o 'iPhone[^(]*' | sed 's/ *$//')\"; \
else dest='platform=macOS'; fi; \
xcodebuild {action}{project} -scheme \"$scheme\" -destination \"$dest\" -quiet 2>&1"
    ))
}
