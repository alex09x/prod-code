/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    Syntax,
    tokens::{pairs, tokens},
};
use anyhow::{Context, Result};
use std::{collections::BTreeMap, net::SocketAddr, ops::Range, path::Path};

/// Normalized spelling is only a candidate in C/C++. Ask clangd for declaration
/// identity, including parameters, typedef/auto declarators and nested scopes,
/// rather than guessing declarations from type keywords. Unknown is never equal
/// to unknown, including in single-selection mode. Literals need no resolution.
#[allow(clippy::too_many_arguments)]
pub(super) async fn preserve(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    syntax: Syntax,
    scope: Range<usize>,
    selected: usize,
    name: &str,
    hits: Vec<Range<usize>>,
) -> Result<Vec<Range<usize>>> {
    if !syntax.is_c_family() {
        return Ok(hits);
    }
    let hits =
        super::c_arrays::preserve(remote, root, file, text, syntax, scope.clone(), hits).await?;
    let hits =
        super::c_templates::preserve(remote, root, file, text, syntax, scope.clone(), hits).await?;
    let hits =
        super::c_constants::assertions(remote, root, file, text, syntax, scope.clone(), hits)
            .await?;
    anyhow::ensure!(
        hits.iter().any(|r| r.start == selected),
        "the selection is not a complete executable expression: this declaration requires constant operands"
    );
    // Include the signature: an existing parameter also collides. Qualified
    // members, C tags and statement labels use separate namespaces.
    // Any remaining occurrence needs scope proof we do not have; refuse before
    // writes instead of assuming an absent local binding proves no capture.
    let ts = tokens(&text[scope], syntax)?;
    let paired = pairs(&ts)?;
    anyhow::ensure!(
        !ts.iter()
            .enumerate()
            .any(|(i, t)| { t.text == name && !separate_name(&ts, &paired, i, syntax) }),
        "incomplete C/C++ binding coverage: possible declaration collision or capture by the new parameter; refused before writes"
    );
    let mut cache = BTreeMap::new();
    let mut identities = BTreeMap::new();
    for hit in &hits {
        let mut resolved = Vec::new();
        for token in tokens(&text[hit.clone()], syntax)? {
            if token.literal
                || !token.text.chars().next().is_some_and(super::tokens::ident)
                || grammar_keyword(token.text, syntax)
            {
                continue;
            }
            let at = hit.start + token.start;
            if let std::collections::btree_map::Entry::Vacant(entry) = cache.entry(at) {
                let (line, col) = crate::signature::position_at(text, at)?;
                let answer = crate::tools::execute_lsp_query(
                    remote,
                    root,
                    file,
                    "textDocument/definition",
                    serde_json::json!({
                        "textDocument": {"uri": url::Url::from_file_path(file)
                            .map_err(|_| anyhow::anyhow!("invalid source path"))?.to_string()},
                        "position": {"line": line - 1, "character": col - 1},
                    }),
                )
                .await
                .with_context(|| {
                    format!(
                        "incomplete C/C++ binding coverage for `{}`; refused before writes",
                        token.text
                    )
                })?;
                let mut locations = crate::refactor::lsp_locations(&answer, "binding definitions")?;
                locations.sort();
                locations.dedup();
                // `.h` is opened as C even when clangd compiles it as C++.
                // A C identifier named `true`, `not`, etc. must still resolve;
                // only an actual C++ grammar node can waive that requirement.
                if locations.is_empty()
                    && syntax == Syntax::C
                    && grammar_keyword(token.text, Syntax::Cpp)
                    && header_keyword(remote, root, file, text, at, token.text).await?
                {
                    continue;
                }
                anyhow::ensure!(
                    locations.len() == 1,
                    "incomplete C/C++ binding coverage for `{}` at {line}:{col}: expected one resolved declaration; refused before writes",
                    token.text
                );
                entry.insert(locations.remove(0));
            }
            resolved.push(cache[&at].clone());
        }
        identities.insert(hit.start, resolved);
    }
    let selected = &identities[&selected];
    Ok(hits
        .into_iter()
        .filter(|r| identities[&r.start] == *selected)
        .collect())
}

async fn header_keyword(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    at: usize,
    word: &str,
) -> Result<bool> {
    let (line, col) = crate::signature::position_at(text, at)?;
    let (end_line, end_col) = crate::signature::position_at(text, at + word.len())?;
    let ast = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/ast",
        serde_json::json!({
            "textDocument": {"uri": url::Url::from_file_path(file)
                .map_err(|_| anyhow::anyhow!("invalid source path"))?.to_string()},
            "range": {"start": {"line":line-1,"character":col-1},
                      "end": {"line":end_line-1,"character":end_col-1}},
        }),
    )
    .await
    .context("incomplete C/C++ header grammar coverage; refused before writes")?;
    let kind = ast["kind"].as_str().unwrap_or_default();
    let role = ast["role"].as_str().unwrap_or_default();
    let detail = ast["detail"].as_str().unwrap_or_default();
    // clangd returns the smallest node covering the token range. A DeclRef,
    // Typedef or unknown node is never proof that an unresolved name is syntax.
    Ok((role == "expression" && kind.starts_with("CXX"))
        || (role == "type" && kind == "Builtin" && detail == word)
        || (role == "expression"
            && matches!(
                kind,
                "UnaryOperator" | "BinaryOperator" | "CompoundAssignOperator"
            )
            && matches!(
                (word, detail),
                ("not", "!")
                    | ("compl", "~")
                    | ("and", "&&")
                    | ("or", "||")
                    | ("bitand", "&")
                    | ("bitor", "|")
                    | ("xor", "^")
                    | ("not_eq", "!=")
                    | ("and_eq", "&=")
                    | ("or_eq", "|=")
                    | ("xor_eq", "^=")
            )))
}

fn separate_name(
    ts: &[super::tokens::Token<'_>],
    pairs: &[Option<usize>],
    i: usize,
    syntax: Syntax,
) -> bool {
    let before = i.checked_sub(1).map(|n| ts[n].text);
    let after = ts.get(i + 1).map(|t| t.text);
    matches!(before, Some("." | "->" | "::" | "goto"))
        || (syntax == Syntax::C && matches!(before, Some("struct" | "union" | "enum")))
        || after == Some("::")
        || (after == Some(":")
            && statement_prefix(ts, pairs, i)
            && !super::regions::conditional_colon(ts, pairs, i + 1))
}

fn statement_prefix(ts: &[super::tokens::Token<'_>], pairs: &[Option<usize>], i: usize) -> bool {
    let Some(previous) = i.checked_sub(1) else {
        return true;
    };
    match ts[previous].text {
        "{" | "}" | ";" | ":" | "else" | "do" => true,
        ")" => pairs[previous]
            .and_then(|open| open.checked_sub(1))
            .is_some_and(|head| {
                matches!(ts[head].text, "if" | "while" | "for" | "switch")
                    || (ts[head].text == "constexpr" && head > 0 && ts[head - 1].text == "if")
            }),
        _ => false,
    }
}

/// Grammar words have no declaration location. This vocabulary only classifies
/// tokens inside an already recognized expression; it never guesses declarator
/// scopes or skips binding-bearing operands such as typedefs or library names.
fn grammar_keyword(word: &str, syntax: Syntax) -> bool {
    matches!(
        word,
        "auto"
            | "break"
            | "case"
            | "char"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extern"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "inline"
            | "int"
            | "long"
            | "register"
            | "return"
            | "short"
            | "signed"
            | "sizeof"
            | "static"
            | "struct"
            | "switch"
            | "typedef"
            | "union"
            | "unsigned"
            | "void"
            | "volatile"
            | "while"
    ) || (syntax == Syntax::C
        && matches!(
            word,
            "restrict"
                | "_Alignas"
                | "_Alignof"
                | "_Atomic"
                | "_Bool"
                | "_Complex"
                | "_Generic"
                | "_Imaginary"
                | "_Noreturn"
                | "_Static_assert"
                | "_Thread_local"
        ))
        || (syntax == Syntax::Cpp
            && matches!(
                word,
                "alignas"
                    | "alignof"
                    | "and"
                    | "and_eq"
                    | "asm"
                    | "bitand"
                    | "bitor"
                    | "bool"
                    | "catch"
                    | "char16_t"
                    | "char32_t"
                    | "class"
                    | "compl"
                    | "constexpr"
                    | "const_cast"
                    | "decltype"
                    | "delete"
                    | "dynamic_cast"
                    | "explicit"
                    | "export"
                    | "false"
                    | "friend"
                    | "mutable"
                    | "namespace"
                    | "new"
                    | "noexcept"
                    | "not"
                    | "not_eq"
                    | "nullptr"
                    | "operator"
                    | "or"
                    | "or_eq"
                    | "private"
                    | "protected"
                    | "public"
                    | "reinterpret_cast"
                    | "static_assert"
                    | "static_cast"
                    | "template"
                    | "this"
                    | "thread_local"
                    | "throw"
                    | "true"
                    | "try"
                    | "typeid"
                    | "typename"
                    | "using"
                    | "virtual"
                    | "wchar_t"
                    | "xor"
                    | "xor_eq"
            ))
}
