/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::path::PathBuf;

/// A location from an LSP answer: a `Location` or a `LocationLink`, as (path, 0-based line,
/// 0-based character).
pub(crate) fn location_of(value: &serde_json::Value) -> Option<(PathBuf, u32, u32)> {
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))?
        .as_str()?;
    let range = value
        .get("range")
        .or_else(|| value.get("targetSelectionRange"))?;
    let at = |k: &str| {
        range
            .pointer(&format!("/start/{k}"))
            .and_then(|v| v.as_u64())
    };
    Some((
        PathBuf::from(crate::remote_fs::uri_to_path(uri)),
        at("line")? as u32,
        at("character")? as u32,
    ))
}

pub(crate) fn locations(value: &serde_json::Value) -> Vec<(PathBuf, u32, u32)> {
    match value {
        serde_json::Value::Array(items) => items.iter().filter_map(location_of).collect(),
        other => location_of(other).into_iter().collect(),
    }
}

pub(crate) fn hierarchy_range(item: &serde_json::Value) -> Option<&serde_json::Value> {
    match item.get("selectionRange") {
        Some(range) => Some(range),
        None => item.get("range"),
    }
}

pub(crate) fn hierarchy_start(item: &serde_json::Value) -> Option<&serde_json::Value> {
    hierarchy_range(item).and_then(|range| range.get("start"))
}

pub(crate) fn lsp_position(position: Option<&serde_json::Value>) -> Option<(u64, u64)> {
    let line = position?.get("line")?.as_u64()?;
    let character = position?.get("character")?.as_u64()?;
    (line < u64::from(u32::MAX) && character < u64::from(u32::MAX)).then_some((line, character))
}

pub(crate) fn valid_lsp_range(range: &serde_json::Value) -> bool {
    matches!(
        (
            lsp_position(range.get("start")),
            lsp_position(range.get("end"))
        ),
        (Some(start), Some(end)) if start <= end
    )
}

pub(crate) fn validate_type_hierarchy_item(item: &serde_json::Value) -> Result<()> {
    anyhow::ensure!(
        item.is_object(),
        "type hierarchy item is not an object: {item}"
    );
    anyhow::ensure!(
        item.get("name").and_then(|n| n.as_str()).is_some(),
        "type hierarchy item has no valid 'name': {item}"
    );
    anyhow::ensure!(
        item.get("uri").and_then(|u| u.as_str()).is_some(),
        "type hierarchy item has no valid 'uri': {item}"
    );
    let has_valid_range = hierarchy_range(item).is_some_and(valid_lsp_range);
    anyhow::ensure!(
        has_valid_range,
        "type hierarchy item has no valid 'selectionRange' or 'range': {item}"
    );
    Ok(())
}
