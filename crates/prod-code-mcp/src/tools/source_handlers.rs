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
use std::net::SocketAddr;
use std::path::Path;

use super::resolve_file_path;
use crate::protocol::McpToolCallResult;

pub(crate) async fn handle_source(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args.get("line").and_then(|v| v.as_u64()).map(|l| l as u32);
    let context = args.get("context").and_then(|v| v.as_u64()).unwrap_or(30) as u32;
    let (bytes, truncated) = crate::remote_fs::read_source(remote, workspace_root, path).await?;
    let text = String::from_utf8_lossy(&bytes);
    let mut out = match line {
        Some(line) => crate::remote_fs::snippet(&text, line, context),
        None => text.into_owned(),
    };
    if truncated {
        out.push_str("\n[truncated at 2 MiB]");
    }
    Ok(McpToolCallResult::text(out))
}

pub(crate) async fn handle_dead_code(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let include_exported = args
        .get("include_exported")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let reachability = args
        .get("reachability")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_files = args
        .get("max_files")
        .and_then(|v| v.as_u64())
        .unwrap_or(400) as usize;
    let report = crate::dead_code::find_dead_code_opts(
        remote,
        workspace_root,
        crate::dead_code::DeadCodeOptions {
            include_exported,
            max_files,
            reachability,
        },
    )
    .await?;
    Ok(McpToolCallResult::text(report.render()))
}

pub(crate) async fn handle_generate_fixture(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let symbol = args
        .get("symbol")
        .and_then(|v| v.as_str())
        .context("Missing 'symbol' argument")?;
    let builder = args
        .get("builder")
        .map(|value| value.as_bool().context("'builder' must be a boolean"))
        .transpose()?
        .unwrap_or(false);
    let builder_name = args
        .get("builder_name")
        .map(|value| value.as_str().context("'builder_name' must be a string"))
        .transpose()?;
    anyhow::ensure!(
        builder || builder_name.is_none(),
        "'builder_name' requires builder=true"
    );
    anyhow::ensure!(
        !builder || args.get("depth").is_none(),
        "'depth' applies to value fixtures only; omit it for builder=true"
    );
    let verify = args.get("verify").and_then(|v| v.as_bool()).unwrap_or(true);
    if builder {
        anyhow::ensure!(
            args.get("verify").is_none_or(|v| v.is_boolean()),
            "'verify' must be a boolean"
        );
    }
    let hint = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    if builder {
        let preview = crate::fixture::builder::preview(
            remote,
            workspace_root,
            &crate::fixture::builder::BuilderRequest {
                symbol,
                hint: hint.as_deref(),
                builder_name,
                verify,
            },
        )
        .await?;
        return Ok(if !verify || preview.verified() {
            McpToolCallResult::text(preview.render())
        } else {
            McpToolCallResult::error(preview.render())
        });
    }
    let randomized = args
        .get("randomized")
        .map(|value| value.as_bool().context("'randomized' must be a boolean"))
        .transpose()?
        .unwrap_or(false);
    let mock = args
        .get("mock")
        .map(|value| value.as_bool().context("'mock' must be a boolean"))
        .transpose()?
        .unwrap_or(false);
    anyhow::ensure!(!builder || !mock, "'mock' conflicts with builder=true");
    let language = args.get("language").and_then(|v| v.as_str()).and_then(|s| {
        match s.to_ascii_lowercase().as_str() {
            "rust" | "rs" => Some(crate::parameter_object::Language::Rust),
            "go" | "golang" => Some(crate::parameter_object::Language::Go),
            "typescript" | "ts" => Some(crate::parameter_object::Language::TypeScript),
            "javascript" | "js" => Some(crate::parameter_object::Language::JavaScript),
            "python" | "py" => Some(crate::parameter_object::Language::Python),
            "c" => Some(crate::parameter_object::Language::C),
            "cpp" | "c++" => Some(crate::parameter_object::Language::Cpp),
            "swift" => Some(crate::parameter_object::Language::Swift),
            _ => None,
        }
    });

    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(crate::fixture::DEFAULT_DEPTH as u64) as u32;
    let fixture = crate::fixture::generate_with_options(
        remote,
        workspace_root,
        symbol,
        crate::fixture::FixtureOptions {
            depth,
            verify,
            hint,
            randomized,
            mock,
            language,
        },
    )
    .await?;
    let clean = fixture.diagnostics.is_empty();
    let text = fixture.render();
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}
