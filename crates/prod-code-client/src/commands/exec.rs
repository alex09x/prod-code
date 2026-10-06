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
use anyhow::{Context, Result};
use prod_code_mcp::verify::VerifyKind;
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Where a check, lint, test or benchmark run is scoped: `--path`, relative to `cwd`, or `cwd`.
pub fn verify_scope(cwd: &Path, path: Option<&Path>) -> Result<PathBuf> {
    let Some(path) = path else {
        return Ok(cwd.to_path_buf());
    };
    let path = cwd.join(path);
    anyhow::ensure!(path.exists(), "--path {} does not exist", path.display());
    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}

/// The arguments of `check`, `lint`, `test` and `benchmarks`, as given.
pub struct VerifyArgs {
    pub filter: Option<String>,
    pub timeout_secs: u64,
    pub json: bool,
    pub env: Vec<String>,
    pub events: bool,
    pub path: Option<PathBuf>,
}

/// Typed remote verification: check / lint / test with parsed diagnostics.
pub async fn run_verify(remote: SocketAddr, kind: VerifyKind, args: VerifyArgs) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let scope = verify_scope(&cwd, args.path.as_deref())?;
    let env = args
        .env
        .iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .with_context(|| format!("--env takes KEY=VALUE, got `{pair}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    let report = prod_code_mcp::verify::run_verify_with(
        remote,
        &root,
        Some(&scope),
        kind,
        args.filter.as_deref(),
        args.timeout_secs,
        &env,
        |event| {
            if args.events
                && let Ok(line) = serde_json::to_string(&event)
            {
                println!("{line}");
            }
        },
    )
    .await?;
    if args.events {
        println!(
            "{}",
            serde_json::json!({ "event": "report", "report": report })
        );
    } else if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render(200));
    }
    std::process::exit(if report.ok() { 0 } else { 1 });
}

pub async fn run_shadow_cli(
    remote: SocketAddr,
    spec: PathBuf,
    timeout_secs: u64,
    parallel: usize,
    apply: bool,
    ram: bool,
    command: Vec<String>,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let subdir = prod_code_mcp::exec::subdir_of(&root, &cwd);
    let text = std::fs::read_to_string(&spec)
        .with_context(|| format!("cannot read {}", spec.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("{} is not JSON", spec.display()))?;
    let ram = ram
        || json
            .get("in_memory")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || json.get("ram").and_then(|v| v.as_bool()).unwrap_or(false);
    let specs = prod_code_mcp::shadow::parse_specs(&root, &json, spec.parent())?;
    let outcome = prod_code_mcp::shadow::run_shadow(
        remote,
        &root,
        subdir.as_deref(),
        &specs,
        command.clone(),
        vec![("CARGO_TERM_COLOR".to_string(), "never".to_string())],
        timeout_secs,
        parallel,
        64 * 1024,
        ram,
    )
    .await?;
    let applied = match (apply, outcome.winner) {
        (true, Some(i)) => Some(prod_code_mcp::shadow::apply_hypothesis(&root, &specs[i])?),
        _ => None,
    };
    println!(
        "{}",
        prod_code_mcp::shadow::render_report(&outcome, &command, applied.as_deref(), 4000)
    );
    if outcome.winner.is_none() {
        std::process::exit(1);
    }
    Ok(())
}

pub async fn run_exec(
    remote: SocketAddr,
    command: Vec<String>,
    env_args: Vec<String>,
    timeout_secs: u64,
    pull_changes: bool,
) -> Result<()> {
    use std::io::Write;
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let subdir = prod_code_mcp::exec::subdir_of(&root, &cwd);
    let mut env_pairs = Vec::new();
    if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        env_pairs.push(("CARGO_TERM_COLOR".to_string(), "always".to_string()));
    }
    for pair in &env_args {
        let (key, value) = pair
            .split_once('=')
            .with_context(|| format!("--env takes KEY=VALUE, got `{pair}`"))?;
        env_pairs.push((key.to_string(), value.to_string()));
    }
    let started = std::time::Instant::now();
    let outcome = prod_code_mcp::exec::run_remote(
        remote,
        &root,
        subdir.as_deref(),
        command.clone(),
        env_pairs,
        timeout_secs,
        pull_changes,
        |is_stderr, data| {
            if is_stderr {
                let mut e = std::io::stderr().lock();
                let _ = e.write_all(data);
                let _ = e.flush();
            } else {
                let mut o = std::io::stdout().lock();
                let _ = o.write_all(data);
                let _ = o.flush();
            }
        },
    )
    .await?;
    let changed_code = outcome.changed_code();
    let exit = outcome.exit;
    if let Some(err) = &exit.error {
        anyhow::bail!("remote exec failed: {err}");
    }
    if !outcome.pulled_files.is_empty() {
        eprintln!(
            "[prod-code exec] {} file(s) changed by the command written back: {}",
            outcome.pulled_files.len(),
            outcome.pulled_files.join(", ")
        );
    }
    if !outcome.kept_files.is_empty() {
        eprintln!(
            "[prod-code exec] {} file(s) changed here while the command ran were kept, and the node's version was not written: {}",
            outcome.kept_files.len(),
            outcome.kept_files.join(", ")
        );
    }
    if let Some(warning) =
        prod_code_mcp::exec::platform_warning(&root, exit.platform.as_deref(), &changed_code)
    {
        eprintln!("[prod-code exec] {warning}");
    }
    eprintln!(
        "[prod-code exec] {} in {:.1}s (server {:.1}s{}) on {}{}",
        match (exit.timed_out, exit.exit_code) {
            (true, _) => "timed out".to_string(),
            (false, Some(code)) => format!("exit {code}"),
            (false, None) => "killed".to_string(),
        },
        started.elapsed().as_secs_f64(),
        exit.duration_ms as f64 / 1000.0,
        exit.usage
            .map(|u| format!(", {}", u.render()))
            .unwrap_or_default(),
        exit.server_workspace_root,
        exit.platform
            .as_deref()
            .map(|p| format!(" ({p})"))
            .unwrap_or_default()
    );
    std::process::exit(exit.exit_code.unwrap_or(1));
}
