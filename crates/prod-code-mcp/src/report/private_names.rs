/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};

use super::declarations;

const GUIDANCE: &str = "Describe the problem with a neutral reproduction. File private details in the private incident store and pass its key as private_ref (--private-ref). For another public repository only, explicitly set allow_private_names: true (--allow-private-names).";
const FILE_BYTES: u64 = 256 * 1024;
const SOURCE_BYTES: u64 = 32 * 1024 * 1024;
const LIST_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn check(root: &Path, texts: &[&str]) -> Result<()> {
    // Bound all git calls + scanning by 10s, the tracked-file list by 8 MiB / 50,000 files,
    // each source read by 256 KiB, total source reads by 32 MiB, and input by 64 KiB.
    // Oversized individual sources are skipped; exhausted aggregate budgets fail closed.
    // Keep only names actually present in the report, not a repository-wide symbol index.
    let deadline = Instant::now() + Duration::from_secs(10);
    if let Some(remote) = git(
        root,
        &["config", "--local", "--get", "remote.origin.url"],
        4096,
        deadline,
    )? && public_remote(String::from_utf8_lossy(&remote).trim())
    {
        return Ok(());
    }
    ensure!(
        texts.iter().map(|text| text.len()).sum::<usize>() <= 64 * 1024,
        "Report too large to check safely. {GUIDANCE}"
    );
    let Some(top) = git(root, &["rev-parse", "--show-toplevel"], 8192, deadline)? else {
        // Reporting placement failures from outside a Git checkout remains supported (#307).
        // There are no tracked checkout names to collect in that case.
        return Ok(());
    };
    let top = String::from_utf8(top).context("git root is not UTF-8")?;
    let root = Path::new(top.trim_end_matches(['\n', '\r']));
    let files = git(root, &["ls-files", "-z", "--cached"], LIST_BYTES, deadline)?
        .context("cannot list tracked files for the privacy check")?;
    let mut offending = BTreeSet::new();
    let mut consumed = 0;
    for (index, path) in files
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .enumerate()
    {
        ensure!(
            index < 50_000 && Instant::now() < deadline,
            "Checkout privacy scan exceeded its budget. {GUIDANCE}"
        );
        let path = std::str::from_utf8(path).context("tracked path is not UTF-8")?;
        let relative = Path::new(path);
        let Some(regex) = relative
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(declarations::pattern)
        else {
            continue;
        };
        let absolute = root.join(relative);
        // Every checkout has an error.rs, src/main.rs or a config module: a name built only
        // from generic words identifies nothing, and refusing it would refuse every report.
        let generic_path = relative.iter().all(|part| {
            let part = Path::new(part);
            part.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(declarations::generic)
        });
        if let Some(absolute) = absolute.to_str() {
            record(texts, absolute, false, &mut offending);
        }
        if !generic_path {
            for name in [
                Some(path),
                relative.file_name().and_then(|name| name.to_str()),
                relative
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .filter(|stem| !declarations::generic(stem)),
            ]
            .into_iter()
            .flatten()
            {
                record(texts, name, false, &mut offending);
            }
        }
        let Ok(metadata) = std::fs::symlink_metadata(&absolute) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > FILE_BYTES {
            continue;
        }
        consumed += metadata.len();
        ensure!(
            consumed <= SOURCE_BYTES,
            "Checkout privacy scan exceeded its source budget. {GUIDANCE}"
        );
        let mut source = String::new();
        std::fs::File::open(&absolute)?
            .take(FILE_BYTES + 1)
            .read_to_string(&mut source)?;
        ensure!(
            source.len() as u64 <= FILE_BYTES,
            "Source grew beyond the privacy scan limit. {GUIDANCE}"
        );
        for captures in regex.captures_iter(&source) {
            ensure!(
                Instant::now() < deadline,
                "Checkout privacy scan exceeded its time budget. {GUIDANCE}"
            );
            for name in captures
                .iter()
                .skip(1)
                .flatten()
                .map(|capture| capture.as_str())
            {
                if !declarations::generic(name) {
                    record(texts, name, true, &mut offending);
                }
            }
        }
    }
    ensure!(
        Instant::now() < deadline,
        "Checkout privacy scan exceeded its time budget. {GUIDANCE}"
    );
    ensure!(
        offending.is_empty(),
        "Refusing to publish checkout-specific tokens: {}. {GUIDANCE}",
        offending
            .into_iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

fn record(texts: &[&str], name: &str, identifier: bool, offending: &mut BTreeSet<String>) {
    // Hyphens belong to filenames, but separate identifiers in prose such as Type-based.
    let part = |ch: char| unicode_ident::is_xid_continue(ch) || (!identifier && ch == '-');
    if texts.iter().any(|text| {
        text.match_indices(name).any(|(at, _)| {
            !text[..at].chars().next_back().is_some_and(part)
                && !text[at + name.len()..].chars().next().is_some_and(part)
        })
    }) {
        offending.insert(name.to_string());
    }
}

fn public_remote(remote: &str) -> bool {
    let remote = remote.trim_end_matches('/');
    let remote = remote.strip_suffix(".git").unwrap_or(remote);
    if let Some(path) = remote.strip_prefix("git@github.com:") {
        return path.eq_ignore_ascii_case("alex09x/prod-code");
    }
    url::Url::parse(remote).is_ok_and(|url| {
        matches!(url.scheme(), "https" | "ssh")
            && url.host_str() == Some("github.com")
            && url
                .port()
                .is_none_or(|port| url.scheme() == "ssh" && port == 22)
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path().eq_ignore_ascii_case("/alex09x/prod-code")
    })
}

/// Drain at most `limit + 1` bytes while enforcing the shared deadline; kill and reap git
/// on timeout. Neither a huge index nor a stuck git process can hang or exhaust the client.
fn git(root: &Path, args: &[&str], limit: usize, deadline: Instant) -> Result<Option<Vec<u8>>> {
    let mut child = Command::new("git")
        .current_dir(root)
        .args(["-c", "core.fsmonitor=false"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().context("git stdout missing")?;
    std::thread::scope(|scope| {
        let reader = scope.spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(limit as u64 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    let _ = result.context("waiting for privacy scan git command")?;
                    anyhow::bail!("Checkout privacy scan timed out. {GUIDANCE}");
                }
            }
        };
        let bytes = reader
            .join()
            .map_err(|_| anyhow::anyhow!("privacy scan reader failed"))??;
        ensure!(
            bytes.len() <= limit,
            "Checkout privacy scan git output exceeded its budget. {GUIDANCE}"
        );
        Ok(status.success().then_some(bytes))
    })
}
