/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use serde_json::json;
use std::{net::SocketAddr, ops::Range, path::Path};

use crate::fixture::builder::lexer::{Kind, Source, bare};

/// Check declaration scope before planning edits. Macro input is token data;
/// inspect the analyzer's expansion at this parameter, not at the outer impl.
pub(super) async fn ensure_receiver_identity(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    param: Range<usize>,
    generics: &str,
) -> Result<()> {
    let generics = crate::signature::blank_comments(generics)
        .context("the method generic parameters have unclosed comments")?;
    let generics = generics.trim();
    if generics.is_empty() {
        return Ok(());
    }
    let generics = generics
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .context("cannot determine the method generic parameters")?;
    let mut type_names = Vec::new();
    let mut const_names = Vec::new();
    for param in crate::signature::split_params(generics) {
        let source = Source::new(&param)?;
        let mut at = 0;
        while source.is(at, "#") && source.is(at + 1, "[") {
            at = source
                .close_of(at + 1)
                .context("unclosed generic attribute")?
                + 1;
        }
        let is_const = source.is(at, "const");
        if is_const {
            at += 1;
        }
        if let Some(token) = source.tokens.get(at) {
            if token.kind != Kind::Ident {
                continue;
            }
            let name = bare(source.t(at)).to_owned();
            if is_const {
                const_names.push(name);
            } else {
                type_names.push(name);
            }
        }
    }
    if type_names.is_empty() && const_names.is_empty() {
        return Ok(());
    }
    let colon = text[param.clone()]
        .find(':')
        .context("the parameter has no type")?;
    let ty_at = param.start + colon + 1;
    let ty = Source::new(&text[ty_at..param.end])?;
    let macros = macro_ranges(&ty)?;
    ensure_no_shadow(&ty, &type_names, &macros, "type")?;
    ensure_no_shadow(&ty, &const_names, &macros, "const")?;
    for range in macros {
        let bang = (range.start..range.end).find(|&i| ty.is(i, "!")).unwrap();
        let (line, col) = crate::signature::position_at(text, ty_at + ty.tokens[bang - 1].start)?;
        let expansion = crate::tools::execute_lsp_query(
            remote,
            root,
            file,
            "rust-analyzer/expandMacro",
            json!({
                "textDocument": { "uri": url::Url::from_file_path(file).ok().context("invalid file path")?.to_string() },
                "position": { "line": line - 1, "character": col - 1 }
            }),
        )
        .await
        .context("cannot establish receiver identity without the macro expansion")?;
        let expanded = expansion["expansion"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .context("cannot establish receiver identity without the macro expansion")?;
        // expandMacro recursively expands nested invocations. Check the resulting
        // type independently of the invocation's macro-namespace path qualifiers.
        let expanded = Source::new(expanded)?;
        ensure_no_shadow(&expanded, &const_names, &[], "const")?;
        if ensure_no_shadow(&expanded, &type_names, &[], "type").is_err() {
            let values = super::expansion::value_names(remote, root, file, &expanded).await?;
            ensure_no_shadow(&expanded, &type_names, &values, "type")?;
        }
    }
    Ok(())
}

/// Opaque token trees, including their macro-namespace path, are not type paths.
fn macro_ranges(source: &Source<'_>) -> Result<Vec<Range<usize>>> {
    let mut ranges = Vec::new();
    let mut i = 0;
    while i + 2 < source.tokens.len() {
        if source.tokens[i].kind == Kind::Ident && source.is(i + 1, "!") {
            let close = source
                .close_of(i + 2)
                .context("unclosed receiver type macro")?;
            let mut start = i;
            while start >= 2
                && source.is(start - 1, "::")
                && source.tokens[start - 2].kind == Kind::Ident
            {
                start -= 2;
            }
            if start > 0 && source.is(start - 1, "::") {
                start -= 1;
            }
            ranges.push(start..close + 1);
            i = close + 1;
        } else {
            i += 1;
        }
    }
    Ok(ranges)
}

fn ensure_no_shadow(
    source: &Source<'_>,
    names: &[String],
    opaque: &[Range<usize>],
    kind: &str,
) -> Result<()> {
    for name in names {
        let shadowed = source.tokens.iter().enumerate().any(|(i, token)| {
            !opaque.iter().any(|range| range.contains(&i))
                && token.kind == Kind::Ident
                && bare(source.t(i)) == name
                && (i == 0 || !source.is(i - 1, "::"))
                && !source.is(i + 1, "!")
                // Labels and associated-type binding names introduce names.
                && !source.is(i + 1, ":")
                && !source.is(i + 1, "=")
        });
        anyhow::ensure!(
            !shadowed,
            "method {kind} parameter `{name}` shadows the impl self type; it cannot become `self`"
        );
    }
    Ok(())
}
