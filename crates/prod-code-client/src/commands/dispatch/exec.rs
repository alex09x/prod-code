/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::DispatchContext;
use crate::cli::Commands;
use crate::commands::bench::{run_benchmark, run_divergent_bench};
use crate::commands::common::run_tool;
use crate::commands::exec::{VerifyArgs, run_exec, run_shadow_cli, run_verify};
use crate::commands::query::run_diagnostics;
use crate::commands::refactor::run_fix;
use crate::validation::*;
use anyhow::{Context, Result};
use prod_code_client::divergent_bench::DivergentBenchConfig;
use prod_code_mcp::verify::VerifyKind;

pub async fn dispatch_exec(cmd: Commands, cx: &DispatchContext<'_>) -> Result<()> {
    match cmd {
        Commands::Validate {
            file,
            from,
            diff,
            with,
            compile,
            stream,
            chunk,
            session,
            close,
            reset,
            borrow_check,
            json,
        } => {
            if compile && json && !stream && chunk.is_none() {
                anyhow::bail!("--compile reports as text; leave out --json");
            }
            if borrow_check && json && !stream && chunk.is_none() {
                anyhow::bail!("--borrow-check reports as text; leave out --json");
            }
            if let Some(ch) = chunk {
                let file = file.context("give the file to validate with --chunk")?;
                let session_id = session.unwrap_or_else(|| {
                    let counter = prod_code_mcp::diagnostics::next_batch_counter();
                    format!("cli-{}-{}", std::process::id(), counter)
                });
                return run_validate_chunk(
                    cx.remote,
                    &file,
                    &session_id,
                    &ch,
                    close,
                    reset,
                    borrow_check || compile,
                    json,
                )
                .await;
            }
            if stream {
                let file = file.context("give the file to validate with --stream")?;
                return run_validate_stream(
                    cx.remote,
                    &file,
                    from,
                    session,
                    borrow_check || compile,
                    json,
                )
                .await;
            }
            if let Some(diff) = diff {
                let patch = if diff.as_os_str() == "-" {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                    buf
                } else {
                    std::fs::read_to_string(&diff)
                        .with_context(|| format!("failed to read {}", diff.display()))?
                };
                if compile || borrow_check {
                    if borrow_check {
                        eprintln!(
                            "Running the remote compiler check for the proposed diff (with borrow-checker proof)..."
                        );
                    } else {
                        eprintln!("Running the remote compiler check for the proposed diff...");
                    }
                }
                return run_tool(
                    cx.remote,
                    "code_validate_edits",
                    serde_json::json!({
                        "diff": patch,
                        "compile": compile || borrow_check,
                        "borrow_check": borrow_check,
                    }),
                )
                .await;
            }
            let file = file.context("give the file to validate, or `--diff PATCH`")?;
            let text = match from {
                Some(path) => std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.display()))?,
                None => {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                    buf
                }
            };
            if compile || borrow_check {
                run_validate_compiled(cx.remote, &file, text, &with, borrow_check).await
            } else if with.is_empty() {
                run_diagnostics(cx.remote, &file, Some(text), json).await
            } else {
                run_validate_together(cx.remote, &file, text, &with, json).await
            }
        }
        Commands::Check {
            timeout_secs,
            json,
            fix: true,
            path,
            ..
        } => run_fix(cx.remote, VerifyKind::Check, timeout_secs, json, path).await,
        Commands::Check {
            timeout_secs,
            json,
            env,
            events,
            path,
            ..
        } => {
            run_verify(
                cx.remote,
                cx.remotes,
                Some(cx.placement_key),
                VerifyKind::Check,
                VerifyArgs {
                    filter: None,
                    timeout_secs,
                    json,
                    env,
                    events,
                    path,
                },
            )
            .await
        }
        Commands::Lint {
            timeout_secs,
            json,
            fix: true,
            path,
            ..
        } => run_fix(cx.remote, VerifyKind::Lint, timeout_secs, json, path).await,
        Commands::Lint {
            timeout_secs,
            json,
            env,
            events,
            path,
            ..
        } => {
            run_verify(
                cx.remote,
                cx.remotes,
                Some(cx.placement_key),
                VerifyKind::Lint,
                VerifyArgs {
                    filter: None,
                    timeout_secs,
                    json,
                    env,
                    events,
                    path,
                },
            )
            .await
        }
        Commands::Test {
            filter,
            timeout_secs,
            json,
            env,
            events,
            path,
        } => {
            run_verify(
                cx.remote,
                cx.remotes,
                Some(cx.placement_key),
                VerifyKind::Test,
                VerifyArgs {
                    filter,
                    timeout_secs,
                    json,
                    env,
                    events,
                    path,
                },
            )
            .await
        }
        Commands::Benchmarks {
            filter,
            timeout_secs,
            json,
            env,
            events,
            path,
        } => {
            run_verify(
                cx.remote,
                cx.remotes,
                Some(cx.placement_key),
                VerifyKind::Bench,
                VerifyArgs {
                    filter,
                    timeout_secs,
                    json,
                    env,
                    events,
                    path,
                },
            )
            .await
        }
        Commands::Exec {
            timeout_secs,
            no_pull,
            env,
            command,
        } => run_exec(cx.remote, command, env, timeout_secs, !no_pull).await,
        Commands::ShadowRun {
            spec,
            timeout_secs,
            parallel,
            apply,
            ram,
            command,
        } => run_shadow_cli(cx.remote, spec, timeout_secs, parallel, apply, ram, command).await,
        Commands::Bench {
            workspaces,
            concurrency,
            depth,
            duration_secs,
        } => run_benchmark(cx.remote, workspaces, concurrency, depth, duration_secs).await,
        Commands::DivergentBench {
            base_repo,
            workdir,
            workers,
            queries_per_worker,
            keep_workdir,
            mode,
            persistent,
            churn,
            worktrees,
        } => {
            run_divergent_bench(DivergentBenchConfig {
                remote: cx.remote,
                base_repo,
                workdir,
                workers,
                queries_per_worker,
                keep_workdir,
                mode,
                persistent,
                churn_percent: churn,
                worktrees,
            })
            .await
        }
        _ => unreachable!(),
    }
}
