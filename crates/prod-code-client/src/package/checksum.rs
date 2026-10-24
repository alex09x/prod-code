/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use crate::update::{REPOSITORY, fetch_release_info};

/// Computes the SHA-256 hash of a file using shasum or sha256sum.
pub fn compute_sha256(path: &Path) -> Result<String> {
    let mut cmd = Command::new("shasum");
    cmd.args(["-a", "256", path.to_str().unwrap()]);
    let output = match cmd.output() {
        Ok(out) if out.status.success() => out,
        _ => {
            let mut fallback = Command::new("sha256sum");
            fallback.arg(path.to_str().unwrap());
            fallback
                .output()
                .with_context(|| format!("failed to calculate sha256 for {}", path.display()))?
        }
    };

    if !output.status.success() {
        bail!("sha256 command failed on {}", path.display());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let hash = text
        .split_whitespace()
        .next()
        .context("empty checksum output")?
        .trim()
        .to_string();
    Ok(hash)
}

/// Parses a SHA256SUMS text file into a map of filename -> sha256.
pub fn parse_checksums_file(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let hash = parts[0].trim().to_lowercase();
            let name = parts[1].trim_start_matches('*').trim().to_string();
            map.insert(name, hash);
        }
    }
    map
}

/// Tag to fetch checksums for when the caller gave none: the release's `tag_name`.
fn release_tag(release: &serde_json::Value) -> String {
    release
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("v0.3.19")
        .to_string()
}

/// Where the SHA256SUMS manifest of release `tag` is published.
fn checksums_url(tag: &str) -> String {
    format!("https://github.com/{REPOSITORY}/releases/download/{tag}/SHA256SUMS")
}

/// Fetches the official SHA256SUMS file from GitHub Releases.
pub async fn fetch_official_checksums(tag: Option<&str>) -> Result<HashMap<String, String>> {
    let tag_str = match tag {
        Some(t) => t.to_string(),
        None => release_tag(&fetch_release_info(None).await?),
    };
    fetch_checksums_from_url(&checksums_url(&tag_str), &tag_str)
}

/// Downloads a SHA256SUMS file from `url` with curl and parses it.
fn fetch_checksums_from_url(url: &str, tag_str: &str) -> Result<HashMap<String, String>> {
    let output = Command::new("curl")
        .args(["-fsSL", "-H", "User-Agent: prod-code-package-manager", url])
        .output()
        .with_context(|| format!("failed to download SHA256SUMS from {url}"))?;

    if !output.status.success() {
        bail!("failed to fetch official checksums for {tag_str}");
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_checksums_file(&text))
}

#[cfg(test)]
mod tests;
