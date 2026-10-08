/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! prod-code client: ultra-thin CLI bridge for editors and AI coding agents over 10G LAN.

use anyhow::{Context, Result};
use clap::Parser;
use std::env;

mod cert;
mod cli;
pub mod commands;
mod lsp_bridge;
mod package;
mod timing;
mod update;
mod validation;
pub mod workspace;

use cli::*;
use commands::*;
use lsp_bridge::*;
use timing::*;
use workspace::*;

#[tokio::main]
async fn main() {
    if let Err(e) = run_cli().await {
        eprintln!("Error: {e:#}");
        eprintln!(
            "\n💡 If this is an unexpected error or a bug in prod-code, please report it:\n\
             - Via CLI: prod-code report-issue --title \"...\" --body \"...\" (automatically sanitizes hostnames, LAN addresses, and home paths)\n\
             - On GitHub: https://github.com/alex09x/prod-code/issues (manually remove hostnames, LAN addresses, home paths, and credentials before posting)"
        );
        std::process::exit(1);
    }
}

async fn run_cli() -> Result<()> {
    // Everything down to the dispatch below runs on every invocation, whatever the
    // subcommand, and until this timer existed none of it was measured: the query timer
    // starts after it. See the report on stderr under `PROD_CODE_TIMING=1`.
    let mut startup = QueryTiming::labelled("startup");
    let cli = Cli::parse();
    startup.mark("parse_args");

    let pinned = env::args().any(|a| a == "-r" || a == "--remote" || a.starts_with("--remote="));
    let mut remote_spec = cli.remote.clone();
    if !pinned
        && env::var_os("PROD_CODE_REMOTE").is_none()
        && env::var_os("PROD_CODE_CLUSTER").is_some()
    {
        remote_spec = "auto".to_string();
    }
    let seeds = prod_code_mcp::cluster::parse_remotes(&remote_spec)?;
    // Nodes named with `--remote` on the command line are the ones to use, as given (#125);
    // otherwise one seed is enough and the rest of the cluster comes from its gossip view.
    let remotes = if pinned {
        seeds.clone()
    } else {
        prod_code_mcp::cluster::discover_nodes(&seeds).await
    };
    startup.mark("discover_nodes");

    let cwd_root = env::current_dir()
        .ok()
        .map(|d| find_workspace_root(&d).unwrap_or(d));
    startup.mark("workspace_root");
    // Placement follows the origin repository: every worktree lands on the node that holds
    // the origin's copy, so seeding from that copy and the shared cargo target directory
    // work. The workspace name itself stays per worktree (`<repo>--wt-<hash>`).
    let cwd_workspace = cwd_root
        .as_deref()
        .map(|root| {
            let identity = prod_code_mcp::sync::workspace_identity(root);
            identity.base.unwrap_or(identity.name)
        })
        .unwrap_or_default();
    startup.mark("workspace_identity");
    // The engine a query needs is that of the nearest project of the file it names (or of
    // the current directory): a SwiftPM package inside a Rust repository must land on a
    // macOS node even though the repository root is Rust.
    let (cwd_subproject, cwd_engine) = cwd_root
        .as_deref()
        .map(|root| {
            let tokens = command_path_tokens(cli.command.as_ref());
            let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            let file_hint = tokens.iter().find_map(|p| {
                let candidate = if p.is_absolute() {
                    p.clone()
                } else {
                    root.join(p)
                };
                if candidate.is_file() {
                    let canonical = std::fs::canonicalize(&candidate).ok()?;
                    if canonical.starts_with(&canonical_root) {
                        return Some(canonical);
                    }
                }
                None
            });
            let hint = file_hint
                .or_else(|| {
                    tokens.iter().find_map(|p| {
                        let candidate = if p.is_absolute() {
                            p.clone()
                        } else {
                            root.join(p)
                        };
                        if candidate.is_dir() && candidate != *root {
                            let canonical = std::fs::canonicalize(&candidate).ok()?;
                            if canonical.starts_with(&canonical_root) && canonical != canonical_root
                            {
                                return Some(canonical);
                            }
                        }
                        None
                    })
                })
                .or_else(|| {
                    env::current_dir()
                        .ok()
                        .and_then(|cwd| std::fs::canonicalize(&cwd).ok())
                        .filter(|cwd| cwd.starts_with(&canonical_root))
                })
                .unwrap_or_else(|| canonical_root.clone());
            prod_code_mcp::sync::engine_project(root, &hint)
        })
        .unwrap_or((None, None));
    // A nested project of another language is placed under its own key, so its node does not
    // displace the checkout's own placement and back again on the next query (#125).
    let placement_key = match (&cwd_subproject, cwd_engine) {
        (Some(_), Some(engine)) => format!("{cwd_workspace}#{engine}"),
        _ => cwd_workspace.clone(),
    };
    // An editor's server names its language, which may not be the root's: a Swift package's
    // server in a Rust checkout goes to a macOS node, under a key of its own (#332).
    let lsp_engine = match &cli.command {
        Some(Commands::Lsp {
            language: Some(language),
            ..
        }) => Some(
            prod_code_client::editor_files::engine_for_language(language).with_context(|| {
                format!(
                    "prod-code lsp has no server for `{language}`: use rust, go, c, cpp, \
                     python, typescript, javascript or swift"
                )
            })?,
        ),
        _ => None,
    };
    let placement_key = match lsp_engine {
        Some(engine) if Some(engine) != cwd_engine => format!("{cwd_workspace}#{engine}"),
        _ => placement_key,
    };
    let cwd_engine = lsp_engine.or(cwd_engine);
    startup.mark("engine_project");

    if let Some(Commands::Cluster { json, rebalance }) = cli.command {
        startup.report();
        let cluster_root = cwd_root.as_deref().map(|root| {
            cwd_subproject
                .as_deref()
                .map_or_else(|| root.to_path_buf(), |subproject| root.join(subproject))
        });
        return run_cluster(
            &remotes,
            &placement_key,
            cwd_engine,
            cluster_root.as_deref(),
            json,
            rebalance,
        )
        .await;
    }
    if let Some(Commands::Resolve { domain, json }) = cli.command {
        startup.report();
        return run_resolve(&domain, json).await;
    }

    // A Go module whose cgo includes macOS headers builds only on macOS; a Linux node would
    // report the headers as missing on every check (#248).
    let macos_cgo = match (cwd_root.as_deref(), cwd_engine) {
        (Some(root), Some("go")) => prod_code_mcp::sync::macos_only_cgo(
            &cwd_subproject
                .as_deref()
                .map_or_else(|| root.to_path_buf(), |sub| root.join(sub)),
        ),
        _ => None,
    };
    startup.mark("macos_only_cgo");
    let picked = prod_code_mcp::cluster::pick_node(
        &remotes,
        &placement_key,
        cwd_engine,
        macos_cgo.as_ref().map(|_| "macos"),
    )
    .await
    .map_err(|err| match &macos_cgo {
        Some((file, named)) => err.context(format!(
            "this Go module uses macOS-only cgo ({file}: {named})"
        )),
        None => err,
    });
    // A bug report needs no workspace: the node only fills in one line of it, and a checkout
    // the cluster cannot place is exactly what may need reporting (#307).
    if let Some(Commands::ReportIssue {
        title,
        body,
        body_file,
        private_ref,
        force,
        dry_run,
        labels,
    }) = cli.command
    {
        startup.report();
        let unplaced = picked.as_ref().err().map(|err| format!("{err:#}"));
        return run_report_issue(
            picked.ok(),
            unplaced,
            ReportArgs {
                title,
                body,
                body_file,
                private_ref,
                force,
                dry_run,
                labels,
            },
        )
        .await;
    }
    // Update needs no workspace or cluster connection: it interacts with GitHub releases.
    if let Some(Commands::Update { check, force, tag }) = cli.command {
        startup.report();
        return update::run_update(check, force, tag).await;
    }
    // Package management needs no specific project workspace.
    if let Some(Commands::Package { subcommand }) = cli.command {
        startup.report();
        match subcommand {
            package::PackageSubcommands::Status { json } => {
                return package::run_package_status(json).await;
            }
            package::PackageSubcommands::Verify => return package::run_package_verify().await,
            package::PackageSubcommands::Install { force, tag, system } => {
                return package::run_package_install(force, tag, system).await;
            }
            package::PackageSubcommands::Sync { node } => {
                let r = node.or_else(|| picked.ok());
                return package::run_package_sync(r).await;
            }
        }
    }
    // PKI certificate management runs locally without a cluster connection.
    if let Some(Commands::Cert { cmd }) = cli.command {
        startup.report();
        return cert::run_cert(cmd).await;
    }
    if let Some(Commands::Status { json }) = cli.command {
        startup.report();
        let target = if pinned && seeds.len() == 1 {
            seeds[0]
        } else {
            picked.unwrap_or(seeds[0])
        };
        let note = if !pinned && target.ip().is_loopback() && target.port() != 9400 {
            Some("loopback forward".to_string())
        } else {
            None
        };
        return run_status_probe(target, json, note).await;
    }
    // An editor learns why its server could not start from the answer to its `initialize`,
    // not from a process that is gone before it asks (#338).
    if let (Some(Commands::Lsp { .. }), Err(err)) = (&cli.command, &picked) {
        return refuse_lsp(err).await;
    }
    let remote = picked?;
    prod_code_mcp::cluster::set_routing(remotes.clone(), cwd_workspace.clone());
    startup.mark("pick_node");
    startup.report();

    let cmd = cli.command.unwrap_or(Commands::Lsp {
        language: None,
        reconnect: false,
        watchdog_secs: 30,
    });

    let cx = DispatchContext {
        remote,
        remotes: &remotes,
        placement_key: &placement_key,
        cwd_engine,
        lsp_engine,
        cwd_root: cwd_root.as_deref(),
    };

    dispatch(cmd, cx).await
}
