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

#[cfg(test)]
#[path = "report_tests.rs"]
mod coverage_tests;

/// The arguments of `report-issue`, as given.
pub struct ReportArgs {
    pub title: String,
    pub body: Option<String>,
    pub body_file: Option<PathBuf>,
    pub private_ref: Option<String>,
    pub force: bool,
    pub dry_run: bool,
    pub allow_private_names: bool,
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
            root: &std::env::current_dir()?,
            title: &args.title,
            body: &body,
            force: args.force,
            dry_run: args.dry_run,
            allow_private_names: args.allow_private_names,
            private_ref: args.private_ref.as_deref(),
            labels: &args.labels,
        },
        &prod_code_mcp::report::gh_program(),
    )
    .await?;
    println!("{}", outcome.render());
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn report_issue_help_advertises_roadmap_and_parses_labels() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let help = Cli::try_parse_from(["prod-code", "report-issue", "--help"])
                    .unwrap_err()
                    .to_string();
                assert!(help.contains("roadmap"), "report labels help omits roadmap");
                let parsed = Cli::try_parse_from([
                    "prod-code",
                    "report-issue",
                    "--title",
                    "Synthetic report request",
                    "--label",
                    "enhancement",
                    "--label",
                    "mcp",
                    "--label",
                    "roadmap",
                ])
                .unwrap();
                let Some(Commands::ReportIssue { labels, .. }) = parsed.command else {
                    panic!("expected report command")
                };
                assert_eq!(
                    prod_code_mcp::report::issue_labels(&labels).unwrap(),
                    ["enhancement", "mcp", "roadmap"]
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn report_private_names_override_is_explicit() {
        // Match the CLI parser tests: Clap's full command tree needs a larger debug stack.
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                for allow in [false, true] {
                    let mut args = vec![
                        "prod-code",
                        "report-issue",
                        "--title",
                        "A lookup returned no results",
                        "--body",
                        "A neutral reproduction returned no results instead of a declaration.",
                        "--dry-run",
                    ];
                    if allow {
                        args.push("--allow-private-names");
                    }
                    let parsed = Cli::try_parse_from(args).unwrap();
                    let Some(Commands::ReportIssue {
                        allow_private_names,
                        dry_run,
                        ..
                    }) = parsed.command
                    else {
                        panic!("expected report command")
                    };
                    assert_eq!(allow_private_names, allow);
                    assert!(dry_run);
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
