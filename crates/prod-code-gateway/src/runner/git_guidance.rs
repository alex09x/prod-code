/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::ExecRequest;
use std::path::Path;

/// Explain the mirror boundary without copying Git metadata, rerunning locally or
/// changing the requested command and its exit status (#1799).
pub(super) fn metadata_guidance(
    request: &ExecRequest,
    run_dir: &Path,
    workspace: &Path,
) -> Option<&'static str> {
    let program = request.command.first()?;
    if Path::new(program).file_name()?.to_str()? != "git"
        || request
            .env
            .iter()
            .any(|(key, _)| matches!(key.as_str(), "GIT_DIR" | "GIT_WORK_TREE"))
    {
        return None;
    }
    let args = &request.command[1..];
    if args.iter().any(|arg| {
        arg == "--no-index"
            || arg.starts_with("--git-dir")
            || arg.starts_with("--work-tree")
            || arg.starts_with("-C")
    }) {
        return None; // The caller selected a different repository, or no repository at all.
    }
    let command = args.iter().find(|arg| !arg.starts_with('-'))?;
    if !matches!(
        command.as_str(),
        "shortlog"
            | "log"
            | "show"
            | "status"
            | "diff"
            | "rev-parse"
            | "describe"
            | "ls-files"
            | "ls-tree"
            | "blame"
            | "reflog"
            | "rev-list"
    ) {
        return None;
    }
    for directory in run_dir
        .ancestors()
        .take_while(|path| path.starts_with(workspace))
    {
        if directory.join(".git").exists() {
            return None;
        }
    }
    Some(
        "[prod-code exec] This workspace is a file mirror without Git metadata. \
         Commands needing the repository history or index may fail here; run read-only Git \
         validation locally in the original checkout. The requested command still runs remotely \
         and keeps its original exit status.\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command: &[&str]) -> ExecRequest {
        ExecRequest {
            client_workspace_root: "fixture".to_string(),
            base_workspace_name: None,
            command: command.iter().map(ToString::to_string).collect(),
            env: Vec::new(),
            timeout_secs: 10,
            pull_changes: false,
            subdir: None,
            client_agent: None,
            client_host: None,
        }
    }

    #[test]
    fn issue_1799_guidance_respects_explicit_repository_and_no_index() {
        let root = tempfile::tempdir().unwrap();
        for command in [
            vec!["git", "--git-dir=elsewhere", "status"],
            vec!["git", "-C", "elsewhere", "status"],
            vec!["git", "diff", "--no-index", "one", "two"],
            vec!["git", "--version"],
            vec!["git", "init"],
            vec!["not-git", "status"],
        ] {
            assert!(metadata_guidance(&request(&command), root.path(), root.path()).is_none());
        }
        let mut req = request(&["git", "status"]);
        req.env.push(("GIT_DIR".into(), "elsewhere".into()));
        assert!(metadata_guidance(&req, root.path(), root.path()).is_none());
    }

    #[test]
    fn issue_1799_guidance_accepts_git_marker_in_mirrored_parent() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("src");
        std::fs::create_dir(&nested).unwrap();
        let req = request(&["/usr/bin/git", "--no-pager", "log"]);
        assert!(metadata_guidance(&req, &nested, root.path()).is_some());
        std::fs::write(root.path().join(".git"), "gitdir: external").unwrap();
        assert!(metadata_guidance(&req, &nested, root.path()).is_none());
    }
}
