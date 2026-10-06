/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Test mock generator: compile-ready mock structs and classes
//! implementing interface/trait/protocol contracts with call tracking and configurable stubs.

pub(crate) mod compiled;
pub(crate) mod go_ts;
pub(crate) mod helpers;
pub(crate) mod python_rust;

#[cfg(test)]
mod tests;

use crate::parameter_object::Language;

/// One method signature in an interface, trait, or protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodSignature {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub return_type: Option<String>,
}

/// Generates a test mock implementation of `type_name` for `language`.
pub fn generate_mock(
    language: Language,
    type_name: &str,
    methods: &[MethodSignature],
    fields: &[(String, String)],
) -> String {
    match language {
        Language::Go => go_ts::generate_go_mock(type_name, methods, fields),
        Language::TypeScript | Language::JavaScript => {
            go_ts::generate_ts_mock(type_name, methods, fields)
        }
        Language::Python => python_rust::generate_python_mock(type_name, methods, fields),
        Language::Rust => python_rust::generate_rust_mock(type_name, methods, fields),
        Language::Cpp | Language::C => compiled::generate_cpp_mock(type_name, methods, fields),
        Language::Swift => compiled::generate_swift_mock(type_name, methods, fields),
        Language::Java => compiled::generate_java_mock(type_name, methods, fields),
    }
}
