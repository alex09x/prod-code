/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::mask::lexical_code_mask;
use super::types::ReplacementCandidate;
use crate::parameter_object::Language;

mod cpp;
mod go;
mod python;
mod rust;
mod swift;
mod ts;

pub(crate) use cpp::scan_cpp;
pub(crate) use go::scan_go;
pub(crate) use python::scan_python;
pub(crate) use rust::scan_rust;
pub(crate) use swift::scan_swift;
pub(crate) use ts::scan_ts;

/// Finds all candidate parameter/variable migrations in a source text.
pub(crate) fn find_caller_migrations(
    text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
) -> Vec<ReplacementCandidate> {
    let mask = lexical_code_mask(text, lang);
    let mut candidates = Vec::new();

    match lang {
        Language::TypeScript | Language::JavaScript => {
            scan_ts(
                text,
                type_name,
                interface_name,
                extracted_methods,
                lang,
                &mask,
                &mut candidates,
            );
        }
        Language::Python => {
            scan_python(
                text,
                type_name,
                interface_name,
                extracted_methods,
                lang,
                &mask,
                &mut candidates,
            );
        }
        Language::Go => {
            scan_go(
                text,
                type_name,
                interface_name,
                extracted_methods,
                lang,
                &mask,
                &mut candidates,
            );
        }
        Language::Cpp | Language::C => {
            scan_cpp(
                text,
                type_name,
                interface_name,
                extracted_methods,
                lang,
                &mask,
                &mut candidates,
            );
        }
        Language::Swift => {
            scan_swift(
                text,
                type_name,
                interface_name,
                extracted_methods,
                lang,
                &mask,
                &mut candidates,
            );
        }
        Language::Rust => {
            scan_rust(
                text,
                type_name,
                interface_name,
                extracted_methods,
                lang,
                &mask,
                &mut candidates,
            );
        }
        Language::Java => {}
    }

    candidates
}
