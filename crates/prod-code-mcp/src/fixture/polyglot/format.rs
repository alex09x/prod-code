/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::mock::generate_mock;
use super::samples::sample_value_for_type;
use super::types::{PolyglotShape, lower_camel_case, snake_case};
use crate::parameter_object::Language;

/// Formats a complete value fixture expression and snippet for `type_name`.
pub fn format_polyglot_fixture(
    language: Language,
    type_name: &str,
    shape: &PolyglotShape,
    randomized: bool,
    mock: bool,
) -> (String, String) {
    if mock {
        let (methods, fields) = match shape {
            PolyglotShape::Interface { methods } => (methods.as_slice(), [].as_slice()),
            PolyglotShape::InterfaceWithFields { methods, fields } => {
                (methods.as_slice(), fields.as_slice())
            }
            PolyglotShape::Record(fields) => ([].as_slice(), fields.as_slice()),
            _ => ([].as_slice(), [].as_slice()),
        };
        let code = generate_mock(language, type_name, methods, fields);
        return (code.clone(), code);
    }

    match shape {
        PolyglotShape::Record(fields) => {
            let mut pairs = Vec::new();
            for (f, ty) in fields {
                let val = sample_value_for_type(language, ty, Some(f), randomized);
                pairs.push((f.clone(), val));
            }

            match language {
                Language::Go => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    {f}: {v},"));
                    }
                    let val = format!("{type_name}{{\n{}\n}}", lines.join("\n"));
                    let var_name = lower_camel_case(type_name);
                    let snippet = format!("var {var_name} = {val}");
                    (val, snippet)
                }
                Language::TypeScript | Language::JavaScript => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    {f}: {v},"));
                    }
                    let val = format!("{{\n{}\n}}", lines.join("\n"));
                    let var_name = lower_camel_case(type_name);
                    let snippet = format!("const {var_name}: {type_name} = {val};");
                    (val, snippet)
                }
                Language::Python => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    {f}={v},"));
                    }
                    let val = format!("{type_name}(\n{}\n)", lines.join("\n"));
                    let var_name = snake_case(type_name);
                    let snippet = format!("{var_name} = {val}");
                    (val, snippet)
                }
                Language::Rust => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    {f}: {v},"));
                    }
                    let val = format!("{type_name} {{\n{}\n}}", lines.join("\n"));
                    let var_name = snake_case(type_name);
                    let snippet = format!("let {var_name} = {val};");
                    (val, snippet)
                }
                Language::Cpp | Language::C => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    .{f} = {v},"));
                    }
                    let val = format!("{type_name}{{\n{}\n}}", lines.join("\n"));
                    let var_name = snake_case(type_name);
                    let snippet = format!("auto {var_name} = {val};");
                    (val, snippet)
                }
                Language::Swift => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    {f}: {v},"));
                    }
                    let val = format!("{type_name}(\n{}\n)", lines.join("\n"));
                    let var_name = lower_camel_case(type_name);
                    let snippet = format!("let {var_name} = {val}");
                    (val, snippet)
                }
                Language::Java => {
                    let mut lines = Vec::new();
                    for (_, v) in &pairs {
                        lines.push(format!("    {v}"));
                    }
                    let val = format!("new {type_name}({})", lines.join(", "));
                    let var_name = lower_camel_case(type_name);
                    let snippet = format!("{type_name} {var_name} = {val};");
                    (val, snippet)
                }
            }
        }
        PolyglotShape::Tuple(types) => {
            let parts: Vec<String> = types
                .iter()
                .map(|t| sample_value_for_type(language, t, None, randomized))
                .collect();
            let val = format!("{type_name}({})", parts.join(", "));
            let var_name = snake_case(type_name);
            (val.clone(), format!("let {var_name} = {val};"))
        }
        PolyglotShape::Unit => {
            let val = type_name.to_string();
            let var_name = snake_case(type_name);
            (val.clone(), format!("let {var_name} = {val};"))
        }
        PolyglotShape::Enum(variants) => {
            let chosen = if randomized && variants.len() > 1 {
                &variants[1]
            } else {
                variants.first().map(String::as_str).unwrap_or("Default")
            };
            let val = match language {
                Language::Rust => format!("{type_name}::{chosen}"),
                Language::Swift => format!("{type_name}.{chosen}"),
                Language::TypeScript => format!("{type_name}.{chosen}"),
                Language::Go => (*chosen).to_string(),
                _ => format!("{type_name}.{chosen}"),
            };
            let var_name = snake_case(type_name);
            (val.clone(), format!("let {var_name} = {val};"))
        }
        PolyglotShape::Interface { methods } => {
            let code = generate_mock(language, type_name, methods, &[]);
            (code.clone(), code)
        }
        PolyglotShape::InterfaceWithFields { methods, fields } => {
            let code = generate_mock(language, type_name, methods, fields);
            (code.clone(), code)
        }
    }
}
