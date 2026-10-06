/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod cpp;
mod go;
mod node;
mod python;
mod rust;
mod swift;
mod util;

pub use util::strip_ansi;

use crate::dossier::types::AssertionEvidence;
use cpp::{parse_catch2_assertion, parse_gtest_assertion};
use go::{parse_go_got_want_assertion, parse_go_testify_assertion};
use node::{parse_jest_node_assert, parse_node_error_fields, parse_node_short_message};
use python::{parse_pytest_assertion, parse_python_unittest_assertion};
use rust::parse_rust_assertion;
use swift::{parse_swift_testing_assertion, parse_swift_xctest_assertion};

/// Parses structured assertion evidence from one test's failure output across polyglot runners:
/// Rust `assert_eq!` / `assert_ne!` panics, Node/TS `assert.strictEqual` / `assert.deepStrictEqual`
/// (Node, Jest, Vitest), Python pytest and unittest assertions, Go testify and got/want conventions,
/// Swift XCTest and swift-testing, and C++ GoogleTest and Catch2. A value is taken only where
/// the printed layout shows where it starts and ends; an incomplete block or an elided value
/// yields `None`. Nothing is evaluated.
pub fn parse_assertion_evidence(output: &str) -> Option<AssertionEvidence> {
    if output.trim().is_empty() {
        return None;
    }
    let raw: Vec<&str> = output.lines().collect();
    let stripped: Vec<String> = raw.iter().map(|l| strip_ansi(l)).collect();
    // Each format answers `None` when it is absent and `Some(None)` when it is present but
    // cannot be bound completely: then no looser format may take part of the same block.
    parse_rust_assertion(&raw, &stripped)
        .or_else(|| parse_node_error_fields(&raw, &stripped))
        .or_else(|| parse_jest_node_assert(&raw, &stripped))
        .or_else(|| parse_node_short_message(&raw, &stripped))
        .or_else(|| parse_pytest_assertion(&raw, &stripped))
        .or_else(|| parse_python_unittest_assertion(&raw, &stripped))
        .or_else(|| parse_go_testify_assertion(&raw, &stripped))
        .or_else(|| parse_go_got_want_assertion(&raw, &stripped))
        .or_else(|| parse_swift_xctest_assertion(&raw, &stripped))
        .or_else(|| parse_swift_testing_assertion(&raw, &stripped))
        .or_else(|| parse_gtest_assertion(&raw, &stripped))
        .or_else(|| parse_catch2_assertion(&raw, &stripped))
        .flatten()
}
