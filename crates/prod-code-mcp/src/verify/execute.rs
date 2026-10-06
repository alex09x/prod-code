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

use anyhow::{Result, anyhow};

use crate::exec::{TailBuffer, run_remote};
use crate::verify::detect::detect_tools;
use crate::verify::parse::{event_of_line, parse_verification_output, relativize_diagnostics};
use crate::verify::plan::{has_xcode_project, narrow_scope, plan_command_with, plan_xcode_command};
use crate::verify::types::{RunEvent, VerifyKind, VerifyReport};

pub async fn run_verify(
    remote: SocketAddr,
    root: &Path,
    project_hint: Option<&Path>,
    kind: VerifyKind,
    filter: Option<&str>,
    timeout_secs: u64,
) -> Result<VerifyReport> {
    run_verify_with(
        remote,
        root,
        project_hint,
        kind,
        filter,
        timeout_secs,
        &[],
        |_| {},
    )
    .await
}

/// [`run_verify`] with extra environment for the command (`RUST_BACKTRACE=1`) and every
/// [`RunEvent`] handed to `on_event` as its line arrives.
#[allow(clippy::too_many_arguments)]
pub async fn run_verify_with(
    remote: SocketAddr,
    root: &Path,
    project_hint: Option<&Path>,
    kind: VerifyKind,
    filter: Option<&str>,
    timeout_secs: u64,
    extra_env: &[(String, String)],
    mut on_event: impl FnMut(RunEvent),
) -> Result<VerifyReport> {
    // A nested project of another language (a SwiftPM package in a Rust repository) is
    // verified in its own directory with its own tooling.
    let (subdir, language) = crate::sync::engine_project(root, project_hint.unwrap_or(root));
    let language = language.ok_or_else(|| {
        anyhow!(
            "no project manifest (Cargo.toml, go.mod, package.json, pyproject.toml, CMakeLists.txt, Package.swift) at {}",
            root.display()
        )
    })?;
    let project_dir = match &subdir {
        Some(sub) => root.join(sub),
        None => root.to_path_buf(),
    };
    let tools = detect_tools(&project_dir);
    let mut command = if language == "swift" && has_xcode_project(&project_dir) {
        plan_xcode_command(kind, filter)?
    } else {
        plan_command_with(&tools, language, kind, filter)?
    };
    if let Some(hint) = project_hint {
        narrow_scope(&mut command, language, &project_dir, hint);
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut tail = TailBuffer::new(8 * 1024);
    let mut env = vec![
        ("CARGO_TERM_COLOR".to_string(), "never".to_string()),
        ("NO_COLOR".to_string(), "1".to_string()),
    ];
    env.extend(extra_env.iter().cloned());
    // Complete stdout lines become events as they arrive; a line split across chunks waits.
    let mut pending: Vec<u8> = Vec::new();
    let outcome = run_remote(
        remote,
        root,
        subdir.as_deref(),
        command.clone(),
        env,
        timeout_secs,
        false,
        |is_stderr, data| {
            if !is_stderr {
                pending.extend_from_slice(data);
                while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=end).collect();
                    if let Some(event) =
                        event_of_line(language, kind, String::from_utf8_lossy(&line).trim_end())
                    {
                        on_event(event);
                    }
                }
            }
            tail.push(data);
            if is_stderr {
                stderr.extend_from_slice(data);
            } else {
                stdout.extend_from_slice(data);
            }
        },
    )
    .await?;
    if let Some(err) = &outcome.exit.error {
        return Err(anyhow!("remote {} failed to start: {err}", kind.label()));
    }
    let stdout = String::from_utf8_lossy(&stdout);
    let stderr = String::from_utf8_lossy(&stderr);

    let (mut diagnostics, fixes, benches, tests_passed, tests_failed, mut failures) =
        parse_verification_output(language, kind, &stdout, &stderr, &tools);
    if let Some(sub) = &subdir {
        let nested = format!(
            "{}/{sub}",
            outcome.exit.server_workspace_root.trim_end_matches('/')
        );
        relativize_diagnostics(&mut diagnostics, &nested);
    }
    relativize_diagnostics(&mut diagnostics, &outcome.exit.server_workspace_root);
    if let Some(subdir) = &subdir {
        let subdir = Path::new(subdir);
        for diagnostic in &mut diagnostics {
            if let Some(file) = diagnostic.file.as_mut() {
                let path = Path::new(file);
                if !path.is_absolute() && !path.starts_with(subdir) {
                    *file = subdir.join(path).to_string_lossy().replace('\\', "/");
                }
            }
        }
    }
    for failure in &mut failures {
        let prefix = format!(
            "{}/",
            outcome.exit.server_workspace_root.trim_end_matches('/')
        );
        if prefix.len() > 1 {
            failure.output = failure.output.replace(&prefix, "");
        }
    }

    Ok(VerifyReport {
        kind,
        language: language.to_string(),
        command,
        exit_code: outcome.exit.exit_code,
        timed_out: outcome.exit.timed_out,
        duration_ms: outcome.exit.duration_ms,
        diagnostics,
        tests_passed,
        tests_failed,
        failures,
        tail: tail.text(),
        fixes,
        benches,
        usage: outcome.exit.usage,
        platform: outcome.exit.platform.clone(),
    })
}
