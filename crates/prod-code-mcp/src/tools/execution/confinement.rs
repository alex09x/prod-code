/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::context::{PATH_HINT_KEYS, git_identity, nested_git_marker};
use anyhow::{Context, Result, bail, ensure};
use std::path::{Component, Path, PathBuf};

const MAX_SYMLINK_DEPTH: usize = 16;

pub(super) fn validate_workspace_paths(
    root: &Path,
    tool_name: &str,
    args: &serde_json::Value,
    has_explicit_workspace: bool,
) -> Result<()> {
    let keys: &[&str] = match tool_name {
        "code_exec" => &["cwd"],
        "code_test" => PATH_HINT_KEYS,
        _ if has_explicit_workspace => PATH_HINT_KEYS,
        _ => return Ok(()),
    };
    for key in keys {
        let Some(value) = args.get(*key) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        let raw = value
            .as_str()
            .with_context(|| format!("'{key}' must be a string path"))?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        validate_single_workspace_path(root, trimmed)?;
    }
    Ok(())
}

fn validate_single_workspace_path(root: &Path, raw: &str) -> Result<()> {
    ensure!(
        !raw.get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:")),
        "file URI paths are not supported; use a filesystem path in the selected workspace"
    );
    let candidate = PathBuf::from(raw);
    ensure!(
        !candidate
            .components()
            .any(|part| part == Component::ParentDir),
        "path or cwd '{raw}' is outside the selected workspace or contains '..' segments"
    );
    let resolved = if candidate.is_absolute() {
        resolve_confined_steps(root, Path::new(""), &candidate, false, 0, raw)?
    } else {
        resolve_confined_steps(root, root, &candidate, true, 0, raw)?
    };
    ensure!(
        resolved.starts_with(root),
        "path or cwd '{raw}' is outside the selected workspace"
    );
    let Some(existing) = deepest_existing_ancestor(&resolved) else {
        return Ok(());
    };
    let directory = if existing.is_dir() {
        existing
    } else {
        existing.parent().context("path has no parent directory")?
    };
    if directory.starts_with(root) {
        match git_identity(directory) {
            Ok((top, _common)) if top != root => bail!(
                "path or cwd '{raw}' lies inside a nested Git worktree or repository; select that checkout as `workspace`"
            ),
            Err(_) if nested_git_marker(root, existing) => bail!(
                "path or cwd '{raw}' is inside a nested Git worktree or repository; select that checkout directly"
            ),
            _ => {}
        }
    }
    Ok(())
}

fn resolve_confined_steps(
    root: &Path,
    base: &Path,
    path: &Path,
    mut inside: bool,
    depth: usize,
    raw: &str,
) -> Result<PathBuf> {
    let mut acc = base.to_path_buf();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => acc.push(prefix.as_os_str()),
            Component::RootDir => acc.push(std::path::MAIN_SEPARATOR_STR),
            Component::CurDir => {}
            Component::ParentDir => {
                if inside && acc == root {
                    bail!("path or cwd '{raw}' is outside the selected workspace");
                }
                acc.pop();
                if inside && !acc.starts_with(root) {
                    bail!("path or cwd '{raw}' is outside the selected workspace");
                }
            }
            Component::Normal(part) => {
                let next = acc.join(part);
                match std::fs::symlink_metadata(&next) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        ensure!(
                            depth < MAX_SYMLINK_DEPTH,
                            "path or cwd '{raw}' exceeds symlink resolution depth"
                        );
                        match std::fs::canonicalize(&next) {
                            Ok(canon) => acc = canon,
                            Err(_) => {
                                let target = std::fs::read_link(&next).with_context(|| {
                                    format!("path or cwd '{raw}' cannot be read")
                                })?;
                                acc = if target.is_absolute() {
                                    resolve_confined_steps(
                                        root,
                                        Path::new(""),
                                        &target,
                                        false,
                                        depth + 1,
                                        raw,
                                    )?
                                } else {
                                    resolve_confined_steps(
                                        root,
                                        &acc,
                                        &target,
                                        inside,
                                        depth + 1,
                                        raw,
                                    )?
                                };
                            }
                        }
                    }
                    Ok(_) => acc = std::fs::canonicalize(&next).unwrap_or(next),
                    Err(_) => acc = next,
                }
                if !inside && acc.starts_with(root) {
                    inside = true;
                } else if inside && !acc.starts_with(root) {
                    bail!("path or cwd '{raw}' is outside the selected workspace");
                }
            }
        }
    }
    Ok(acc)
}

fn deepest_existing_ancestor(path: &Path) -> Option<&Path> {
    path.ancestors().find(|ancestor| ancestor.exists())
}

#[cfg(test)]
mod workspace_confinement_regressions {
    use super::*;

    #[test]
    fn rejects_file_uris_in_path_arguments() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let uri = url::Url::from_file_path(root.join("src.rs"))
            .unwrap()
            .to_string();
        for raw in [
            uri.clone(),
            uri.replacen("file:", "FILE:", 1),
            "file:/outside.rs".into(),
        ] {
            let err = validate_single_workspace_path(&root, &raw).unwrap_err();
            assert!(format!("{err:#}").contains("file URI"), "{raw}: {err:#}");
        }
    }

    #[test]
    fn rejects_absolute_paths_containing_parent_components() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let raw = format!("/missing/..{}/src.rs", root.display());
        let err = validate_single_workspace_path(&root, &raw).unwrap_err();
        assert!(format!("{err:#}").contains(".."), "{err:#}");
    }
}
