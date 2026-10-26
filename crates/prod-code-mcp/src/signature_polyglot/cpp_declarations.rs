/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    declaration::{build_decl_edits, find_polyglot_declaration, format_polyglot_param},
    syntax::is_word_used,
    types::PolyglotDecl,
};
use crate::parameter_object::Language;
use crate::signature::{Modifiers, Param};
use anyhow::{Context, Result};

/// Called only for an analyzer-resolved declaration, including definitions reached
/// from a selected header. Local parameter names need not match the definition's.
#[allow(clippy::too_many_arguments)]
pub fn edits(
    content: &str,
    line: u32,
    col: u32,
    args_start: usize,
    selected: &PolyglotDecl,
    request: &[Param],
    modifiers: &Modifiers,
    lang: Language,
    force: bool,
) -> Result<Vec<(usize, usize, String)>> {
    let local = find_polyglot_declaration(content, lang, line, col)?;
    anyhow::ensure!(
        local.open_paren + 1 == args_start && local.params.len() == selected.params.len(),
        "unresolved declaration shape"
    );
    if !force {
        let body = &content[local.body_open..local.body_close];
        for (index, param) in local.params.iter().enumerate() {
            let selected_name = &selected.params[index].name;
            let retained = request.iter().any(|p| match p {
                Param::Keep(name) => name == selected_name,
                Param::Add { name, .. } => name == &param.name,
            });
            anyhow::ensure!(
                retained || !is_word_used(body, &param.name),
                "parameter `{}` is still used in the function body; pass force: true to override",
                param.name
            );
        }
    }
    let mut params = Vec::new();
    for param in request {
        params.push(match param {
            Param::Keep(name) => {
                let index = selected
                    .params
                    .iter()
                    .position(|p| p.name == *name)
                    .context("unknown selected parameter")?;
                local.params[index].raw.trim().to_string()
            }
            Param::Add { name, ty, value } => format_polyglot_param(name, ty, value, lang),
        });
    }
    Ok(build_decl_edits(
        content,
        &local,
        &params.join(", "),
        modifiers,
        lang,
    ))
}
