/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;

pub fn format_polyglot_param(name: &str, ty: &str, value: &str, lang: Language) -> String {
    match lang {
        Language::TypeScript => {
            if !ty.is_empty() && !value.is_empty() {
                format!("{name}: {ty} = {value}")
            } else if !ty.is_empty() {
                format!("{name}: {ty}")
            } else if !value.is_empty() {
                format!("{name} = {value}")
            } else {
                name.to_string()
            }
        }
        Language::JavaScript => {
            if !value.is_empty() {
                format!("{name} = {value}")
            } else {
                name.to_string()
            }
        }
        Language::Python => {
            if !ty.is_empty() && !value.is_empty() {
                format!("{name}: {ty} = {value}")
            } else if !ty.is_empty() {
                format!("{name}: {ty}")
            } else if !value.is_empty() {
                format!("{name} = {value}")
            } else {
                name.to_string()
            }
        }
        // Added call sites receive `value` explicitly. A C++ default here would be
        // duplicated between a header declaration and its source definition.
        Language::Cpp | Language::C => format!("{ty} {name}"),
        Language::Swift => {
            if !value.is_empty() {
                format!("{name}: {ty} = {value}")
            } else {
                format!("{name}: {ty}")
            }
        }
        Language::Go => format!("{name} {ty}"),
        Language::Rust => {
            if !value.is_empty() {
                format!("{name}: {ty} = {value}")
            } else {
                format!("{name}: {ty}")
            }
        }
        Language::Java => {
            let ty_str = if ty.is_empty() { "Object" } else { ty };
            format!("{ty_str} {name}")
        }
    }
}
