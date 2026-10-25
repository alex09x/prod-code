/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

use anyhow::Result;

use super::plan::plan_in;
use super::types::{Fixed, Outcome};

/// Runs `kind` (check or lint), applies every machine-applicable fix it reports to the
/// checkout, and runs it again to show what is left.
pub async fn check_and_fix(
    remote: SocketAddr,
    root: &Path,
    hint: Option<&Path>,
    kind: crate::verify::VerifyKind,
    timeout_secs: u64,
) -> Result<Fixed> {
    let before = crate::verify::run_verify(remote, root, hint, kind, None, timeout_secs).await?;
    if before.language != "rust" {
        return tool_fix(remote, root, hint, kind, timeout_secs, before).await;
    }
    let (subdir, _) = crate::sync::engine_project(root, hint.unwrap_or(root));
    let project = subdir.as_ref().map_or(root.to_path_buf(), |s| root.join(s));
    let (files, outcomes) = plan_in(root, &project, &before.fixes);
    if files.is_empty() {
        return Ok(Fixed {
            before,
            outcomes,
            after: None,
            note: None,
        });
    }
    crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
    let after = crate::verify::run_verify(remote, root, hint, kind, None, timeout_secs).await?;
    Ok(Fixed {
        before,
        outcomes,
        after: Some(after),
        note: None,
    })
}

/// `lint --fix` for a language whose linter fixes by itself (#205): its fix mode runs on the
/// node, the files it rewrote come back into the checkout, and the lint runs again.
async fn tool_fix(
    remote: SocketAddr,
    root: &Path,
    hint: Option<&Path>,
    kind: crate::verify::VerifyKind,
    timeout_secs: u64,
    before: crate::verify::VerifyReport,
) -> Result<Fixed> {
    let language = before.language.clone();
    let nothing = |before, note: String| Fixed {
        before,
        outcomes: Vec::new(),
        after: None,
        note: Some(note),
    };
    if kind != crate::verify::VerifyKind::Lint {
        return Ok(nothing(
            before,
            format!("fixes: only `lint --fix` applies fixes for {language}"),
        ));
    }
    let (subdir, _) = crate::sync::engine_project(root, hint.unwrap_or(root));
    let project = subdir.as_ref().map_or(root.to_path_buf(), |s| root.join(s));
    let tools = crate::verify::detect_tools(&project);
    let Some(command) = crate::verify::fix_command(&tools, &language)? else {
        return Ok(nothing(
            before,
            format!("fixes: the {language} linter has no fix mode; nothing was changed"),
        ));
    };
    let mut tail = crate::exec::TailBuffer::new(8 * 1024);
    let outcome = crate::exec::run_remote(
        remote,
        root,
        subdir.as_deref(),
        command.clone(),
        vec![("NO_COLOR".to_string(), "1".to_string())],
        timeout_secs,
        true,
        |_, bytes| tail.push(bytes),
    )
    .await?;
    let failure = if let Some(error) = &outcome.exit.error {
        Some(format!("failed to start: {error}"))
    } else if outcome.exit.timed_out {
        Some("timed out".to_string())
    } else if outcome.exit.exit_code != Some(0) {
        Some(match outcome.exit.exit_code {
            Some(code) => format!("failed with exit code {code}"),
            None => "failed: exit code is unknown".to_string(),
        })
    } else {
        None
    };
    if let Some(failure) = failure {
        anyhow::bail!(
            "fixes: `{}` {failure}; {} changed file(s) already pulled: {}\n{}",
            command.join(" "),
            outcome.pulled_files.len(),
            outcome.pulled_files.join(", "),
            tail.text()
        );
    }
    let outcomes: Vec<Outcome> = outcome
        .pulled_files
        .iter()
        .map(|file| Outcome {
            file: file.clone(),
            line: 0,
            message: format!("rewritten by `{}`", command.join(" ")),
            skipped: None,
        })
        .collect();
    let note = format!(
        "fixes: `{}` rewrote {} file(s)",
        command.join(" "),
        outcomes.len()
    );
    let after = if outcomes.is_empty() {
        None
    } else {
        Some(crate::verify::run_verify(remote, root, hint, kind, None, timeout_secs).await?)
    };
    Ok(Fixed {
        before,
        outcomes,
        after,
        note: Some(note),
    })
}
