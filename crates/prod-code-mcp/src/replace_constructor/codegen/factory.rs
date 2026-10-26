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

#[cfg(test)]
mod tests;

/// Generates the factory code to add to the target file.
pub fn generate_factory_code(decl: &StructDecl, factory_name: &str) -> String {
    let name = &decl.name;
    match decl.language.as_str() {
        "rust" => {
            let vis = if decl.is_pub { "pub " } else { "" };
            let (parameters, where_clause) = rust_declaration_parts(decl);
            let (generics_header, generics_name) = rust_impl_generics(parameters);
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
                "\n\nimpl {generics_header}{name}{generics_name}{where_clause} {{\n    {vis}fn {factory_name}({params}) -> Self {{\n        Self {{\n            {field_inits},\n        }}\n    }}\n}}"
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
            // Forward the argument vector without relocating defaults or changing
            // arguments.length. A signature tuple also works for private and
            // protected constructors, unlike ConstructorParameters<typeof Class>.
            if decl
                .fields
                .iter()
                .any(|f| !f.vis.is_empty() || f.ty.is_empty())
            {
                let tuple = super::members::typescript_argument_tuple(decl);
                return format!(
                    "\n    static {factory_name}(...args: [{tuple}]): {name} {{\n        return new {name}(...args);\n    }}\n"
                );
            }
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
            if decl.fields.iter().any(|f| f.vis == "default") {
                return format!(
                    "\n    static {factory_name}(...args) {{\n        return new {name}(...args);\n    }}\n"
                );
            }
            let params = decl
                .fields
                .iter()
                .map(|f| {
                    if f.vis == "rest" {
                        format!("...{}", f.name)
                    } else {
                        f.name.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            let args = params.clone();
            format!(
                "\n    static {factory_name}({params}) {{\n        return new {name}({args});\n    }}\n"
            )
        }
        "python" => {
            if decl
                .fields
                .iter()
                .any(|f| f.vis == "default" || f.ty == "Any")
            {
                return format!(
                    "\n    @classmethod\n    def {factory_name}(cls, *args, **kwargs) -> \"{name}\":\n        return cls(*args, **kwargs)\n"
                );
            }
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

pub(super) fn rust_declaration_parts(decl: &StructDecl) -> (&str, String) {
    let generics = decl.generics.as_deref().unwrap_or("");
    let end = if generics.starts_with('<') {
        crate::extract_trait::generics::matching_angle(generics).map_or(0, |i| i + 1)
    } else {
        0
    };
    let where_clause = generics[end..].trim();
    (
        &generics[..end],
        if where_clause.is_empty() {
            String::new()
        } else {
            format!(" {where_clause}")
        },
    )
}

/// Split a Rust generic declaration into impl parameters and type arguments.
/// Defaults belong on the type declaration, not on an `impl` parameter list.
pub(crate) fn rust_impl_generics(generics: &str) -> (String, String) {
    let inner = generics
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(generics);
    let params = crate::extract_trait::generics::split_top_level(inner).unwrap_or_default();
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
            '>' if !param[..i].ends_with('-') => angle = angle.saturating_sub(1),
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
