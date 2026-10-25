/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Macro input is not an ordinary expression tree. Only resolved standard formatting
//! macros have a capture contract established here; other expansions remain uncertain.

use super::syntax::Syntax;
use anyhow::{Context, Result, ensure};
use std::{net::SocketAddr, path::Path};

pub(super) async fn preserve_macro_bindings(
    s: &Syntax<'_>,
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    block: usize,
    anchor: usize,
    name: &str,
) -> Result<()> {
    for (call, n) in s.nodes.iter().enumerate() {
        if n.span.start < anchor || !s.inside(call, block) {
            continue;
        }
        ensure!(
            !matches!(n.kind.as_str(), "ATTR" | "MACRO_RULES"),
            "cannot prove binding preservation across an attribute or macro definition"
        );
        if n.kind != "MACRO_CALL" {
            continue;
        }
        let path = s.child(call, "PATH").context("unknown macro path")?;
        let macro_name = s
            .nodes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, n)| (n.kind == "NAME_REF" && s.inside(i, path)).then_some(i))
            .context("unknown macro name")?;
        ensure!(
            [
                "print",
                "println",
                "eprint",
                "eprintln",
                "format",
                "format_args",
                "format_args_nl"
            ]
            .contains(&s.code(macro_name)),
            "macro-generated references have unproven binding semantics"
        );
        let (line, col) = crate::signature::position_at(s.text, s.nodes[macro_name].span.start)?;
        let uri = url::Url::from_file_path(file).map_err(|_| anyhow::anyhow!("invalid file"))?;
        let answer = crate::tools::execute_lsp_query(
            remote,
            root,
            file,
            "textDocument/definition",
            serde_json::json!({
                "textDocument": {"uri": uri.as_str()},
                "position": {"line":line - 1, "character":col - 1}
            }),
        )
        .await?;
        let entries: Vec<_> = match &answer {
            serde_json::Value::Array(a) => a.iter().collect(),
            serde_json::Value::Object(_) => vec![&answer],
            _ => Vec::new(),
        };
        let target = entries
            .first()
            .and_then(|loc| loc.get("targetUri").or_else(|| loc.get("uri"))?.as_str())
            .and_then(crate::signature::local_file);
        ensure!(
            entries.len() == 1
                && target.as_ref().is_some_and(|p| {
                    !p.starts_with(root)
                        && [
                            "lib/rustlib/src/rust/library/std/src/macros.rs",
                            "lib/rustlib/src/rust/library/core/src/macros/mod.rs",
                            "lib/rustlib/src/rust/library/alloc/src/macros.rs",
                        ]
                        .iter()
                        .any(|suffix| p.ends_with(suffix))
                }),
            "macro-generated references are unproven: formatting macro identity is unavailable or nonstandard"
        );
        for (i, token) in s
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, _)| s.inside(*i, call))
        {
            if token.kind == "BANG" {
                // The invocation's own `!` is a direct child. Nested invocations can
                // synthesize input and require a separate expansion/capture proof.
                ensure!(
                    token.parent == Some(call),
                    "nested macro-generated references have unproven binding semantics"
                );
            }
            if token.kind != "STRING" {
                continue;
            }
            let literal = s.code(i);
            // An overapproximation, not a format parser: every possible capture name
            // in an unescaped Rust string is spelled contiguously in its token. Raw
            // strings also preserve that spelling. Only accept escapes that produce
            // nonidentifier characters. Refuse decoding/NFC uncertainty explicitly.
            ensure!(
                name.is_ascii() && literal.is_ascii() && capture_spelling_preserved(literal),
                "macro string escapes or Unicode capture normalization leave binding preservation unproven"
            );
            ensure!(
                !literal.contains(name),
                "requested name `{name}` is not fresh: a formatting string may implicitly capture it"
            );
        }
    }
    Ok(())
}

fn capture_spelling_preserved(literal: &str) -> bool {
    if literal.starts_with('r') {
        return true;
    }
    let mut chars = literal.chars();
    while let Some(c) = chars.next() {
        if c == '\\'
            && !matches!(
                chars.next(),
                Some('n' | 'r' | 't' | '0' | '\\' | '"' | '\'')
            )
        {
            // Hex/Unicode escapes can synthesize identifier characters; continuation
            // escapes can join them. Neither is a proof of contiguous capture spelling.
            return false;
        }
    }
    true
}
