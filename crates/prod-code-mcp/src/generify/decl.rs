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

use super::types::PolyglotFuncDecl;

pub fn insert_generic_decl(
    new_text: &mut String,
    text: &str,
    decl: &PolyglotFuncDecl,
    type_param: &str,
    bound: &str,
    lang: Language,
) {
    match lang {
        Language::TypeScript | Language::JavaScript => {
            let bound_spec = if bound.is_empty() || bound == "any" {
                String::new()
            } else {
                format!(" extends {bound}")
            };
            let gen_decl = format!("{type_param}{bound_spec}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                new_text.insert_str(decl.name_end, &format!("<{gen_decl}>"));
            }
        }
        Language::Python => {
            let bound_spec = if bound.is_empty() || bound == "Any" || bound == "object" {
                String::new()
            } else {
                format!(": {bound}")
            };
            let gen_decl = format!("{type_param}{bound_spec}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                new_text.insert_str(decl.name_end, &format!("[{gen_decl}]"));
            }
        }
        Language::Swift => {
            let bound_spec = if bound.is_empty() || bound == "Any" {
                String::new()
            } else {
                format!(": {bound}")
            };
            let gen_decl = format!("{type_param}{bound_spec}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                new_text.insert_str(decl.name_end, &format!("<{gen_decl}>"));
            }
        }
        Language::Go => {
            let bound_spec = if bound.is_empty() { "any" } else { bound };
            let gen_decl = format!("{type_param} {bound_spec}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                new_text.insert_str(decl.name_end, &format!("[{gen_decl}]"));
            }
        }
        Language::Cpp | Language::C => {
            let concept_spec = if bound.is_empty() || bound == "typename" || bound == "class" {
                "typename"
            } else {
                bound
            };
            let gen_decl = format!("{concept_spec} {type_param}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                let line_start = text[..decl.decl_start].rfind('\n').map_or(0, |p| p + 1);
                let indent_len = text[line_start..].len() - text[line_start..].trim_start().len();
                let indent = &text[line_start..line_start + indent_len];
                new_text.insert_str(decl.decl_start, &format!("{indent}template<{gen_decl}>\n"));
            }
        }
        Language::Java => {
            let bound_spec = if bound.is_empty() || bound == "Object" {
                String::new()
            } else {
                format!(" extends {bound}")
            };
            let gen_decl = format!("{type_param}{bound_spec}");
            if decl.has_generics {
                if let Some((_, ge)) = decl.generics_span {
                    new_text.insert_str(ge, &format!(", {gen_decl}"));
                }
            } else {
                new_text.insert_str(decl.decl_start, &format!("<{gen_decl}> "));
            }
        }
        Language::Rust => unreachable!(),
    }
}
