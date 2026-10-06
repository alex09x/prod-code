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
use prod_code_mcp::report::ReportRequest;
use std::net::SocketAddr;
use std::path::PathBuf;

/// The arguments of `report-issue`, as given.
pub struct ReportArgs {
    pub title: String,
    pub body: Option<String>,
    pub body_file: Option<PathBuf>,
    pub private_ref: Option<String>,
    pub force: bool,
    pub dry_run: bool,
    pub labels: Vec<String>,
}

/// Files (or drafts) a prod-code bug report.
pub async fn run_report_issue(
    remote: Option<SocketAddr>,
    unplaced: Option<String>,
    args: ReportArgs,
) -> Result<()> {
    let mut body = match (args.body, args.body_file) {
        (Some(body), _) => body,
        (None, Some(path)) if path.as_os_str() == "-" => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
            text
        }
        (None, Some(path)) => std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?,
        (None, None) => anyhow::bail!("give the report a --body or a --body-file"),
    };
    if let Some(reason) = unplaced {
        body.push_str(&format!(
            "\n\nThis checkout could not be placed on a node: {}",
            reason.replace('\n', " ")
        ));
    }
    let outcome = prod_code_mcp::report::report(
        remote,
        ReportRequest {
            title: &args.title,
            body: &body,
            force: args.force,
            dry_run: args.dry_run,
            private_ref: args.private_ref.as_deref(),
            labels: &args.labels,
        },
        &prod_code_mcp::report::gh_program(),
    )
    .await?;
    println!("{}", outcome.render());
    Ok(())
}
