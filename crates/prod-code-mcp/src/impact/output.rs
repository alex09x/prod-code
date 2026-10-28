/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::{Map, Value};

use super::types::ImpactReport;

pub const MAX_IMPACT_OUTPUT_ITEMS: usize = 25;

pub(crate) fn append_omitted(out: &mut String, total: usize, label: &str, indent: &str) {
    if total > MAX_IMPACT_OUTPUT_ITEMS {
        out.push_str(&format!(
            "{indent}... ({} more {label} omitted)\n",
            total - MAX_IMPACT_OUTPUT_ITEMS
        ));
    }
}

/// A compact JSON view of an impact report. The full analysis remains available to callers in-process.
pub fn bounded_json_value(report: &ImpactReport) -> Value {
    let mut value = serde_json::to_value(report).expect("ImpactReport is serializable");
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    let mut truncated = Map::new();
    for field in [
        "changed_files",
        "changed",
        "callers",
        "tests",
        "reaches",
        "incomplete",
        "unattributed_files",
        "signature_warnings",
    ] {
        truncate_array(object, field, field, &mut truncated);
    }
    if let Some(Value::Array(warnings)) = object.get_mut("signature_warnings") {
        for (index, warning) in warnings.iter_mut().enumerate() {
            if let Some(warning) = warning.as_object_mut() {
                truncate_array(
                    warning,
                    "unadjusted_call_sites",
                    &format!("signature_warnings[{index}].unadjusted_call_sites"),
                    &mut truncated,
                );
            }
        }
    }
    if let Some(Value::Object(plan)) = object.get_mut("test_plan") {
        for field in [
            "selection_fallbacks",
            "unsupported_selected",
            "unsupported_whole",
        ] {
            truncate_array(plan, field, &format!("test_plan.{field}"), &mut truncated);
        }
    }
    if !truncated.is_empty() {
        object.insert("output_truncated".to_string(), Value::Object(truncated));
    }
    value
}

fn truncate_array(
    object: &mut Map<String, Value>,
    field: &str,
    label: &str,
    truncated: &mut Map<String, Value>,
) {
    if let Some(Value::Array(items)) = object.get_mut(field)
        && items.len() > MAX_IMPACT_OUTPUT_ITEMS
    {
        let total = items.len();
        items.truncate(MAX_IMPACT_OUTPUT_ITEMS);
        truncated.insert(
            label.to_string(),
            serde_json::json!({ "included": MAX_IMPACT_OUTPUT_ITEMS, "total": total }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::impact::types::{CallSite, ImpactTestPlan, SignatureWarning, Symbol};

    fn symbol(index: usize) -> Symbol {
        Symbol {
            name: format!("caller_{index}"),
            file: format!("src/caller_{index}.rs"),
            line: 1,
            col: 1,
        }
    }

    fn signature_warning(index: usize) -> SignatureWarning {
        SignatureWarning {
            symbol: symbol(index),
            old_signature: "fn f()".to_string(),
            new_signature: "fn f(arg: u32)".to_string(),
            unadjusted_call_sites: (0..60)
                .map(|call| CallSite {
                    file: format!("src/caller_{call}.rs"),
                    line: 1,
                    col: 1,
                    caller: Some(format!("caller_{call}")),
                    is_sibling: true,
                })
                .collect(),
        }
    }

    #[test]
    fn json_output_caps_large_arrays_and_reports_truncation() {
        let report = ImpactReport {
            language: "rust".to_string(),
            base: "HEAD".to_string(),
            changed_files: Vec::new(),
            changed: Vec::new(),
            callers: (0..60).map(symbol).collect(),
            tests: Vec::new(),
            test_command: None,
            test_plan: None,
            unattributed_files: Vec::new(),
            index: None,
            reaches: Vec::new(),
            incomplete: Vec::new(),
            signature_warnings: Vec::new(),
        };

        let output = bounded_json_value(&report);
        assert_eq!(report.callers.len(), 60, "the analysis result is unchanged");
        assert_eq!(
            output["callers"].as_array().unwrap().len(),
            MAX_IMPACT_OUTPUT_ITEMS
        );
        assert_eq!(
            output["output_truncated"]["callers"]["total"],
            serde_json::json!(60)
        );
        assert_eq!(
            output["output_truncated"]["callers"]["included"].as_u64(),
            Some(MAX_IMPACT_OUTPUT_ITEMS as u64)
        );
        assert!(report.render().contains("35 more callers omitted"));
    }

    #[test]
    fn json_output_keeps_test_command_argv_and_tracks_each_warning_truncation() {
        let command: Vec<String> = (0..60).map(|index| format!("arg_{index}")).collect();
        let report = ImpactReport {
            language: "rust".to_string(),
            base: "HEAD".to_string(),
            changed_files: Vec::new(),
            changed: Vec::new(),
            callers: Vec::new(),
            tests: Vec::new(),
            test_command: Some(command.clone()),
            test_plan: Some(ImpactTestPlan {
                selected: Some(command.clone()),
                selection_fallbacks: Vec::new(),
                unsupported_selected: Vec::new(),
                whole: Some(command.clone()),
                whole_is_root_only: false,
                unsupported_whole: Vec::new(),
            }),
            unattributed_files: Vec::new(),
            index: None,
            reaches: Vec::new(),
            incomplete: Vec::new(),
            signature_warnings: vec![signature_warning(0), signature_warning(1)],
        };

        let output = bounded_json_value(&report);
        assert_eq!(output["test_command"].as_array().unwrap().len(), 60);
        assert_eq!(
            output["test_plan"]["selected"].as_array().unwrap().len(),
            60
        );
        assert_eq!(output["test_plan"]["whole"].as_array().unwrap().len(), 60);
        for index in 0..2 {
            let key = format!("signature_warnings[{index}].unadjusted_call_sites");
            assert_eq!(
                output["output_truncated"][key.as_str()]["total"],
                serde_json::json!(60)
            );
            assert_eq!(
                output["output_truncated"][key.as_str()]["included"].as_u64(),
                Some(MAX_IMPACT_OUTPUT_ITEMS as u64)
            );
        }
    }
}
