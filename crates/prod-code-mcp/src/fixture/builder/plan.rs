/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Result, bail};

use super::codegen::render_code;
use super::lexer::{Kind, Source, bare, is_plain_identifier};
use super::parser::read_declaration;
use super::types::{BuilderPlan, RESERVED_METHODS};
use super::verify::insert_after;

/// Plans the builder for the declaration of `type_name` that starts within `within` (1-based
/// lines, as the analyzer's outline gives them) in `file_text`. Pure: no analyzer, no disk.
pub fn plan(
    file_text: &str,
    type_name: &str,
    within: (u32, u32),
    builder_name: Option<&str>,
) -> Result<BuilderPlan> {
    let src = Source::new(file_text)?;
    let wanted = bare(type_name);
    let candidates: Vec<usize> = (1..src.tokens.len())
        .filter(|&i| {
            src.tokens[i].kind == Kind::Ident
                && bare(src.t(i)) == wanted
                && src.tokens[i - 1].kind == Kind::Ident
                && matches!(src.t(i - 1), "struct" | "enum" | "union")
                && (within.0..=within.1).contains(&src.line(i))
        })
        .collect();
    let name_at = match candidates.as_slice() {
        [one] => *one,
        [] => bail!("no `struct {type_name}` in lines {}-{}", within.0, within.1),
        _ => bail!(
            "more than one declaration of `{type_name}` in lines {}-{}",
            within.0,
            within.1
        ),
    };
    let decl = read_declaration(&src, name_at)?;
    let builder = match builder_name {
        Some(name) => {
            if !is_plain_identifier(name) {
                bail!("`{name}` is not an identifier a builder can be named");
            }
            name.to_string()
        }
        None => format!("{}Builder", bare(&decl.name)),
    };
    let error = format!("{builder}Error");
    if builder == bare(&decl.name) {
        bail!("the builder cannot have the struct's own name `{builder}`");
    }
    for name in [&builder, &error] {
        if let Some(i) = (0..src.tokens.len())
            .find(|&i| src.tokens[i].kind == Kind::Ident && bare(src.t(i)) == name.as_str())
        {
            bail!(
                "`{name}` already appears in this file at line {}; the builder would collide with it or change what it names. Pass another `builder_name`",
                src.line(i)
            );
        }
    }
    for field in &decl.fields {
        if RESERVED_METHODS.contains(&bare(&field.name)) {
            bail!(
                "field `{}` would need a setter called `{}`, which the builder already has for itself; rename the field before generating a builder",
                field.name,
                bare(&field.name)
            );
        }
    }
    let newline = if file_text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let code = render_code(&decl, &builder, &error, newline);
    let (file_text_after, first) = insert_after(file_text, decl.last_line, &code, newline);
    let code_len = code.split('\n').count() as u32;
    Ok(BuilderPlan {
        type_name: decl.name.clone(),
        builder_name: builder,
        error_name: error,
        visibility: decl.visibility.clone(),
        fields: decl.fields.clone(),
        code,
        file_text: file_text_after,
        declaration_lines: (decl.first_line, decl.last_line),
        insert_after_line: decl.last_line,
        code_lines: (first, first + code_len - 1),
        notes: decl.notes.clone(),
        indent: decl.indent.clone(),
        newline,
    })
}
