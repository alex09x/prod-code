/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::workspace::find_workspace_root;
use anyhow::Result;
use std::env;
use std::net::SocketAddr;
use std::path::Path;

pub async fn run_impact(
    remote: SocketAddr,
    base: Option<&str>,
    depth: usize,
    run: bool,
    ci: bool,
    json: bool,
) -> Result<()> {
    use std::io::Write;
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let started = std::time::Instant::now();
    let report = prod_code_mcp::impact::analyze(remote, &root, base, depth).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
        eprintln!(
            "[prod-code impact] analysed in {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }
    let (command, why) = if ci {
        let decision = report.ci_decision();
        let command = match decision.run {
            prod_code_mcp::impact::CiRun::WholeSuite => Some(
                prod_code_mcp::verify::plan_command_with(
                    &prod_code_mcp::verify::detect_tools(&root),
                    &report.language,
                    prod_code_mcp::verify::VerifyKind::Test,
                    None,
                )
                .map_err(|e| {
                    anyhow::anyhow!(
                        "impact --ci has to run {}, but there is no test command for {}: {e:#}",
                        decision.why,
                        report.language
                    )
                })?,
            ),
            prod_code_mcp::impact::CiRun::Selected(selected) => Some(selected),
            prod_code_mcp::impact::CiRun::Nothing => None,
        };
        (command, decision.why)
    } else {
        (report.test_command.clone(), String::new())
    };
    if ci {
        println!("[prod-code impact --ci] {why}");
        if let Ok(path) = env::var("GITHUB_STEP_SUMMARY") {
            use std::io::Write as _;
            let summary = report.ci_summary(command.as_deref(), &why);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = f.write_all(summary.as_bytes());
            }
        }
        if command.is_none() {
            return Ok(());
        }
    }
    if run || ci {
        let Some(command) = command else {
            println!("nothing to run");
            return Ok(());
        };
        println!("$ {}", command.join(" "));
        let outcome = prod_code_mcp::exec::run_remote(
            remote,
            &root,
            None,
            command,
            vec![("CARGO_TERM_COLOR".to_string(), "never".to_string())],
            0,
            false,
            |is_stderr, data| {
                if is_stderr {
                    let _ = std::io::stderr().write_all(data);
                } else {
                    let _ = std::io::stdout().write_all(data);
                }
            },
        )
        .await?;
        std::process::exit(outcome.exit.exit_code.unwrap_or(1));
    }
    Ok(())
}

pub async fn run_diagnostics(
    remote: SocketAddr,
    file: &Path,
    proposed: Option<String>,
    json: bool,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let started = std::time::Instant::now();
    let report = match proposed {
        Some(text) => {
            prod_code_mcp::diagnostics::validate_text(remote, &root, &abs_path, &text).await?
        }
        None => prod_code_mcp::diagnostics::diagnostics(remote, &root, &abs_path).await?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
        eprintln!(
            "[prod-code] analysed in {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }
    if !report.ok() {
        std::process::exit(1);
    }
    Ok(())
}

pub async fn run_dead_code(
    remote: SocketAddr,
    include_exported: bool,
    reachability: bool,
    max_files: usize,
    json: bool,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let started = std::time::Instant::now();
    let report = prod_code_mcp::dead_code::find_dead_code_opts(
        remote,
        &root,
        prod_code_mcp::dead_code::DeadCodeOptions {
            include_exported,
            max_files,
            reachability,
        },
    )
    .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
        eprintln!(
            "[prod-code dead-code] scanned in {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

pub async fn run_prune(
    remote: SocketAddr,
    max_files: usize,
    reachability: bool,
    apply: bool,
    force: bool,
    patch: bool,
    commit: bool,
    json: bool,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let started = std::time::Instant::now();
    let pruned = prod_code_mcp::prune::prune_orphans_opts(
        remote,
        &root,
        prod_code_mcp::dead_code::DeadCodeOptions {
            include_exported: false,
            max_files,
            reachability,
        },
        apply || commit,
        force,
        patch,
        commit,
    )
    .await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&pruned)?);
    } else if patch && !apply && !commit {
        if let Some(p) = &pruned.git_patch {
            print!("{p}");
        } else {
            println!("nothing to patch; 0 orphans found");
        }
    } else {
        print!("{}", pruned.render());
        eprintln!(
            "[prod-code prune] completed in {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }
    if !pruned.diagnostics.is_empty() && !force {
        std::process::exit(1);
    }
    Ok(())
}

/// Runs the tests and prints a dossier for every failure.
pub async fn run_diagnose(
    remote: SocketAddr,
    filter: Option<&str>,
    timeout_secs: u64,
    json: bool,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let hint = if cwd != root {
        Some(cwd.as_path())
    } else {
        None
    };
    let report =
        prod_code_mcp::dossier::diagnose(remote, &root, hint, filter, timeout_secs).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
    }
    if report.tests_failed > 0 || !report.build_errors.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}
