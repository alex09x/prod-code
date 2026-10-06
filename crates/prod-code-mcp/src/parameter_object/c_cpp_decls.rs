/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::container::body_open;
use super::params::parse_params;
use super::types::{CDeclaration, Language};

pub(crate) fn collect_c_declarations(
    spots: &[(PathBuf, u32, u32)],
    calls: &[(PathBuf, u32, u32)],
    callee: &str,
    texts: &mut BTreeMap<PathBuf, String>,
    initial_language: Language,
) -> (Vec<CDeclaration>, Language) {
    let mut decls: Vec<CDeclaration> = Vec::new();
    for (path, l, c) in spots {
        if !texts.contains_key(path) {
            let Ok(t) = std::fs::read_to_string(path) else {
                continue;
            };
            texts.insert(path.clone(), t);
        }
        let t = &texts[path];
        let Some(at) = crate::signature::offset_of(t, *l, *c) else {
            continue;
        };
        if !t[at..].starts_with(callee) {
            continue;
        }
        let Some((n, open, close)) = crate::signature::param_span(t, at) else {
            continue;
        };
        if n != callee || decls.iter().any(|d| d.path == *path && d.open == open) {
            continue;
        }
        decls.push(CDeclaration {
            path: path.clone(),
            text: t.clone(),
            name_at: at,
            open,
            close,
            params: Vec::new(),
            body: false,
        });
    }

    // A `.h` header is C by its name; the function is C++ when anything around it is.
    let language = if initial_language == Language::Cpp
        || decls
            .iter()
            .map(|d| &d.path)
            .chain(calls.iter().map(|(p, _, _)| p))
            .any(|p| Language::of(p) == Some(Language::Cpp))
    {
        Language::Cpp
    } else {
        Language::C
    };
    for d in &mut decls {
        d.body = body_open(&d.text, d.close, language).is_some();
        d.params = parse_params(&d.text[d.open..d.close], language).1;
    }
    (decls, language)
}
