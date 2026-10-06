/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::lexer::bare;
use super::parser::Declaration;

/// The builder, its error type and their impls, one line per element of the result joined with
/// `newline`, every line indented like the declaration.
pub(crate) fn render_code(decl: &Declaration, builder: &str, error: &str, newline: &str) -> String {
    let ty = &decl.name;
    let type_use = format!("{ty}{}", decl.generics.arguments);
    let builder_declaration = format!("{builder}{}", decl.generics.declaration);
    let builder_use = format!("{builder}{}", decl.generics.arguments);
    let where_suffix = if decl.where_clause.is_empty() {
        String::new()
    } else {
        format!(" {}", decl.where_clause)
    };
    let shown = bare(ty);
    let vis = if decl.visibility.is_empty() {
        String::new()
    } else {
        format!("{} ", decl.visibility)
    };
    let mut lines: Vec<String> = Vec::new();
    let mut push = |s: String| lines.push(s);
    push(format!(
        "/// Builds a `{shown}` one field at a time. Every field is required: `{builder}::build`"
    ));
    push("/// names the first one that was never set.".to_string());
    push("#[must_use]".to_string());
    let snake = decl
        .fields
        .iter()
        .any(|f| bare(&f.name).chars().any(char::is_uppercase));
    if snake {
        push("#[allow(non_snake_case)]".to_string());
    }
    push(format!(
        "{vis}struct {builder_declaration}{where_suffix} {{"
    ));
    for f in &decl.fields {
        push(format!("    {}: ::core::option::Option<{}>,", f.name, f.ty));
    }
    push("}".to_string());
    push(String::new());
    let mut allowed = vec![
        "clippy::new_without_default",
        "clippy::should_implement_trait",
        "clippy::wrong_self_convention",
    ];
    if snake {
        allowed.push("non_snake_case");
    }
    push(format!("#[allow({})]", allowed.join(", ")));
    let impl_prefix = if decl.generics.impl_declaration.is_empty() {
        "impl".to_string()
    } else {
        format!("impl{}", decl.generics.impl_declaration)
    };
    push(format!("{impl_prefix} {builder_use}{where_suffix} {{"));
    push("    /// A builder with no field set.".to_string());
    push(format!("    {vis}fn new() -> Self {{"));
    if decl.fields.is_empty() {
        push("        Self {}".to_string());
    } else {
        push("        Self {".to_string());
        for f in &decl.fields {
            push(format!(
                "            {}: ::core::option::Option::None,",
                f.name
            ));
        }
        push("        }".to_string());
    }
    push("    }".to_string());
    for f in &decl.fields {
        push(String::new());
        push(format!("    /// Sets `{}`.", bare(&f.name)));
        push(format!(
            "    {vis}fn {}(mut self, value: {}) -> Self {{",
            f.setter, f.ty
        ));
        push(format!(
            "        self.{} = ::core::option::Option::Some(value);",
            f.name
        ));
        push("        self".to_string());
        push("    }".to_string());
    }
    push(String::new());
    push(format!(
        "    /// The `{shown}`, or the first field in declaration order that was never set."
    ));
    push(format!(
        "    {vis}fn build(self) -> ::core::result::Result<{type_use}, {error}> {{"
    ));
    if decl.fields.is_empty() {
        push(format!("        ::core::result::Result::Ok({ty} {{}})"));
    } else {
        push(format!("        ::core::result::Result::Ok({ty} {{"));
        for f in &decl.fields {
            push(format!(
                "            {}: self.{}.ok_or({error} {{ field: \"{}\" }})?,",
                f.name,
                f.name,
                bare(&f.name)
            ));
        }
        push("        })".to_string());
    }
    push("    }".to_string());
    push("}".to_string());
    push(String::new());
    push(format!(
        "/// The error `{builder}::build` returns for a field that was never set."
    ));
    push("#[derive(::core::fmt::Debug, ::core::clone::Clone, ::core::marker::Copy, ::core::cmp::PartialEq, ::core::cmp::Eq)]".to_string());
    push(format!("{vis}struct {error} {{"));
    push("    field: &'static str,".to_string());
    push("}".to_string());
    push(String::new());
    push(format!("impl {error} {{"));
    push("    /// The field that was never set, as declared.".to_string());
    push(format!("    {vis}fn field(&self) -> &'static str {{"));
    push("        self.field".to_string());
    push("    }".to_string());
    push("}".to_string());
    push(String::new());
    push(format!("impl ::core::fmt::Display for {error} {{"));
    push(
        "    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {"
            .to_string(),
    );
    push(format!("        f.write_str(\"`{shown}` field `\")?;"));
    push("        f.write_str(self.field)?;".to_string());
    push("        f.write_str(\"` was never set\")".to_string());
    push("    }".to_string());
    push("}".to_string());
    push(String::new());
    push(format!("impl ::core::error::Error for {error} {{}}"));
    lines
        .into_iter()
        .map(|l| {
            if l.is_empty() {
                l
            } else {
                format!("{}{l}", decl.indent)
            }
        })
        .collect::<Vec<_>>()
        .join(newline)
}
