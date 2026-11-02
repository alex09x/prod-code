/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, ensure};
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
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(3) as usize;
    ensure!((1..=8).contains(&limit), "grep limit must be 1..=8");
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
            items.push(json!({"path":hit.path,"symbol":symbol,"match_line":hit.line,"match_character":hit.column}));
        } else {
            unexpanded.push(json!({"path":hit.path,"line":hit.line,"text":hit.text}));
        }
    }
    Ok(
        json!({"items":items,"unexpanded":unexpanded,"truncated":truncated || !unexpanded.is_empty(),"engine":report.engine,"enclosing_symbols":"heuristic hints; definitions are resolved separately"}),
    )
}
