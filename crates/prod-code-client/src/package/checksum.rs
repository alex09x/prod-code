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

/// Fetches the official SHA256SUMS file from GitHub Releases.
pub async fn fetch_official_checksums(tag: Option<&str>) -> Result<HashMap<String, String>> {
    let tag_str = match tag {
        Some(t) => t.to_string(),
        None => {
            let rel = fetch_release_info(None).await?;
            rel.get("tag_name")
                .and_then(|v| v.as_str())
                .unwrap_or("v0.3.19")
                .to_string()
        }
    };

    let url = format!("https://github.com/{REPOSITORY}/releases/download/{tag_str}/SHA256SUMS");

    let output = Command::new("curl")
        .args(["-fsSL", "-H", "User-Agent: prod-code-package-manager", &url])
        .output()
        .with_context(|| format!("failed to download SHA256SUMS from {url}"))?;

    if !output.status.success() {
        bail!("failed to fetch official checksums for {tag_str}");
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_checksums_file(&text))
}
