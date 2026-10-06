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

use super::lexer::{Kind, Source};

/// Generic syntax copied to the builder declaration and adapted for its impl and type uses.
#[derive(Debug, Clone, Default)]
pub(crate) struct Generics {
    /// The declaration's `<...>`, including legal type and const defaults.
    pub(crate) declaration: String,
    /// The impl's `<...>`, with type and const defaults removed as Rust requires.
    pub(crate) impl_declaration: String,
    /// The original type's arguments, containing parameter names only.
    pub(crate) arguments: String,
}

pub(crate) struct GenericParameter {
    pub(crate) impl_parameter: String,
    pub(crate) argument: String,
}

/// Reads `<...>` and derives each spelling Rust needs without changing bounds or defaults.
pub(crate) fn read_generics(
    src: &Source<'_>,
    open: usize,
    owner: &str,
) -> Result<(Generics, usize)> {
    let close = close_angle(src, open)
        .with_context(|| format!("the generic parameters of `{owner}` are not closed"))?;
    validate_rebound_syntax(src, open + 1, close, owner, "generic parameters")?;
    let mut parameters = Vec::new();
    let mut from = open + 1;
    let mut brackets = 0i32;
    let mut angles = 0i32;
    for i in open + 1..=close {
        let split = if i == close {
            true
        } else {
            match src.t(i) {
                "(" | "[" | "{" if src.tokens[i].kind == Kind::Punct => brackets += 1,
                ")" | "]" | "}" if src.tokens[i].kind == Kind::Punct => brackets -= 1,
                "<" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles += 1,
                ">" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles -= 1,
                _ => {}
            }
            src.is(i, ",") && brackets == 0 && angles == 0
        };
        if split {
            if from == i {
                if i != close {
                    bail!(
                        "`{owner}` has an empty generic parameter on line {}",
                        src.line(i)
                    );
                }
            } else {
                parameters.push(read_generic_parameter(src, from, i, owner)?);
            }
            from = i + 1;
        }
    }
    if parameters.is_empty() {
        bail!("`{owner}` has an empty generic parameter list");
    }
    Ok((
        Generics {
            declaration: src.spell(open, close + 1),
            impl_declaration: format!(
                "<{}>",
                parameters
                    .iter()
                    .map(|p| p.impl_parameter.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            arguments: format!(
                "<{}>",
                parameters
                    .iter()
                    .map(|p| p.argument.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
        close,
    ))
}

pub(crate) fn read_generic_parameter(
    src: &Source<'_>,
    from: usize,
    to: usize,
    owner: &str,
) -> Result<GenericParameter> {
    if src.is(from, "#") {
        bail!(
            "`{owner}` has an attributed generic parameter on line {}; attributes on generic parameters are not supported",
            src.line(from)
        );
    }
    let top_level = |needle: &str| {
        let mut brackets = 0i32;
        let mut angles = 0i32;
        for i in from..to {
            if src.is(i, needle) && brackets == 0 && angles == 0 {
                return Some(i);
            }
            match src.t(i) {
                "(" | "[" | "{" if src.tokens[i].kind == Kind::Punct => brackets += 1,
                ")" | "]" | "}" if src.tokens[i].kind == Kind::Punct => brackets -= 1,
                "<" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles += 1,
                ">" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles -= 1,
                _ => {}
            }
        }
        None
    };
    let equals = top_level("=");
    let parameter_end = equals.unwrap_or(to);
    let argument = if src.tokens[from].kind == Kind::Lifetime {
        if equals.is_some() {
            bail!(
                "lifetime parameter `{}` of `{owner}` cannot have a default",
                src.t(from)
            );
        }
        if from + 1 < parameter_end && !src.is(from + 1, ":") {
            bail!(
                "cannot read lifetime parameter `{}` of `{owner}` on line {}",
                src.t(from),
                src.line(from)
            );
        }
        if src.is(from + 1, ":") && from + 2 >= parameter_end {
            bail!(
                "lifetime parameter `{}` of `{owner}` has no bound",
                src.t(from)
            );
        }
        src.t(from).to_string()
    } else if src.is(from, "const") {
        if from + 1 >= to || src.tokens[from + 1].kind != Kind::Ident {
            bail!(
                "cannot read a const parameter of `{owner}` on line {}",
                src.line(from)
            );
        }
        let colon = top_level(":").filter(|&i| i == from + 2).with_context(|| {
            format!(
                "cannot read const parameter `{}` of `{owner}`; expected `const NAME: TYPE`",
                src.t(from + 1)
            )
        })?;
        if colon + 1 >= parameter_end {
            bail!(
                "const parameter `{}` of `{owner}` has no type",
                src.t(from + 1)
            );
        }
        if let Some(eq) = equals {
            validate_const_default(src, eq + 1, to, owner, src.t(from + 1))?;
        }
        src.t(from + 1).to_string()
    } else if src.tokens[from].kind == Kind::Ident {
        if from + 1 < parameter_end && !src.is(from + 1, ":") {
            bail!(
                "cannot read type parameter `{}` of `{owner}` on line {}; expected a bound or default",
                src.t(from),
                src.line(from)
            );
        }
        if src.is(from + 1, ":") && from + 2 >= parameter_end {
            bail!("type parameter `{}` of `{owner}` has no bound", src.t(from));
        }
        src.t(from).to_string()
    } else {
        bail!(
            "cannot read a generic parameter of `{owner}` on line {}",
            src.line(from)
        );
    };
    Ok(GenericParameter {
        impl_parameter: src.spell(from, parameter_end),
        argument,
    })
}

fn validate_const_default(
    src: &Source<'_>,
    from: usize,
    to: usize,
    owner: &str,
    parameter: &str,
) -> Result<()> {
    let simple = is_literal_or_const_path(src, from, to);
    if !simple {
        bail!(
            "const parameter `{parameter}` of `{owner}` has an unsupported default expression; use a literal or const path"
        );
    }
    Ok(())
}

pub(crate) fn validate_rebound_syntax(
    src: &Source<'_>,
    from: usize,
    to: usize,
    owner: &str,
    place: &str,
) -> Result<()> {
    for i in from..to {
        if src.tokens[i].kind == Kind::Ident && src.t(i) == "Self" {
            bail!(
                "the {place} of `{owner}` spells `Self` (line {}), which would name the builder inside its impl; spell `{owner}` explicitly",
                src.line(i)
            );
        }
        if src.is(i, "!") && i > from && src.tokens[i - 1].kind == Kind::Ident {
            bail!(
                "the {place} of `{owner}` invokes a macro on line {}; macro-expanded generic syntax is not supported",
                src.line(i)
            );
        }
        if src.is(i, "{") {
            bail!(
                "the {place} of `{owner}` has an unsupported const expression on line {}; const blocks are not supported",
                src.line(i)
            );
        }
    }
    Ok(())
}

pub(crate) fn close_angle(src: &Source<'_>, open: usize) -> Option<usize> {
    let mut angles = 0i32;
    let mut brackets = 0i32;
    for i in open..src.tokens.len() {
        match src.t(i) {
            "(" | "[" | "{" if src.tokens[i].kind == Kind::Punct => brackets += 1,
            ")" | "]" | "}" if src.tokens[i].kind == Kind::Punct => brackets -= 1,
            "<" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles += 1,
            ">" if src.tokens[i].kind == Kind::Punct && brackets == 0 => {
                angles -= 1;
                if angles == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn struct_body_after_where(src: &Source<'_>, from: usize, owner: &str) -> Result<usize> {
    let mut angles = 0i32;
    let mut brackets = 0i32;
    for i in from + 1..src.tokens.len() {
        match src.t(i) {
            "{" if src.tokens[i].kind == Kind::Punct && angles == 0 && brackets == 0 => {
                return Ok(i);
            }
            "(" | "[" | "{" if src.tokens[i].kind == Kind::Punct => brackets += 1,
            ")" | "]" | "}" if src.tokens[i].kind == Kind::Punct => brackets -= 1,
            "<" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles += 1,
            ">" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles -= 1,
            ";" if angles == 0 && brackets == 0 => break,
            _ => {}
        }
    }
    bail!("cannot find the body after the `where` clause of `{owner}`")
}

pub(crate) fn is_literal_or_const_path(src: &Source<'_>, from: usize, to: usize) -> bool {
    if from + 1 == to && src.tokens[from].kind == Kind::Literal {
        return true;
    }
    let mut i = from;
    if src.is(i, "::") {
        i += 1;
    }
    if i >= to || src.tokens[i].kind != Kind::Ident {
        return false;
    }
    i += 1;
    while i < to {
        if !src.is(i, "::") || i + 1 >= to || src.tokens[i + 1].kind != Kind::Ident {
            return false;
        }
        i += 2;
    }
    true
}
