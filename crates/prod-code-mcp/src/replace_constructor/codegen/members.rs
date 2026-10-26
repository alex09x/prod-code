/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::StructDecl;

/// Allocate helpers against both public setters and their saved-field attributes.
pub(crate) fn fresh_member(decl: &StructDecl, preferred: &str) -> String {
    let mut candidate = preferred.to_string();
    let mut suffix = 0;
    while decl
        .fields
        .iter()
        .any(|f| candidate == f.name || candidate == format!("_{}", f.name))
    {
        suffix += 1;
        candidate = format!("{preferred}_{suffix}");
    }
    candidate
}

pub(crate) fn argument_helper(decl: &StructDecl) -> String {
    fresh_member(decl, "withArguments")
}

/// Defaults stay in the constructor. Only their argument-presence contract is
/// represented here; a default before a required parameter still needs a slot.
pub(crate) fn typescript_argument_tuple(decl: &StructDecl) -> String {
    decl.fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let ty = if field.ty.is_empty() {
                if field.vis == "rest" { "any[]" } else { "any" }
            } else {
                &field.ty
            };
            match field.vis.as_str() {
                "rest" => format!("...{}: {ty}", field.name),
                "default"
                    if decl.fields[index + 1..]
                        .iter()
                        .any(|later| later.vis.is_empty()) =>
                {
                    format!("{}: {ty} | undefined", field.name)
                }
                "default" | "optional" => format!("{}?: {ty}", field.name),
                _ => format!("{}: {ty}", field.name),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}
