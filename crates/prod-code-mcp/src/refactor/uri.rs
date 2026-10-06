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

use anyhow::{Result, anyhow, bail};
use url::Url;

pub fn uri_to_root_and_relative<'a>(
    canonical_roots: &'a [PathBuf],
    uri: &str,
) -> Result<(&'a Path, String)> {
    let path = Url::parse(uri)
        .ok()
        .and_then(|u| u.to_file_path().ok())
        .ok_or_else(|| anyhow!("not a file URI: {uri}"))?;
    let path = resolve(&path)?;
    let mut best_match: Option<(&Path, &Path)> = None;
    for root in canonical_roots {
        if let Ok(rel) = path.strip_prefix(root) {
            if rel.as_os_str().is_empty() {
                bail!("{uri} names the checkout itself, not a file in it");
            }
            match &best_match {
                Some((best_root, _)) if root.as_os_str().len() <= best_root.as_os_str().len() => {}
                _ => best_match = Some((root.as_path(), rel)),
            }
        }
    }
    let (matched_root, rel) = match best_match {
        Some((r, rel)) => (r, rel),
        None => {
            if canonical_roots.len() == 1 {
                bail!(
                    "{} is outside the checkout {}",
                    path.display(),
                    canonical_roots[0].display()
                );
            } else {
                bail!(
                    "{} is outside any of the specified repository roots ({})",
                    path.display(),
                    canonical_roots
                        .iter()
                        .map(|r| r.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    };
    Ok((matched_root, rel.to_string_lossy().replace('\\', "/")))
}

#[allow(dead_code)]
pub fn uri_to_relative(root: &Path, uri: &str) -> Result<String> {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let (_, rel) = uri_to_root_and_relative(&[canonical], uri)?;
    Ok(rel)
}

/// `root/rel`, refused unless it still resolves inside the checkout. Every path was resolved
/// before the first step, against the checkout as it was; an earlier step can move a directory
/// that holds a symlink so that a later path runs through it, so each step checks its paths
/// again right before it acts on them.
pub fn contained(root: &Path, rel: &str) -> Result<PathBuf> {
    let abs = root.join(rel);
    let real = resolve(&abs)?;
    anyhow::ensure!(
        real.starts_with(root),
        "{rel} leads outside the checkout, to {}, after the earlier steps of the edit",
        real.display()
    );
    Ok(abs)
}

/// `path` with every symlink on it resolved, including those above a part that does not exist
/// yet: a new file under a symlinked directory lands where the symlink points, and that is what
/// has to be inside the checkout. A symlink that cannot be resolved is refused, since writing
/// through it could land anywhere.
pub fn resolve(path: &Path) -> Result<PathBuf> {
    let mut missing = Vec::new();
    let mut at = path.to_path_buf();
    loop {
        match std::fs::canonicalize(&at) {
            Ok(mut real) => {
                real.extend(missing.iter().rev());
                return Ok(real);
            }
            Err(err) => {
                if std::fs::symlink_metadata(&at).is_ok() {
                    bail!("{} cannot be resolved: {err}", at.display());
                }
                let (Some(parent), Some(name)) = (at.parent(), at.file_name()) else {
                    bail!("{} has no existing directory above it", path.display());
                };
                missing.push(name.to_os_string());
                at = parent.to_path_buf();
            }
        }
    }
}
