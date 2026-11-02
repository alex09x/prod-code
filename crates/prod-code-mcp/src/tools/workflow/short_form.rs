/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspect {
    symbol: String,
    apply: Option<bool>,
    structured: Option<bool>,
    path: Option<String>,
    workspace: Option<String>,
    #[serde(default = "default_include")]
    include: Vec<String>,
    depth: Option<u32>,
    max_bytes: Option<usize>,
    timeout_ms: Option<u64>,
}

fn default_include() -> Vec<String> {
    vec!["dependencies".into()]
}

pub(super) const PLAN_FIELDS: &[&str] = &[
    "apply",
    "steps",
    "workspace",
    "outputs",
    "max_bytes",
    "timeout_ms",
    "structured",
];
pub(super) const SYMBOL_FIELDS: &[&str] = &[
    "apply",
    "symbol",
    "path",
    "workspace",
    "include",
    "depth",
    "max_bytes",
    "timeout_ms",
    "structured",
];
pub(super) const SEARCH_FIELDS: &[&str] = &[
    "apply",
    "grep",
    "path",
    "workspace",
    "limit",
    "fixed_strings",
    "max_bytes",
    "timeout_ms",
    "structured",
];

pub(super) fn expand(value: Value) -> Result<Value> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("workflow must be an object"))?;
    let modes = ["steps", "symbol", "grep"]
        .into_iter()
        .filter(|key| object.contains_key(*key))
        .collect::<Vec<_>>();
    if modes.len() != 1 {
        bail!("provide exactly one of steps, symbol or grep");
    }
    let fields = match modes[0] {
        "steps" => PLAN_FIELDS,
        "symbol" => SYMBOL_FIELDS,
        _ => SEARCH_FIELDS,
    };
    if let Some(unknown) = object.keys().find(|key| !fields.contains(&key.as_str())) {
        bail!(
            "field '{unknown}' is not supported in the {} form",
            modes[0]
        );
    }
    if value.get("steps").is_some() {
        if !value["steps"].is_array() {
            bail!("steps must be an array");
        }
        return Ok(value);
    }
    if value.get("grep").is_some() {
        return expand_search(serde_json::from_value(value)?);
    }
    let input: Inspect = serde_json::from_value(value)?;
    if input.apply == Some(true) {
        bail!("short-form investigations cannot apply edits; use explicit steps");
    }
    if input.depth.is_some_and(|depth| depth > 3) {
        bail!("depth must be in 0..=3");
    }
    if input.symbol.trim().is_empty() {
        bail!("symbol must not be empty");
    }
    if input.include.is_empty() || input.include.len() > 5 {
        bail!("include requires 1..=5 investigation sections");
    }
    let mut steps = vec![json!({"id":"target","tool":"resolve","args":{"symbol":input.symbol}})];
    if let Some(path) = input.path {
        steps[0]["args"]["path"] = json!(path);
    }
    let mut outputs = Vec::new();
    let slice_budget =
        input.max_bytes.unwrap_or(24_000).saturating_sub(1024) / (2 * input.include.len());
    for section in input.include {
        if outputs.contains(&section) {
            bail!("duplicate include section: {section}");
        }
        let tool = match section.as_str() {
            "body" => "code_definition",
            "dependencies" => "code_slice",
            "hover" => "code_hover",
            "callers" => "code_callers",
            "references" => "code_references",
            _ => bail!(
                "unsupported include section: {section}; use body, dependencies, hover, callers or references"
            ),
        };
        let mut args = json!({"path":{"$ref":"target#/path"},"line":{"$ref":"target#/line"},"character":{"$ref":"target#/character"}});
        if section == "dependencies" {
            args["depth"] = json!(input.depth.unwrap_or(1));
            args["max_bytes"] = json!(slice_budget);
        }
        steps.push(json!({"id":section,"tool":tool,"args":args}));
        outputs.push(section);
    }
    let mut plan = json!({"steps":steps,"outputs":outputs});
    if let Some(structured) = input.structured {
        plan["structured"] = json!(structured);
    }
    if let Some(workspace) = input.workspace {
        plan["workspace"] = json!(workspace);
    }
    if let Some(max_bytes) = input.max_bytes {
        plan["max_bytes"] = json!(max_bytes);
    }
    if let Some(timeout_ms) = input.timeout_ms {
        plan["timeout_ms"] = json!(timeout_ms);
    }
    Ok(plan)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    grep: String,
    apply: Option<bool>,
    structured: Option<bool>,
    path: Option<String>,
    workspace: Option<String>,
    limit: Option<usize>,
    fixed_strings: Option<bool>,
    max_bytes: Option<usize>,
    timeout_ms: Option<u64>,
}

fn expand_search(input: Search) -> Result<Value> {
    if input.apply == Some(true) {
        bail!("short-form search cannot apply edits; use explicit steps");
    }
    if input.grep.trim().is_empty() {
        bail!("grep must contain non-whitespace text");
    }
    let limit = input.limit.unwrap_or(3);
    if !(1..=8).contains(&limit) {
        bail!("limit must be 1..=8");
    }
    let mut args = json!({"pattern":input.grep,"limit":limit,"fixed_strings":input.fixed_strings.unwrap_or(true)});
    if let Some(path) = input.path {
        args["path"] = json!(path);
    }
    let mut plan = json!({"outputs":["matches","bodies"],"steps":[
        {"id":"matches","tool":"grep","args":args},
        {"id":"bodies","tool":"code_definition","for_each":{"$ref":"matches#/items"},"args":{
            "path":{"$ref":"$item#/path"},"symbol":{"$ref":"$item#/symbol"}
        }}
    ]});
    if let Some(structured) = input.structured {
        plan["structured"] = json!(structured);
    }
    if let Some(workspace) = input.workspace {
        plan["workspace"] = json!(workspace);
    }
    if let Some(max_bytes) = input.max_bytes {
        plan["max_bytes"] = json!(max_bytes);
    }
    if let Some(timeout_ms) = input.timeout_ms {
        plan["timeout_ms"] = json!(timeout_ms);
    }
    Ok(plan)
}
