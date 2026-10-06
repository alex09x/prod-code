/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::parse::split_balanced_commas;
use super::super::types::StructDecl;

/// Generates the factory code to add to the target file.
pub fn generate_factory_code(decl: &StructDecl, factory_name: &str) -> String {
    let name = &decl.name;
    match decl.language.as_str() {
        "rust" => {
            let vis = if decl.is_pub { "pub " } else { "" };
            let (generics_header, generics_name) = decl
                .generics
                .as_deref()
                .map(rust_impl_generics)
                .unwrap_or_default();
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join(", ");
            let field_inits = decl
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
                .join(",\n            ");
            format!(
                "\n\nimpl {generics_header}{name}{generics_name} {{\n    {vis}fn {factory_name}({params}) -> Self {{\n        Self {{\n            {field_inits},\n        }}\n    }}\n}}"
            )
        }
        "go" => {
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{} {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join(", ");
            let field_inits = decl
                .fields
                .iter()
                .map(|f| format!("{}: {},", f.name, f.name))
                .collect::<Vec<_>>()
                .join("\n        ");
            format!(
                "\n\nfunc {factory_name}({params}) *{name} {{\n    return &{name}{{\n        {field_inits}\n    }}\n}}"
            )
        }
        "typescript" | "typescriptreact" => {
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join(", ");
            let args = decl
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\n    static {factory_name}({params}): {name} {{\n        return new {name}({args});\n    }}\n"
            )
        }
        "javascript" | "javascriptreact" => {
            let params = decl
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let args = params.clone();
            format!(
                "\n    static {factory_name}({params}) {{\n        return new {name}({args});\n    }}\n"
            )
        }
        "python" => {
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join(", ");
            let args = decl
                .fields
                .iter()
                .map(|f| format!("{}={}", f.name, f.name))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\n    @classmethod\n    def {factory_name}(cls, {params}) -> \"{name}\":\n        return cls({args})\n"
            )
        }
        "cpp" | "c" => {
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{} {}", f.ty, f.name))
                .collect::<Vec<_>>()
                .join(", ");
            let args = decl
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\n    static {name} {factory_name}({params}) {{\n        return {name}{{{args}}};\n    }}\n"
            )
        }
        "swift" => {
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join(", ");
            let args = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.name))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\n    static func {factory_name}({params}) -> {name} {{\n        return {name}({args})\n    }}\n"
            )
        }
        _ => String::new(),
    }
}

/// Split a Rust generic declaration into impl parameters and type arguments.
/// Defaults belong on the type declaration, not on an `impl` parameter list.
pub(crate) fn rust_impl_generics(generics: &str) -> (String, String) {
    let inner = generics
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(generics);
    let params = split_balanced_commas(inner);
    let mut impl_params = Vec::new();
    let mut args = Vec::new();
    for param in params {
        let param = param.trim();
        if param.is_empty() {
            continue;
        }
        let without_default = strip_rust_generic_default(param).trim();
        let name = without_default
            .strip_prefix("const ")
            .unwrap_or(without_default)
            .split(|c: char| c == ':' || c.is_whitespace())
            .next()
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            continue;
        }
        impl_params.push(without_default.to_string());
        args.push(name.to_string());
    }
    if impl_params.is_empty() {
        (String::new(), String::new())
    } else {
        (
            format!("<{}> ", impl_params.join(", ")),
            format!("<{}>", args.join(", ")),
        )
    }
}

pub(crate) fn strip_rust_generic_default(param: &str) -> &str {
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;
    let mut brace = 0usize;
    for (i, ch) in param.char_indices() {
        match ch {
            '<' => angle += 1,
            '>' => angle = angle.saturating_sub(1),
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            '{' => brace += 1,
            '}' => brace = brace.saturating_sub(1),
            '=' if angle == 0 && paren == 0 && bracket == 0 && brace == 0 => {
                return &param[..i];
            }
            _ => {}
        }
    }
    param
}
