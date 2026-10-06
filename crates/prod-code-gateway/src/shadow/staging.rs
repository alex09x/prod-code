/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use prod_code_protocol::{FileDelta, ShadowHypothesisResult};
use std::path::{Path, PathBuf};

use super::types::HYPOTHESIS_DIR_PREFIX;

/// A relative path that stays inside the workspace, or `None`. Control characters are refused:
/// the overlay's deletion list is line-based, and `name\n../outside` would otherwise become a
/// second, unchecked path there (#440).
pub(crate) fn safe_relative(path: &str) -> Option<PathBuf> {
    let p = Path::new(path);
    if path.is_empty() || p.is_absolute() || path.chars().any(char::is_control) {
        return None;
    }
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            std::path::Component::Normal(s) => out.push(s),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// The workspace-relative paths of a hypothesis's files, in order, once they are known to
/// apply the same way in the overlay and in place (see the module docs); otherwise why not.
/// Only the workspace copy is inspected, nothing is written.
pub(crate) fn check_files(
    workspace: &Path,
    files: &[FileDelta],
) -> std::result::Result<Vec<PathBuf>, String> {
    let mut paths: Vec<PathBuf> = Vec::with_capacity(files.len());
    for file in files {
        let rel = safe_relative(&file.relative_path).ok_or_else(|| {
            format!(
                "invalid path {:?}: it must be relative, stay inside the workspace and hold \
                 no control character",
                file.relative_path
            )
        })?;
        if paths.contains(&rel) {
            return Err(format!("{rel:?} is proposed more than once"));
        }
        if let Some(other) = paths
            .iter()
            .find(|p| p.starts_with(&rel) || rel.starts_with(p))
        {
            return Err(format!(
                "{rel:?} and {other:?} are both proposed, and one lies inside the other"
            ));
        }
        paths.push(rel);
    }
    for rel in &paths {
        let mut cur = workspace.to_path_buf();
        let depth = rel.components().count();
        for (i, component) in rel.components().enumerate() {
            cur.push(component);
            let last = i + 1 == depth;
            let meta = match std::fs::symlink_metadata(&cur) {
                Ok(meta) => meta,
                // Created by the hypothesis, or a deletion of nothing: the same in both modes.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => return Err(format!("cannot inspect {cur:?}: {e}")),
            };
            let kind = meta.file_type();
            if !last {
                if kind.is_symlink() {
                    return Err(format!(
                        "a parent of {rel:?} is a symbolic link; shadow runs do not write \
                         through symlinked directories"
                    ));
                }
                if !kind.is_dir() {
                    return Err(format!("a parent of {rel:?} is not a directory"));
                }
            } else if kind.is_dir() {
                return Err(format!(
                    "{rel:?} is a directory; shadow runs replace or delete files only"
                ));
            } else if !kind.is_file() && !kind.is_symlink() {
                return Err(format!(
                    "{rel:?} is neither a regular file nor a symbolic link"
                ));
            }
        }
    }
    Ok(paths)
}

/// Writes the hypothesis's files into the overlay upper directory (they shadow the workspace
/// copy once mounted) and returns the paths to delete inside the mount.
pub(crate) fn stage_upper(upper: &Path, files: &[FileDelta]) -> Result<Vec<PathBuf>> {
    let mut deleted = Vec::new();
    for file in files {
        let rel = safe_relative(&file.relative_path)
            .with_context(|| format!("invalid path {:?}", file.relative_path))?;
        match &file.content {
            Some(bytes) => {
                let dest = upper.join(&rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, bytes)?;
                #[cfg(unix)]
                if file.is_executable {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
                }
            }
            None => deleted.push(rel),
        }
    }
    Ok(deleted)
}

pub(crate) fn dir_name(workspace: &Path, hypothesis: &str, nonce: u64) -> String {
    let ws = workspace
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".to_string());
    let safe: String = hypothesis
        .chars()
        .take(48)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{HYPOTHESIS_DIR_PREFIX}{ws}--{safe}-{nonce:x}")
}

pub(crate) fn failed(name: &str, error: String) -> ShadowHypothesisResult {
    ShadowHypothesisResult {
        name: name.to_string(),
        exit_code: None,
        duration_ms: 0,
        timed_out: false,
        error: Some(error),
        output_tail: None,
        output_len: 0,
    }
}
