/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    plan::{ConditionStatus, OnError, Plan, Step},
    references,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::HashMap, future::Future};
use tokio::time::{Duration, Instant, timeout_at};

#[derive(Debug)]
struct PartialIteration {
    message: String,
    completed: Vec<Value>,
}
impl std::fmt::Display for PartialIteration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for PartialIteration {}

/// The plan is fully parsed before any operation. Each inner call keeps its own routing,
/// locking and semantic error behavior. Nothing here retries a partially executed plan.
pub(super) async fn run<F, Fut>(plan: Plan, mut call: F) -> Value
where
    F: FnMut(String, Value) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let deadline = Instant::now() + Duration::from_millis(plan.timeout_ms);
    let mut values = HashMap::new();
    let mut statuses = HashMap::new();
    let mut results = Vec::new();
    let mut calls = 0;
    let mut retained = 0;
    let mut stopped = false;
    let mut partial = false;
    let mut truncated = false;
    for step in &plan.steps {
        let enabled = step.when.as_ref().is_none_or(|condition| {
            let expected = match condition.status {
                ConditionStatus::Ok => "ok",
                ConditionStatus::Error => "error",
            };
            statuses.get(&condition.step).copied() == Some(expected)
        });
        if stopped || !enabled {
            statuses.insert(step.id.clone(), "skipped");
            results.push(json!({"id":step.id,"status":"skipped"}));
            continue;
        }
        let start = Instant::now();
        let outcome = execute_step(step, &values, deadline, &mut calls, &mut call).await;
        let mut result = json!({"id":step.id,"elapsed_ms": start.elapsed().as_millis() as u64});
        match outcome {
            Ok(value) => {
                let warnings = super::metadata::warnings(&value);
                partial |= super::metadata::partial(&warnings);
                if !warnings.is_empty() {
                    result["warnings"] = json!(warnings);
                }
                let size = serde_json::to_vec(&value).map_or(usize::MAX, |bytes| bytes.len());
                if size > 262_144 || retained + size > 1_048_576 {
                    partial = true;
                    statuses.insert(step.id.clone(), "error");
                    result["status"] = json!("error");
                    result["error"] = json!(
                        "intermediate result exceeds workflow memory budget; narrow the operation"
                    );
                    stopped = step.on_error == OnError::Stop;
                } else {
                    partial |= value.get("truncated").and_then(Value::as_bool) == Some(true);
                    retained += size;
                    statuses.insert(step.id.clone(), "ok");
                    values.insert(step.id.clone(), value.clone());
                    result["status"] = json!("ok");
                    let selected = if plan.outputs.is_empty() {
                        plan.steps.last().is_some_and(|last| last.id == step.id)
                    } else {
                        plan.outputs.contains(&step.id)
                    };
                    if selected {
                        result["value"] = value;
                    }
                }
            }
            Err(error) => {
                partial = true;
                statuses.insert(step.id.clone(), "error");
                result["status"] = json!("error");
                let message = format!("{error:#}");
                result["error"] = json!(message.chars().take(2048).collect::<String>());
                let selected = plan.outputs.contains(&step.id)
                    || (plan.outputs.is_empty()
                        && plan.steps.last().is_some_and(|last| last.id == step.id));
                if selected && let Some(partial) = error.downcast_ref::<PartialIteration>() {
                    result["partial_value"] = json!(partial.completed);
                }
                if let Some(partial) = error.downcast_ref::<PartialIteration>() {
                    let warnings = super::metadata::warnings(&json!(partial.completed));
                    if !warnings.is_empty() {
                        result["warnings"] = json!(warnings);
                    }
                }
                stopped = step.on_error == OnError::Stop;
            }
        }
        results.push(result);
    }
    // Keep structured outputs valid JSON: omit oversized selected values rather than
    // chopping serialized JSON, positions or source code at an arbitrary byte boundary.
    let mut report = json!({"status":if partial {"partial"} else {"complete"}, "executed_calls":calls,"truncated":false,"steps":results});
    if serde_json::to_vec(&report).is_ok_and(|bytes| bytes.len() > plan.max_bytes) {
        let mut sizes = report["steps"]
            .as_array()
            .expect("step array")
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let size = ["value", "partial_value"]
                    .iter()
                    .filter_map(|key| step.get(key))
                    .map(|value| serde_json::to_vec(value).map_or(0, |bytes| bytes.len()))
                    .sum::<usize>();
                (index, size)
            })
            .collect::<Vec<_>>();
        sizes.sort_by_key(|(index, size)| (std::cmp::Reverse(*size), std::cmp::Reverse(*index)));
        for (index, size) in sizes {
            if serde_json::to_vec(&report).is_ok_and(|bytes| bytes.len() <= plan.max_bytes) {
                break;
            }
            if size == 0 {
                continue;
            }
            let original = report["steps"][index].clone();
            let before = serde_json::to_vec(&report).map_or(usize::MAX, |bytes| bytes.len());
            let result = &mut report["steps"][index];
            let object = result.as_object_mut().expect("step object");
            let removed = object.remove("value").is_some();
            let partial_removed = object.remove("partial_value").is_some();
            if removed || partial_removed {
                result["output_omitted"] =
                    json!("max_bytes budget; request a smaller result or select fewer outputs");
            }
            if (removed || partial_removed)
                && serde_json::to_vec(&report).is_ok_and(|bytes| bytes.len() < before)
            {
                truncated = true;
            } else {
                report["steps"][index] = original;
            }
        }
    }
    if serde_json::to_vec(&report).is_ok_and(|bytes| bytes.len() > plan.max_bytes) {
        for result in report["steps"].as_array_mut().expect("step array") {
            if let Some(error) = result["error"].as_str() {
                let shortened = error.chars().take(24).collect::<String>();
                result["error"] = json!(shortened);
                result["error_truncated"] = json!(true);
                truncated = true;
            }
        }
    }
    if serde_json::to_vec(&report).is_ok_and(|bytes| bytes.len() > plan.max_bytes) {
        for result in report["steps"].as_array_mut().expect("step array") {
            let object = result.as_object_mut().expect("step object");
            object.remove("elapsed_ms");
            if object.contains_key("error") {
                object.insert("error".into(), json!("omitted: output budget"));
                object.insert("error_truncated".into(), json!(true));
            }
            if object.contains_key("output_omitted") {
                object.insert("output_omitted".into(), json!(true));
            }
        }
        truncated = true;
    }
    if serde_json::to_vec(&report).is_ok_and(|bytes| bytes.len() > plan.max_bytes) {
        let mut warning_steps = std::collections::BTreeMap::<String, Vec<usize>>::new();
        for (index, result) in report["steps"]
            .as_array_mut()
            .expect("step array")
            .iter_mut()
            .enumerate()
        {
            if let Some(Value::Array(warnings)) = result
                .as_object_mut()
                .expect("step object")
                .remove("warnings")
            {
                for warning in warnings {
                    if let Some(kind) = warning.as_str() {
                        warning_steps.entry(kind.into()).or_default().push(index);
                    }
                }
            }
        }
        report["warning_steps"] = json!(warning_steps);
        report["warnings_compacted"] = json!(true);
        truncated = true;
    }
    report["truncated"] = json!(truncated);
    if truncated {
        report["status"] = json!("partial");
    }
    report
}

async fn execute_step<F, Fut>(
    step: &Step,
    values: &HashMap<String, Value>,
    deadline: Instant,
    calls: &mut usize,
    call: &mut F,
) -> Result<Value>
where
    F: FnMut(String, Value) -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let items = if let Some(source) = &step.for_each {
        let value = references::resolve(source, values, None)?;
        let array = value
            .as_array()
            .context("for_each must resolve to an array")?;
        if array.len() > 8 {
            bail!("for_each exceeds 8 items; narrow the earlier result");
        }
        Some(array.clone())
    } else {
        None
    };
    let mut output = Vec::new();
    let inputs: Vec<_> = items
        .as_ref()
        .map_or_else(|| vec![None], |items| items.iter().map(Some).collect());
    for item in inputs {
        let result = async {
            if *calls >= 32 {
                bail!("workflow reached its 32-call budget");
            }
            if Instant::now() >= deadline {
                bail!("workflow deadline reached; earlier steps are preserved");
            }
            let args = references::resolve(&step.args, values, item)?;
            super::plan::validate_arguments(&args)?;
            *calls += 1;
            timeout_at(deadline, call(step.tool.clone(), args))
                .await
                .context("workflow deadline reached; earlier steps are preserved")?
        }
        .await;
        match result {
            Ok(value) => output.push(value),
            Err(error) if items.is_some() && !output.is_empty() => {
                return Err(PartialIteration {
                    message: format!("{error:#}"),
                    completed: output,
                }
                .into());
            }
            Err(error) => return Err(error),
        }
    }
    if items.is_some() {
        Ok(json!(output))
    } else {
        Ok(output.pop().expect("one call"))
    }
}
