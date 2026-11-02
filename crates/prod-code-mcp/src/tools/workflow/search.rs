/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    net::SocketAddr,
    path::{Path, PathBuf},
};

pub(super) async fn grep(remote: SocketAddr, root: &Path, args: Value) -> Result<Value> {
    let pattern = args["pattern"]
        .as_str()
        .context("grep requires pattern")?
        .to_owned();
    let limit = super::plan::limit(&args, 3)?;
    let options = crate::grep::GrepOptions {
        pattern,
        path: args.get("path").and_then(Value::as_str).map(PathBuf::from),
        fixed_strings: args
            .get("fixed_strings")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        max_results: 64,
        ..Default::default()
    };
    let report = crate::grep::run_grep(root, Some(remote), &options).await?;
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut unexpanded = Vec::new();
    let mut unexpanded_total = 0;
    let mut unexpanded_bytes = 0;
    let mut truncated = report.truncated;
    for hit in report.hits {
        if let Some(symbol) = hit.enclosing_symbol {
            if !seen.insert((hit.path.clone(), symbol.clone())) {
                continue;
            }
            if items.len() >= limit {
                truncated = true;
                continue;
            }
            items.push(json!({"path":hit.path,"symbol":symbol,"match_line":hit.line,"match_column_bytes":hit.column}));
        } else {
            unexpanded_total += 1;
            let mut end = hit.text.len().min(256);
            while !hit.text.is_char_boundary(end) {
                end -= 1;
            }
            let shortened = end < hit.text.len();
            let row = json!({"path":hit.path,"line":hit.line,"column_bytes":hit.column,"text":&hit.text[..end],"text_truncated":shortened});
            let bytes = serde_json::to_vec(&row)?.len();
            if unexpanded.len() < 8 && unexpanded_bytes + bytes <= 8192 {
                unexpanded.push(row);
                unexpanded_bytes += bytes;
            }
            truncated |= shortened;
        }
    }
    let unexpanded_truncated = unexpanded_total > unexpanded.len();
    Ok(
        json!({"items":items,"unexpanded":unexpanded,"unexpanded_total":unexpanded_total,"unexpanded_truncated":unexpanded_truncated,"truncated":truncated || unexpanded_truncated,"incomplete":unexpanded_total > 0,"engine":report.engine,"enclosing_symbols":"heuristic hints; definitions are resolved separately"}),
    )
}
