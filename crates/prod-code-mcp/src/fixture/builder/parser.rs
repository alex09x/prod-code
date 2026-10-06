/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, bail};

use super::generics::{
    Generics, is_literal_or_const_path, read_generics, struct_body_after_where,
    validate_rebound_syntax,
};
use super::lexer::{Kind, Source, bare};
use super::types::{BuilderField, INERT_ATTRIBUTES};

/// A struct declaration as read from its tokens.
#[derive(Debug, Clone)]
pub(crate) struct Declaration {
    pub(crate) name: String,
    pub(crate) visibility: String,
    pub(crate) generics: Generics,
    pub(crate) where_clause: String,
    pub(crate) fields: Vec<BuilderField>,
    pub(crate) first_line: u32,
    pub(crate) last_line: u32,
    pub(crate) indent: String,
    pub(crate) notes: Vec<String>,
}

/// Reads the declaration whose name is token `name_at`, refusing every shape it cannot be sure of.
pub(crate) fn read_declaration(src: &Source<'_>, name_at: usize) -> Result<Declaration> {
    let name = src.t(name_at).to_string();
    match src.t(name_at - 1) {
        "struct" => {}
        "enum" => {
            bail!("`{name}` is an enum; a builder is generated only for a struct with named fields")
        }
        _ => {
            bail!("`{name}` is a union; a builder is generated only for a struct with named fields")
        }
    }
    // Backwards over the visibility and the outer attributes.
    let mut begin = name_at - 1;
    if begin > 0 && src.is(begin - 1, ")") {
        let open = src
            .open_of(begin - 1)
            .context("unbalanced parentheses before the declaration")?;
        if open == 0 || !src.is(open - 1, "pub") {
            bail!("cannot read what precedes `struct {name}`");
        }
        begin = open - 1;
    } else if begin > 0 && src.is(begin - 1, "pub") {
        begin -= 1;
    }
    let visibility = src.spell(begin, name_at - 1);
    let mut attributes = Vec::new();
    while begin >= 2 && src.is(begin - 1, "]") {
        let Some(open) = src.open_of(begin - 1) else {
            bail!("unbalanced brackets before `struct {name}`");
        };
        // `#![...]` belongs to the enclosing module.
        if open == 0 || !src.is(open - 1, "#") {
            break;
        }
        attributes.push((src.attribute_path(open + 1, begin - 1), src.line(open)));
        begin = open - 1;
    }
    if begin > 0 && !matches!(src.t(begin - 1), ";" | "{" | "}" | "]") {
        bail!(
            "cannot tell where the declaration of `{name}` starts: `{}` precedes it on line {}",
            src.t(begin - 1),
            src.line(begin - 1)
        );
    }
    let has_derive = attributes.iter().any(|(p, _)| p == "derive");
    let mut notes = Vec::new();
    for (path, line) in &attributes {
        if path == "cfg" || path == "cfg_attr" {
            bail!(
                "`{name}` carries `#[{path}(…)]` (line {line}): whether and how it is declared depends on the build configuration, which a generated builder cannot follow. Generate it for a declaration without `{path}`"
            );
        }
        let tool = ["rustfmt::", "clippy::", "diagnostic::"]
            .iter()
            .any(|t| path.starts_with(t));
        if INERT_ATTRIBUTES.contains(&path.as_str()) || tool {
            continue;
        }
        if !has_derive {
            bail!(
                "`#[{path}]` on `{name}` (line {line}) is not a built-in attribute and no derive declares it as a helper: it may be an attribute macro that rewrites the struct, and a builder read from the source could miss fields"
            );
        }
        notes.push(format!(
            "`#[{path}]` is taken for a derive helper; if it is an attribute macro that changes the fields, only verification can show it"
        ));
    }
    let mut next = name_at + 1;
    let generics = if src.is(next, "<") {
        let (generics, close) = read_generics(src, next, &name)?;
        next = close + 1;
        generics
    } else {
        Generics::default()
    };
    let (where_clause, open) = if src.is(next, "where") {
        let open = struct_body_after_where(src, next, &name)?;
        validate_rebound_syntax(src, next, open, &name, "where clause")?;
        (src.spell(next, open), open)
    } else {
        (String::new(), next)
    };
    if !src.is(open, "{") {
        match src.tokens.get(open).map(|_| src.t(open)) {
            Some("(") => bail!(
                "`{name}` is a tuple struct; a builder is generated only for a struct with named fields"
            ),
            Some(";") => bail!("`{name}` is a unit struct; there is nothing for a builder to set"),
            _ => bail!("cannot read the declaration of `{name}`"),
        }
    }
    let close = src
        .close_of(open)
        .with_context(|| format!("the body of `{name}` is not closed"))?;
    let mut fields: Vec<BuilderField> = Vec::new();
    let (mut from, mut depth, mut angle) = (open + 1, 0i32, 0i32);
    for i in open + 1..=close {
        let split = if i == close {
            true
        } else if src.tokens[i].kind == Kind::Punct {
            match src.t(i) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                "<" if depth == 0 => angle += 1,
                ">" if depth == 0 && angle > 0 => angle -= 1,
                _ => {}
            }
            src.is(i, ",") && depth == 0 && angle == 0
        } else {
            false
        };
        if split {
            if from < i {
                let field = read_field(src, from, i, &name)?;
                if fields.iter().any(|f| bare(&f.name) == bare(&field.name)) {
                    bail!("`{name}` declares `{}` twice", field.name);
                }
                fields.push(field);
            }
            from = i + 1;
        }
    }
    // The builder goes after the line the declaration ends on, so nothing else may be on it.
    let end = src.tokens[close].end;
    let line_end = src.text[end..]
        .find('\n')
        .map_or(src.text.len(), |p| end + p);
    let rest = src.text[end..line_end].trim();
    if !rest.is_empty() && !rest.starts_with("//") {
        bail!(
            "the declaration of `{name}` shares its last line ({}) with other code; put it on a line of its own first",
            src.line(close)
        );
    }
    let start = src.tokens[begin].start;
    let line_start = src.text[..start].rfind('\n').map_or(0, |p| p + 1);
    let indent: String = src.text[line_start..start]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    Ok(Declaration {
        name,
        visibility,
        generics,
        where_clause,
        fields,
        first_line: src.line(begin),
        last_line: src.line(close),
        indent,
        notes,
    })
}

/// One field: tokens `from..to` of the body, between commas.
pub(crate) fn read_field(
    src: &Source<'_>,
    from: usize,
    to: usize,
    owner: &str,
) -> Result<BuilderField> {
    let mut i = from;
    let mut attributes = Vec::new();
    while i < to && src.is(i, "#") {
        if !src.is(i + 1, "[") {
            bail!(
                "cannot read the attribute on line {} of `{owner}`",
                src.line(i)
            );
        }
        let close = src
            .close_of(i + 1)
            .filter(|&c| c < to)
            .with_context(|| format!("unbalanced attribute on line {}", src.line(i)))?;
        attributes.push(src.attribute_path(i + 2, close));
        i = close + 1;
    }
    if i < to && src.is(i, "pub") {
        i += 1;
        if i < to && src.is(i, "(") {
            i = src
                .close_of(i)
                .filter(|&c| c < to)
                .with_context(|| format!("unbalanced visibility on line {}", src.line(i)))?
                + 1;
        }
    }
    if i < to && src.is(i, "unsafe") {
        bail!(
            "`{owner}` has an `unsafe` field (line {}); a safe setter cannot be generated for it",
            src.line(i)
        );
    }
    if i >= to || src.tokens[i].kind != Kind::Ident || i + 1 >= to || !src.is(i + 1, ":") {
        bail!(
            "cannot read a field of `{owner}` on line {}: expected `name: Type`",
            src.line(i.min(to - 1))
        );
    }
    let name = src.t(i).to_string();
    for path in &attributes {
        if path == "cfg" || path == "cfg_attr" {
            bail!(
                "field `{name}` of `{owner}` carries `#[{path}(…)]` (line {}): whether it exists depends on the build configuration, which a generated builder cannot follow",
                src.line(i)
            );
        }
    }
    let ty_from = i + 2;
    if ty_from >= to {
        bail!("field `{name}` of `{owner}` has no type");
    }
    let mut depth = 0i32;
    for j in ty_from..to {
        match src.t(j) {
            "(" | "[" | "{" if src.tokens[j].kind == Kind::Punct => depth += 1,
            ")" | "]" | "}" if src.tokens[j].kind == Kind::Punct => depth -= 1,
            "=" if src.tokens[j].kind == Kind::Punct && depth == 0 => bail!(
                "field `{name}` of `{owner}` has a default value (line {}); a builder that requires every field would not honour it",
                src.line(j)
            ),
            "Self" if src.tokens[j].kind == Kind::Ident => bail!(
                "the type of field `{name}` of `{owner}` spells `Self` (line {}), which names the builder inside it; spell the struct's name instead",
                src.line(j)
            ),
            _ => {}
        }
    }
    validate_field_type(src, ty_from, to, owner, &name)?;
    Ok(BuilderField {
        setter: name.clone(),
        ty: src.spell(ty_from, to),
        name,
    })
}

fn validate_field_type(
    src: &Source<'_>,
    from: usize,
    to: usize,
    owner: &str,
    field: &str,
) -> Result<()> {
    for i in from..to {
        if src.is(i, "!") && i > from && src.tokens[i - 1].kind == Kind::Ident {
            bail!(
                "the type of field `{field}` of `{owner}` invokes a macro on line {}; macro-expanded field types are not supported",
                src.line(i)
            );
        }
        if src.is(i, ";") {
            let mut end = i + 1;
            while end < to && !src.is(end, "]") {
                end += 1;
            }
            let simple = is_literal_or_const_path(src, i + 1, end);
            if !simple {
                bail!(
                    "the type of field `{field}` of `{owner}` has an unsupported const expression on line {}; array lengths must be a literal or const path",
                    src.line(i)
                );
            }
        }
    }
    Ok(())
}
