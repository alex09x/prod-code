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
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::scrub::{draft, scrub};
use super::types::{DUPLICATES_SHOWN, Outcome, REPOSITORY, ReportRequest, Similar, issue_labels};

/// Files the issue with the `gh` program `gh`, unless it is a dry run or issues look the same
/// and `force` is not set. `remote` is the node asked for the Environment section.
pub async fn report(
    remote: Option<SocketAddr>,
    request: ReportRequest<'_>,
    gh: &Path,
) -> Result<Outcome> {
    let labels = issue_labels(request.labels)?;
    let node = match remote {
        Some(addr) => crate::cluster::node_status(addr).await.ok(),
        None => None,
    };
    let mut draft = draft(request.title, request.body, node.as_ref())?;
    draft.labels = labels;
    if let Some(reference) = request.private_ref.map(str::trim).filter(|r| !r.is_empty()) {
        draft.body = draft.body.replace(
            "_Filed with `prod-code report-issue`._",
            &format!(
                "Private details: report `{}`.\n_Filed with `prod-code report-issue`._",
                scrub(reference, None, None)
            ),
        );
    }
    if request.dry_run {
        return Ok(Outcome::DryRun(draft));
    }
    if !request.force {
        let similar = search(gh, &draft.title)?;
        if !similar.is_empty() {
            return Ok(Outcome::Similar(draft, similar));
        }
    }
    // The body goes through stdin, so that it is never written to a file on this machine.
    let mut child = std::process::Command::new(gh)
        .args([
            "issue",
            "create",
            "--repo",
            REPOSITORY,
            "--title",
            &draft.title,
        ])
        .args(
            draft
                .labels
                .iter()
                .flat_map(|label| ["--label", label.as_str()]),
        )
        .args(["--body-file", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("running {}", gh.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(draft.body.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    anyhow::ensure!(
        output.status.success(),
        "gh issue create failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let url = String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .find(|line| line.starts_with("https://"))
        .unwrap_or("")
        .to_string();
    anyhow::ensure!(!url.is_empty(), "gh issue create printed no issue URL");
    Ok(Outcome::Filed(url))
}

/// Issues, open or closed, whose titles match the words of `title`.
fn search(gh: &Path, title: &str) -> Result<Vec<Similar>> {
    let query = format!("{title} in:title");
    let output = std::process::Command::new(gh)
        .args([
            "issue", "list", "--repo", REPOSITORY, "--state", "all", "--search",
        ])
        .arg(&query)
        .args(["--json", "number,title,url,state", "--limit"])
        .arg(DUPLICATES_SHOWN.to_string())
        .output()
        .with_context(|| format!("running {}", gh.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "gh issue list failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(serde_json::from_slice(&output.stdout).unwrap_or_default())
}

/// The `gh` program: `PROD_CODE_GH` when set, `gh` from the PATH otherwise.
pub fn gh_program() -> PathBuf {
    std::env::var_os("PROD_CODE_GH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("gh"))
}
