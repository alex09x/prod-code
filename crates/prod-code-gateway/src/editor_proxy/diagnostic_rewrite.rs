/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ROUNDTRIP_MARKER_KEY: &str = "_prod_code_roundtrip";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataState {
    #[serde(rename = "absent", alias = "Absent")]
    Absent,
    #[serde(rename = "null", alias = "Null")]
    Null,
    #[serde(rename = "object", alias = "Object")]
    Object,
    #[serde(rename = "other", alias = "Other")]
    Other,
}

impl Default for DataState {
    fn default() -> Self {
        Self::Absent
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundTripMarker {
    #[serde(default = "default_code")]
    pub code: Value,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "code_description"
    )]
    pub code_description: Option<Value>,
    #[serde(default, alias = "data_state")]
    pub data_state: DataState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "original_data"
    )]
    pub original_data: Option<Value>,
}

fn default_code() -> Value {
    Value::String("E0559".to_string())
}

pub fn extract_roundtrip_marker_from_map(
    diag_obj: &serde_json::Map<String, Value>,
) -> Option<RoundTripMarker> {
    if let Some(data) = diag_obj.get("data") {
        if let Some(marker_val) = data.get(ROUNDTRIP_MARKER_KEY) {
            if let Ok(marker) = serde_json::from_value::<RoundTripMarker>(marker_val.clone()) {
                return Some(marker);
            }
        }
    }
    if let Some(marker_val) = diag_obj.get(ROUNDTRIP_MARKER_KEY) {
        if let Ok(marker) = serde_json::from_value::<RoundTripMarker>(marker_val.clone()) {
            return Some(marker);
        }
    }
    None
}

pub fn extract_roundtrip_marker(diag: &Value) -> Option<RoundTripMarker> {
    diag.as_object().and_then(extract_roundtrip_marker_from_map)
}

pub fn rewrite_diagnostic_for_editor(diag: &mut Value) -> bool {
    let diag_obj = match diag.as_object_mut() {
        Some(obj) => obj,
        None => return false,
    };

    let is_e0559 = diag_obj.get("code").and_then(|c| c.as_str()) == Some("E0559");
    let is_missing_field = diag_obj
        .get("message")
        .and_then(|m| m.as_str())
        .is_some_and(|m| m.starts_with("no field `"));

    if !is_e0559 || !is_missing_field {
        return false;
    }

    let orig_code = diag_obj
        .get("code")
        .cloned()
        .unwrap_or_else(|| Value::String("E0559".to_string()));
    let orig_code_description = diag_obj.get("codeDescription").cloned();

    let (data_state, orig_data) = match diag_obj.get("data") {
        None => (DataState::Absent, None),
        Some(Value::Null) => (DataState::Null, None),
        Some(Value::Object(obj)) => (DataState::Object, Some(Value::Object(obj.clone()))),
        Some(other) => (DataState::Other, Some(other.clone())),
    };

    let marker = RoundTripMarker {
        code: orig_code,
        code_description: orig_code_description,
        data_state,
        original_data: orig_data,
    };
    let marker_json = serde_json::to_value(&marker).unwrap_or(Value::Null);

    diag_obj.insert("code".to_string(), Value::String("E0609".to_string()));

    if let Some(cd) = diag_obj.get_mut("codeDescription") {
        if let Some(href) = cd.get_mut("href").and_then(|h| h.as_str()) {
            if href.contains("E0559") {
                let new_href = href.replace("E0559", "E0609");
                cd["href"] = Value::String(new_href);
            }
        }
    }

    match data_state {
        DataState::Absent | DataState::Null | DataState::Other => {
            let mut new_data = serde_json::Map::new();
            new_data.insert(ROUNDTRIP_MARKER_KEY.to_string(), marker_json);
            diag_obj.insert("data".to_string(), Value::Object(new_data));
        }
        DataState::Object => {
            if let Some(data_obj) = diag_obj.get_mut("data").and_then(|d| d.as_object_mut()) {
                data_obj.insert(ROUNDTRIP_MARKER_KEY.to_string(), marker_json);
            }
        }
    }

    true
}

pub fn restore_diagnostic_for_server(diag: &mut Value) -> bool {
    let diag_obj = match diag.as_object_mut() {
        Some(obj) => obj,
        None => return false,
    };

    let marker = extract_roundtrip_marker_from_map(diag_obj);

    let is_e0609_missing_field = diag_obj.get("code").and_then(|c| c.as_str()) == Some("E0609")
        && diag_obj
            .get("message")
            .and_then(|m| m.as_str())
            .is_some_and(|m| m.starts_with("no field `"));

    if marker.is_none() && !is_e0609_missing_field {
        return false;
    }

    if let Some(marker) = marker {
        diag_obj.insert("code".to_string(), marker.code);

        match marker.code_description {
            Some(cd) => {
                diag_obj.insert("codeDescription".to_string(), cd);
            }
            None => {
                diag_obj.remove("codeDescription");
            }
        }

        match marker.data_state {
            DataState::Absent => {
                diag_obj.remove("data");
            }
            DataState::Null => {
                diag_obj.insert("data".to_string(), Value::Null);
            }
            DataState::Object => {
                if let Some(data_obj) = diag_obj.get_mut("data").and_then(|v| v.as_object_mut()) {
                    data_obj.remove(ROUNDTRIP_MARKER_KEY);
                } else if let Some(orig) = marker.original_data {
                    diag_obj.insert("data".to_string(), orig);
                }
            }
            DataState::Other => {
                diag_obj.insert(
                    "data".to_string(),
                    marker.original_data.unwrap_or(Value::Null),
                );
            }
        }
        diag_obj.remove(ROUNDTRIP_MARKER_KEY);
    } else {
        diag_obj.insert("code".to_string(), Value::String("E0559".to_string()));
        if let Some(cd) = diag_obj.get_mut("codeDescription") {
            if let Some(href) = cd.get_mut("href").and_then(|h| h.as_str()) {
                if href.contains("E0609") {
                    let new_href = href.replace("E0609", "E0559");
                    cd["href"] = Value::String(new_href);
                }
            }
        }
    }

    true
}

pub fn normalize_server_diagnostics(raw: &str) -> String {
    if !raw.contains("E0559") {
        return raw.to_string();
    }
    let mut value = match serde_json::from_str::<Value>(raw) {
        Ok(v) => v,
        Err(_) => return raw.to_string(),
    };

    let mut modified = false;

    if let Some(params) = value.get_mut("params") {
        if let Some(diags) = params.get_mut("diagnostics").and_then(|d| d.as_array_mut()) {
            for diag in diags {
                if rewrite_diagnostic_for_editor(diag) {
                    modified = true;
                }
            }
        }
    }

    if let Some(result) = value.get_mut("result") {
        if let Some(items) = result.get_mut("items") {
            if rewrite_items(items) {
                modified = true;
            }
        }
        if let Some(related) = result.get_mut("relatedDocuments") {
            if rewrite_related_documents(related) {
                modified = true;
            }
        }
    }

    if modified {
        value.to_string()
    } else {
        raw.to_string()
    }
}

pub fn normalize_diagnostics_for_editor(raw: &str) -> String {
    normalize_server_diagnostics(raw)
}

fn rewrite_items(items: &mut Value) -> bool {
    let mut modified = false;
    if let Some(arr) = items.as_array_mut() {
        for item in arr {
            if rewrite_diagnostic_for_editor(item) {
                modified = true;
            }
            if let Some(nested) = item.get_mut("items") {
                if rewrite_items(nested) {
                    modified = true;
                }
            }
        }
    }
    modified
}

fn rewrite_related_documents(related: &mut Value) -> bool {
    let mut modified = false;
    if let Some(map) = related.as_object_mut() {
        for (_, doc_report) in map.iter_mut() {
            if let Some(items) = doc_report.get_mut("items") {
                if rewrite_items(items) {
                    modified = true;
                }
            } else if doc_report.is_array() {
                if rewrite_items(doc_report) {
                    modified = true;
                }
            }
        }
    }
    modified
}

pub fn restore_code_action_diagnostics(value: &mut Value) -> bool {
    let method = value.get("method").and_then(|m| m.as_str());
    if method != Some("textDocument/codeAction") && method != Some("codeAction") {
        return false;
    }

    let mut modified = false;
    if let Some(params) = value.get_mut("params") {
        if let Some(context) = params.get_mut("context") {
            if let Some(diagnostics) = context
                .get_mut("diagnostics")
                .and_then(|d| d.as_array_mut())
            {
                for diag in diagnostics {
                    if restore_diagnostic_for_server(diag) {
                        modified = true;
                    }
                }
            }
        }
    }

    modified
}

pub fn restore_diagnostics_for_server(raw: &str) -> String {
    if !raw.contains("E0609") && !raw.contains(ROUNDTRIP_MARKER_KEY) {
        return raw.to_string();
    }
    let mut value = match serde_json::from_str::<Value>(raw) {
        Ok(v) => v,
        Err(_) => return raw.to_string(),
    };
    if restore_code_action_diagnostics(&mut value) {
        value.to_string()
    } else {
        raw.to_string()
    }
}
