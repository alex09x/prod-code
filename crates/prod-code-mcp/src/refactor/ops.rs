/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use super::edits::text_for_edit;
use super::uri::uri_to_root_and_relative;

/// One change of a `WorkspaceEdit`, its paths resolved inside the designated repository root.
pub enum MultiOp {
    Text {
        root: PathBuf,
        rel: String,
        edits: Vec<serde_json::Value>,
    },
    Create {
        root: PathBuf,
        rel: String,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Rename {
        from_root: PathBuf,
        from_rel: String,
        to_root: PathBuf,
        to_rel: String,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Delete {
        root: PathBuf,
        rel: String,
        recursive: bool,
    },
}

/// Every change of `edit`, in the order it names them (`documentChanges` wins over `changes`, as
/// LSP says). Refuses a path outside all designated repository checkouts and a resource operation
/// it does not know, before anything is written.
pub fn multi_operations(
    canonical_roots: &[PathBuf],
    edit: &serde_json::Value,
) -> Result<Vec<MultiOp>> {
    let mut ops = Vec::new();
    if let Some(changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in changes {
            let uri_at = |key: &str| change.get(key).and_then(|u| u.as_str()).unwrap_or("");
            let option = |key: &str| {
                change
                    .pointer(&format!("/options/{key}"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            };
            ops.push(match change.get("kind").and_then(|k| k.as_str()) {
                Some("rename") => {
                    let (from_root, from_rel) =
                        uri_to_root_and_relative(canonical_roots, uri_at("oldUri"))?;
                    let (to_root, to_rel) =
                        uri_to_root_and_relative(canonical_roots, uri_at("newUri"))?;
                    MultiOp::Rename {
                        from_root: from_root.to_path_buf(),
                        from_rel,
                        to_root: to_root.to_path_buf(),
                        to_rel,
                        overwrite: option("overwrite"),
                        ignore_if_exists: option("ignoreIfExists"),
                    }
                }
                Some("create") => {
                    let (root, rel) = uri_to_root_and_relative(canonical_roots, uri_at("uri"))?;
                    MultiOp::Create {
                        root: root.to_path_buf(),
                        rel,
                        overwrite: option("overwrite"),
                        ignore_if_exists: option("ignoreIfExists"),
                    }
                }
                Some("delete") => {
                    let (root, rel) = uri_to_root_and_relative(canonical_roots, uri_at("uri"))?;
                    MultiOp::Delete {
                        root: root.to_path_buf(),
                        rel,
                        recursive: option("recursive"),
                    }
                }
                Some(other) => bail!(
                    "unsupported resource operation `{other}` in the workspace edit; only \
                     create, rename, delete and text edits can be applied, so nothing was written"
                ),
                None => {
                    let (root, rel) = uri_to_root_and_relative(
                        canonical_roots,
                        change
                            .pointer("/textDocument/uri")
                            .and_then(|u| u.as_str())
                            .unwrap_or(""),
                    )?;
                    MultiOp::Text {
                        root: root.to_path_buf(),
                        rel,
                        edits: change
                            .get("edits")
                            .and_then(|e| e.as_array())
                            .cloned()
                            .unwrap_or_default(),
                    }
                }
            });
        }
    } else if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, edits) in changes {
            let (root, rel) = uri_to_root_and_relative(canonical_roots, uri)?;
            ops.push(MultiOp::Text {
                root: root.to_path_buf(),
                rel,
                edits: edits.as_array().cloned().unwrap_or_default(),
            });
        }
    }
    Ok(ops)
}

/// What can be told wrong about an edit before any mutation occurs: a file to edit that is not
/// readable text, or a directory to delete that is not empty without `recursive`. Found here,
/// nothing has been written yet.
pub fn check_multi_ops(ops: &[MultiOp]) -> Result<()> {
    for op in ops {
        match op {
            MultiOp::Text { root, rel, .. } => {
                text_for_edit(&root.join(rel))?;
            }
            MultiOp::Delete {
                root,
                rel,
                recursive: false,
            } => {
                let abs = root.join(rel);
                if abs.is_dir()
                    && std::fs::read_dir(&abs)
                        .with_context(|| format!("cannot read {rel}"))?
                        .next()
                        .is_some()
                {
                    bail!("deleting the directory {rel} needs `recursive: true`, it is not empty");
                }
            }
            _ => {}
        }
    }
    Ok(())
}
