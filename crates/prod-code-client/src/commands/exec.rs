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
/// The run belongs to the checkout at `root`, so a path outside it is refused rather than
/// quietly replaced by that checkout's own project (#1478).
pub fn verify_scope(cwd: &Path, root: &Path, path: Option<&Path>) -> Result<PathBuf> {
    let Some(path) = path else {
        return Ok(cwd.to_path_buf());
    };
    let path = cwd.join(path);
    anyhow::ensure!(path.exists(), "--path {} does not exist", path.display());
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    anyhow::ensure!(
        path.starts_with(&root),
        "--path {} is outside this checkout ({}); run the command from the checkout that holds it",
        path.display(),
        root.display()
    );
    Ok(path)
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
pub async fn run_verify(
    remote: SocketAddr,
    remotes: &[SocketAddr],
    placement_key: Option<&str>,
    kind: VerifyKind,
    args: VerifyArgs,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let scope = verify_scope(&cwd, &root, args.path.as_deref())?;
    // Preserve the spelling of a Rust test-file hint after containment validation so the
    // planner can reject symlink aliases instead of silently selecting a different target.
    let scope = if kind == VerifyKind::Test {
        args.path
            .as_deref()
            .map(|path| cwd.join(path))
            .filter(|path| {
                path.is_file() && path.extension().is_some_and(|extension| extension == "rs")
            })
            .unwrap_or(scope)
    } else {
        scope
    };
    let env = args
        .env
        .iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .with_context(|| format!("--env takes KEY=VALUE, got `{pair}`"))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut candidates = vec![remote];
    for &r in remotes {
        if !candidates.contains(&r) {
            candidates.push(r);
        }
    }
    if candidates.len() <= 1 {
        if let Some(known) = prod_code_mcp::cluster::known_cluster_nodes() {
            for r in known {
                if !candidates.contains(&r) {
                    candidates.push(r);
                }
            }
        }
    }

    let mut last_msrv_report = None;
    let mut last_err = None;

    for (idx, &current_remote) in candidates.iter().enumerate() {
        let result = prod_code_mcp::verify::run_verify_with(
            current_remote,
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
        .await;

        match result {
            Ok(report) => {
                if report.is_toolchain_msrv_rejection() && idx + 1 < candidates.len() {
                    tracing::warn!(
                        remote = %current_remote,
                        "remote toolchain rejected by workspace MSRV; trying next candidate"
                    );
                    last_msrv_report = Some((current_remote, report));
                    continue;
                }

                if current_remote != remote {
                    if let Some(key) = placement_key {
                        prod_code_mcp::cluster::remember_placement(key, current_remote);
                    }
                }

                if args.events {
                    println!(
                        "{}",
                        serde_json::json!({ "event": "report", "report": report })
                    );
                } else if args.json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print!("{}", report.render(200));
                    if report.is_toolchain_msrv_rejection() {
                        eprintln!(
                            "\nHint: The workspace requires a newer Rust toolchain than available on {current_remote}. Specify a compatible node with `--remote <addr>` or update the node's toolchain."
                        );
                    }
                }
                std::process::exit(if report.ok() { 0 } else { 1 });
            }
            Err(e) => {
                if prod_code_mcp::server::failover::is_retryable_connection_error("verify", &e)
                    && idx + 1 < candidates.len()
                {
                    tracing::warn!(
                        remote = %current_remote,
                        error = %e,
                        "verify connection or pre-flight failed on candidate; trying next candidate"
                    );
                    last_err = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }

    if let Some((failed_remote, report)) = last_msrv_report {
        if args.events {
            println!(
                "{}",
                serde_json::json!({ "event": "report", "report": report })
            );
        } else if args.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", report.render(200));
            eprintln!(
                "\nHint: The workspace requires a newer Rust toolchain than available on cluster nodes (tested {failed_remote}). Specify a compatible node with `--remote <addr>` or update the node's toolchain."
            );
        }
        std::process::exit(1);
    }

    if let Some(err) = last_err {
        return Err(err);
    }
    Ok(())
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
    let warning = outcome.platform_warning(&root);
    let exit = outcome.exit;
    if let Some(err) = &exit.error {
        anyhow::bail!("remote exec failed: {err}");
    }
    prod_code_mcp::exec::remember_exec_node(&root, remote);
    if !outcome.pulled_files.is_empty() {
        eprintln!(
            "[prod-code exec] {} file(s) changed by the command written back: {}",
            outcome.pulled_files.len(),
            outcome.pulled_files.join(", ")
        );
    }
    if !outcome.kept_files.is_empty() {
        eprintln!(
            "[prod-code exec] {} file(s) with changed or unverified local state were kept as they are; the node's version was not written: {}",
            outcome.kept_files.len(),
            outcome.kept_files.join(", ")
        );
    }
    if let Some(warning) = warning {
        eprintln!("[prod-code exec] {warning}");
    }
    eprintln!(
        "[prod-code exec] {} in {:.1}s (server {:.1}s{}) on node {remote}, copy {}{}",
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

#[cfg(test)]
mod scope_tests {
    use super::verify_scope;

    /// A `--path` inside this checkout scopes the run; one in another project is refused,
    /// instead of this checkout's own project being checked under its name (#1478).
    #[test]
    fn a_path_outside_the_checkout_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let this = dir.path().join("this");
        let other = dir.path().join("other");
        std::fs::create_dir_all(this.join("crates/inner")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let inner = verify_scope(&this, &this, Some(std::path::Path::new("crates/inner"))).unwrap();
        assert!(inner.ends_with("crates/inner"), "{inner:?}");
        assert!(verify_scope(&this.join("crates"), &this, None).is_ok());
        let error = verify_scope(&this, &this, Some(std::path::Path::new("../other")))
            .expect_err("another project");
        assert!(
            error.to_string().contains("outside this checkout"),
            "{error}"
        );
    }
}
