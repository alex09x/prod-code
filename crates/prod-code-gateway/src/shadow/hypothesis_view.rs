/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::FileDelta;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::staging::safe_relative;

/// Symlinks followed while resolving one path before giving up, as Linux does (`ELOOP`).
pub(crate) const MAX_SYMLINKS: usize = 40;

/// What a path is in the hypothesis's mount namespace.
pub(crate) enum Node<'a> {
    Missing,
    /// `upper_only`: a staged directory that hides a non-directory below it, so the workspace
    /// copy contributes nothing under it.
    Dir {
        upper_only: bool,
    },
    Link(PathBuf),
    Proposed(&'a [u8]),
    Disk,
}

pub(crate) enum Step {
    Root,
    Up,
    Name(OsString),
}

pub(crate) fn steps(path: &Path) -> Vec<Step> {
    path.components()
        .filter_map(|c| match c {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => Some(Step::Root),
            std::path::Component::CurDir => None,
            std::path::Component::ParentDir => Some(Step::Up),
            std::path::Component::Normal(name) => Some(Step::Name(name.to_os_string())),
        })
        .collect()
}

/// The filesystem as an overlay hypothesis sees it: at the workspace path, the proposed files
/// (staged as regular files, so one replaces a symlink rather than writing through it) over the
/// workspace copy, minus the deletions the run script applies after mounting; everywhere else,
/// the gateway's own filesystem. It models only hypotheses `check_files` accepted, which is
/// checked first in `run_overlay`: no parent symlinks, no duplicate or nested proposals.
pub(crate) struct HypothesisView<'a> {
    /// Where the overlay is mounted: the workspace path with its symlinks resolved.
    workspace: PathBuf,
    proposed: Vec<(PathBuf, &'a [u8])>,
    /// Physical paths `rm -f` removes, in the view without symlinks.
    deleted: Vec<PathBuf>,
}

impl<'a> HypothesisView<'a> {
    pub(crate) fn new(workspace: &Path, files: &'a [FileDelta]) -> Self {
        let workspace =
            std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
        let mut view = Self {
            workspace,
            proposed: Vec::new(),
            deleted: Vec::new(),
        };
        let mut deletions = Vec::new();
        for file in files {
            // An invalid path fails staging, so the command never runs.
            let Some(rel) = safe_relative(&file.relative_path) else {
                continue;
            };
            match &file.content {
                Some(bytes) => view.proposed.push((rel, bytes.as_slice())),
                None => deletions.push(rel),
            }
        }
        // Each `rm -f` removes the last component of its path itself, not what a symlink there
        // points to.
        for rel in deletions {
            let path = view.workspace.join(rel);
            match view.resolve(&path, false) {
                Ok((_, Node::Missing)) | Err(_) => {}
                Ok((target, _)) => view.deleted.push(target),
            }
        }
        view
    }

    /// Follows an absolute path component by component as the kernel does inside the
    /// namespace, through `..` and symlinks (the last one only with `follow_last`), and returns
    /// the physical path reached and what is there.
    pub(crate) fn resolve(
        &self,
        path: &Path,
        follow_last: bool,
    ) -> std::result::Result<(PathBuf, Node<'a>), String> {
        if !path.is_absolute() {
            return Err("it is not an absolute path".to_string());
        }
        let mut pending = steps(path);
        pending.reverse();
        let mut cur = PathBuf::from("/");
        // One entry per component of `cur`: whether it is an upper-only directory.
        let mut upper_only: Vec<bool> = Vec::new();
        let mut links = 0;
        while let Some(step) = pending.pop() {
            let name = match step {
                Step::Root => {
                    cur = PathBuf::from("/");
                    upper_only.clear();
                    continue;
                }
                Step::Up => {
                    if cur.pop() {
                        upper_only.pop();
                    }
                    continue;
                }
                Step::Name(name) => name,
            };
            let next = cur.join(&name);
            let last = pending.is_empty();
            match self.node(&next, upper_only.last().copied().unwrap_or(false))? {
                Node::Dir { upper_only: hidden } => {
                    cur = next;
                    upper_only.push(hidden);
                }
                Node::Link(target) if follow_last || !last => {
                    links += 1;
                    if links > MAX_SYMLINKS {
                        return Err("too many levels of symbolic links".to_string());
                    }
                    pending.extend(steps(&target).into_iter().rev());
                }
                node if last => return Ok((next, node)),
                Node::Missing => return Ok((next, Node::Missing)),
                _ => return Err("a component of the path is not a directory".to_string()),
            }
        }
        let hidden = upper_only.last().copied().unwrap_or(false);
        Ok((cur, Node::Dir { upper_only: hidden }))
    }

    /// What `path`, whose parent is a directory of the view without symlinks, is. `upper_only`
    /// says the parent is a staged directory that hides the workspace copy.
    fn node(&self, path: &Path, upper_only: bool) -> std::result::Result<Node<'a>, String> {
        if self.deleted.iter().any(|d| path.starts_with(d)) {
            return Ok(Node::Missing);
        }
        if let Ok(rel) = path.strip_prefix(&self.workspace) {
            // Staged in order, so the last copy of a path wins.
            if let Some((_, bytes)) = self.proposed.iter().rev().find(|(p, _)| p == rel) {
                return Ok(Node::Proposed(bytes));
            }
            if self.proposed.iter().any(|(p, _)| p.starts_with(rel)) {
                // A staged directory merges with a directory below it and hides anything else.
                let merged =
                    !upper_only && std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
                return Ok(Node::Dir {
                    upper_only: !merged,
                });
            }
            if upper_only {
                return Ok(Node::Missing);
            }
        }
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => std::fs::read_link(path)
                .map(Node::Link)
                .map_err(|e| e.kind().to_string()),
            Ok(m) if m.is_dir() => Ok(Node::Dir { upper_only: false }),
            Ok(_) => Ok(Node::Disk),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Node::Missing),
            Err(e) => Err(e.kind().to_string()),
        }
    }
}
