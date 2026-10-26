/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::{ReplaceMode, StructDecl};
use std::collections::BTreeSet;

pub(crate) fn partial_build_method(decl: &StructDecl, arity: usize) -> String {
    super::members::fresh_member(decl, &format!("build_with_{arity}_arguments"))
}

/// Generate only arities present in the original program. The aggregate still
/// initializes omitted members itself, including any default member initializers.
/// Generating unused arities would reject aggregates with required references.
pub(crate) fn add_partial_members(
    code: &mut String,
    decl: &StructDecl,
    mode: ReplaceMode,
    target: &str,
    arities: &BTreeSet<usize>,
) {
    let mut methods = String::new();
    for &arity in arities {
        let fields = &decl.fields[..arity];
        let name = &decl.name;
        match mode {
            ReplaceMode::Factory => {
                let params = fields
                    .iter()
                    .map(|f| format!("{} {}", f.ty, f.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                let args = fields
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                methods.push_str(&format!("\n    static {name} {target}({params}) {{\n        return {name}{{{args}}};\n    }}\n"));
            }
            ReplaceMode::Builder => {
                let method = partial_build_method(decl, arity);
                let args = fields
                    .iter()
                    .map(|f| format!("{}_", f.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                methods.push_str(&format!(
                    "\n    {name} {method}() {{\n        return {name}{{{args}}};\n    }}\n"
                ));
            }
        }
    }
    match mode {
        ReplaceMode::Factory => code.push_str(&methods),
        ReplaceMode::Builder => {
            let end = code
                .rfind("};")
                .expect("generated C++ builder closing brace");
            code.insert_str(end, &methods);
        }
    }
}
