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
use serde_json::Value;
use std::collections::HashSet;

pub(super) const TOOLS: &[&str] = &[
    "resolve",
    "symbols",
    "grep",
    "code_definition",
    "code_hover",
    "code_outline",
    "code_references",
    "code_callers",
    "code_callees",
    "code_implementations",
    "code_supertypes",
    "code_search",
    "code_grep",
    "code_slice",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Plan {
    pub steps: Vec<Step>,
    #[serde(default)]
    pub structured: bool,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default = "default_bytes")]
    pub max_bytes: usize,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_bytes() -> usize {
    24_000
}
fn default_timeout() -> u64 {
    30_000
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Step {
    pub id: String,
    pub tool: String,
    #[serde(default = "empty_args")]
    pub args: Value,
    #[serde(default)]
    pub for_each: Option<Value>,
    #[serde(default)]
    pub when: Option<Condition>,
    #[serde(default)]
    pub on_error: OnError,
}

fn empty_args() -> Value {
    serde_json::json!({})
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum OnError {
    #[default]
    Stop,
    Continue,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Condition {
    pub step: String,
    pub status: ConditionStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ConditionStatus {
    Ok,
    Error,
}

impl Plan {
    pub fn parse(args: Value) -> Result<Self> {
        if serde_json::to_vec(&args)?.len() > 65_536 {
            bail!("workflow input exceeds 65536 bytes");
        }
        let plan: Self = serde_json::from_value(args)?;
        if plan.steps.is_empty() || plan.steps.len() > 16 {
            bail!("workflow requires 1..=16 steps");
        }
        if !(4096..=65_536).contains(&plan.max_bytes) {
            bail!("max_bytes must be 4096..=65536");
        }
        if !(1..=60_000).contains(&plan.timeout_ms) {
            bail!("timeout_ms must be 1..=60000");
        }
        let mut ids = HashSet::new();
        for step in &plan.steps {
            if step.id.is_empty()
                || step.id.len() > 48
                || !step
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                bail!("step ids must contain 1..=48 ASCII letters, digits or underscores");
            }
            if !TOOLS.contains(&step.tool.as_str()) {
                bail!(
                    "unsupported workflow tool: {}; this version supports investigation only",
                    step.tool
                );
            }
            if !step.args.is_object() {
                bail!("step args must be objects");
            }
            validate_arguments(&step.args)?;
            validate_adapter_arguments(&step.tool, &step.args)?;
            super::references::validate(&step.args, &ids, step.for_each.is_some())?;
            if let Some(items) = &step.for_each {
                super::references::validate(items, &ids, false)?;
            }
            if let Some(condition) = &step.when
                && !ids.contains(&condition.step)
            {
                bail!("when must name an earlier step");
            }
            if !ids.insert(step.id.clone()) {
                bail!("duplicate step id: {}", step.id);
            }
        }
        for output in &plan.outputs {
            if !ids.contains(output) {
                bail!("unknown output step: {output}");
            }
        }
        Ok(plan)
    }
}

pub(super) fn validate_arguments(value: &Value) -> Result<()> {
    if !value.is_object() {
        bail!("resolved step args must be an object");
    }
    if let Some(object) = value.as_object() {
        for key in [
            "workspace",
            "also_in",
            "build_index",
            "apply",
            "fix",
            "run",
            "symbols",
        ] {
            if object.contains_key(key) {
                bail!(
                    "workflow step argument '{key}' is unsupported; choose one workspace and investigation operations"
                );
            }
        }
        if let Some(symbol) = object.get("symbol")
            && !symbol.is_string()
            && !super::references::is_reference(symbol)
        {
            bail!(
                "symbol must be a string or exact reference; use bounded for_each for multiple symbols"
            );
        }
    }
    Ok(())
}

pub(super) fn limit(args: &Value, default: u64) -> Result<usize> {
    let limit = match args.get("limit") {
        None => default,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("limit must be an integer in 1..=8"))?,
    };
    if !(1..=8).contains(&limit) {
        bail!("limit must be in 1..=8");
    }
    Ok(limit as usize)
}

pub(super) fn validate_adapter_arguments(tool: &str, args: &Value) -> Result<()> {
    if matches!(tool, "symbols" | "grep") {
        if args
            .get("limit")
            .is_some_and(|value| !super::references::is_reference(value))
        {
            limit(args, 8)?;
        }
        let key = if tool == "symbols" {
            "query"
        } else {
            "pattern"
        };
        if let Some(value) = args.get(key)
            && !super::references::is_reference(value)
        {
            let text = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{key} must be a string"))?;
            if text.is_empty() {
                bail!("{key} must not be empty");
            }
        }
    }
    Ok(())
}
