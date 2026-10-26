/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "extract_trait_syntax_preservation/conditional_cases.rs"]
mod conditional_cases;
#[path = "extract_trait_syntax_preservation/control_cases.rs"]
mod control_cases;
#[path = "extract_trait_syntax_preservation/execution_manifest.rs"]
mod execution_manifest;
#[path = "extract_trait_syntax_preservation/generic_const_cases.rs"]
mod generic_const_cases;
#[path = "extract_trait_syntax_preservation/recovered_cases.rs"]
mod recovered_cases;
#[path = "extract_trait_syntax_preservation/support.rs"]
mod support;
#[path = "extract_trait_syntax_preservation/visibility_cases.rs"]
mod visibility_cases;

pub use prod_code_mcp::extract_trait::{items, rewrite};

#[tokio::test]
async fn recovered_parser_regressions() {
    execution_manifest::capture();
    recovered_cases::recovered_parser_regressions().await;
}

#[path = "../src/extract_trait/tests.rs"]
mod existing_unit_tests;

#[tokio::test]
async fn method_conditional_attributes_are_preserved_on_trait_and_impl() {
    conditional_cases::method_cfg_and_cfg_attr_preservation().await;
}

#[tokio::test]
async fn impl_conditional_attributes_cover_every_generated_block() {
    conditional_cases::impl_cfg_and_cfg_attr_preservation().await;
}

#[tokio::test]
async fn widest_restricted_visibility_is_order_independent_and_preserves_caller_access() {
    visibility_cases::widest_visibility_preservation().await;
}

#[tokio::test]
async fn const_comparisons_and_negations_in_generic_bounds_are_preserved() {
    generic_const_cases::const_comparison_and_negation_preservation().await;
}

#[tokio::test]
async fn generic_where_nested_comment_controls_and_refusal_guards_hold() {
    control_cases::positive_controls_and_uncertainty_guards().await;
}
