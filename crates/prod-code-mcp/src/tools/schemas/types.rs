/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub const POSITION_ARGUMENTS: [&str; 4] = ["line", "character", "end_line", "end_character"];

pub const COMPILE_DESCRIPTION: &str = "Also run the project's check command (`cargo check` for Rust, \
     `go build`, `tsc`, ...) on the proposed text in a private shadow copy on the node, and report \
     the compiler's errors: the analyzer does not check everything the compiler does (rust-analyzer \
     runs no borrow checker, so a reference to a local, E0515, or a use after a move, E0382, passes \
     without it; nor does it report a private function of another crate, E0603). Slower: a build, \
     warm on the node";

/// Tools that accept `symbol` in place of `path`/`line`/`character`.
pub const SYMBOL_ADDRESSABLE: &[&str] = &[
    "code_slice",
    "code_change_signature",
    "code_move",
    "code_introduce_parameter_object",
    "code_migrate_type",
    "code_encapsulate_field",
    "code_wrap_return",
    "code_make_static",
    "code_convert_to_method",
    "code_invert_boolean",
    "code_generify",
    "code_definition",
    "code_references",
    "code_hover",
    "code_type_at",
    "code_callers",
    "code_callees",
    "code_implementations",
    "code_supertypes",
    "code_rename",
    "code_safe_delete",
    "code_assists",
    "code_assist",
    "code_replace_constructor_with_factory",
    "code_replace_constructor_with_builder",
    "code_replace_constructor",
    "code_pull_up",
    "code_push_down",
    "code_replace_inheritance_with_delegation",
    "code_replace_conditional_with_polymorphism",
    "code_extract_interface",
];

/// Symbol-addressable tools accept `symbol` instead of a position: advertise the property and
/// stop requiring path/line/character, otherwise a schema-validating client cannot use the
/// name-based form at all.
pub fn relax_position_schema(schema: &mut serde_json::Value) {
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        props.entry("symbol").or_insert_with(|| {
            serde_json::json!({
                "type": "string",
                "description": "Symbol name instead of path/line/character, optionally qualified (`Metrics::record`, `pkg.Func`, `Class.method`); resolved through the workspace symbol index. `path` may still be given to disambiguate."
            })
        });
    }
    if let Some(required) = schema.get_mut("required").and_then(|r| r.as_array_mut()) {
        required.retain(|r| !matches!(r.as_str(), Some("path") | Some("line") | Some("character")));
    }
}
