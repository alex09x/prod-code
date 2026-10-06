/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod advanced_refactoring;
pub mod analysis;
pub mod execution;
pub mod navigation;
pub mod refactoring;
pub mod types;
pub mod utility;

pub use types::{
    COMPILE_DESCRIPTION, POSITION_ARGUMENTS, SYMBOL_ADDRESSABLE, relax_position_schema,
};

use crate::protocol::McpTool;

pub fn build_tools_raw() -> Vec<McpTool> {
    let mut tools = Vec::new();
    tools.extend(execution::execution_tools());
    tools.extend(navigation::navigation_tools());
    tools.extend(refactoring::refactoring_tools());
    tools.extend(advanced_refactoring::advanced_refactoring_tools());
    tools.extend(analysis::analysis_tools());
    tools.extend(utility::utility_tools());

    for tool in &mut tools {
        if let Some(properties) = tool
            .input_schema
            .get_mut("properties")
            .and_then(|v| v.as_object_mut())
        {
            for key in POSITION_ARGUMENTS {
                if let Some(property) = properties.get_mut(key) {
                    property["minimum"] = serde_json::json!(1);
                    property["maximum"] = serde_json::json!(u32::MAX);
                }
            }
            for key in ["end_line", "end_character"] {
                if let Some(description) = properties
                    .get_mut(key)
                    .and_then(|p| p.get_mut("description"))
                    && let Some(text) = description.as_str()
                {
                    *description = serde_json::json!(format!(
                        "{text}; supply both end_line and end_character, with the end at or after the start"
                    ));
                }
            }
        }
        if SYMBOL_ADDRESSABLE.contains(&tool.name.as_str()) {
            relax_position_schema(&mut tool.input_schema);
        }
    }
    tools
}
