/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

fn reference(value: &Value) -> Result<Option<(&str, &str)>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let Some(raw) = object.get("$ref") else {
        return Ok(None);
    };
    if object.len() != 1 {
        bail!("a $ref object must not contain other keys");
    }
    let (id, pointer) = raw
        .as_str()
        .and_then(|s| s.split_once('#'))
        .context("use $ref: 'step#/json/pointer' (or '$item#/pointer' inside for_each)")?;
    if !pointer.is_empty() && !pointer.starts_with('/') {
        bail!("$ref requires an RFC 6901 JSON pointer after '#'");
    }
    Ok(Some((id, pointer)))
}

pub(super) fn is_reference(value: &Value) -> bool {
    reference(value).is_ok_and(|reference| reference.is_some())
}

pub(super) fn validate(value: &Value, ids: &HashSet<String>, has_item: bool) -> Result<()> {
    if let Some((id, _)) = reference(value)? {
        if (id == "$item" && !has_item) || (id != "$item" && !ids.contains(id)) {
            bail!("reference must name an earlier step, or $item inside for_each: {id}");
        }
        return Ok(());
    }
    match value {
        Value::Object(object) => {
            for child in object.values() {
                validate(child, ids, has_item)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                validate(child, ids, has_item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn resolve(
    value: &Value,
    values: &HashMap<String, Value>,
    item: Option<&Value>,
) -> Result<Value> {
    if let Some((id, pointer)) = reference(value)? {
        let target = if id == "$item" { item } else { values.get(id) }
            .context("referenced step did not produce a successful result")?;
        return target
            .pointer(pointer)
            .cloned()
            .context("referenced JSON pointer does not exist");
    }
    Ok(match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, child)| Ok((key.clone(), resolve(child, values, item)?)))
                .collect::<Result<_>>()?,
        ),
        Value::Array(array) => Value::Array(
            array
                .iter()
                .map(|child| resolve(child, values, item))
                .collect::<Result<_>>()?,
        ),
        _ => value.clone(),
    })
}
