/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::syntax::*;
use super::types::*;

/// Removes the parameter `index` (after the receiver) of the trait method whose name is at
/// `fn_at` of `file` (in the trait or in an implementation of it), from every declaration and
/// every call. Nothing is written unless `apply` and nothing blocks it and the analyzer accepts
/// the result, or `force`.
pub async fn remove_parameter(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    fn_at: usize,
    index: usize,
    apply: bool,
    force: bool,
) -> Result<TraitParameter> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let root = &canon(root);
    let file = &canon(file);
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    texts.insert(file.clone(), text.clone());
    let method: String = text[fn_at..].chars().take_while(|c| is_ident(*c)).collect();
    anyhow::ensure!(!method.is_empty(), "no method name at that position");

    // The trait's own declaration of the method.
    let (trait_file, trait_at, trait_name) = match owner_of(&text, fn_at)
        .context("the method is not in a trait or a trait implementation")?
    {
        Owner::Trait { name } => (file.clone(), fn_at, name),
        Owner::Impl { trait_at } => {
            let (line, col) = crate::signature::position_at(&text, trait_at)?;
            let answer = crate::tools::execute_lsp_query(
                remote,
                root,
                file,
                "textDocument/definition",
                position(file, line, col)?,
            )
            .await
            .context("the analyzer does not say where the trait is declared")?;
            let (tfile, tline, tcol) = crate::refactor::lsp_locations(&answer, "definition")
                .context("the analyzer does not say where the trait is declared")?
                .into_iter()
                .next()
                .context("the analyzer does not say where the trait is declared")?;
            let tfile = canon(&tfile);
            let ttext = std::fs::read_to_string(&tfile)
                .with_context(|| format!("cannot read {}", tfile.display()))?;
            let at = crate::signature::offset_of(&ttext, tline, tcol)
                .context("the trait's position is not in its file")?;
            let name: String = ttext[at..].chars().take_while(|c| is_ident(*c)).collect();
            let fn_at = method_in_block(&ttext, at, &method)
                .with_context(|| format!("the trait `{name}` does not declare `fn {method}`"))?;
            texts.insert(tfile.clone(), ttext);
            (tfile, fn_at, name)
        }
    };
    let (tl, tc) = crate::signature::position_at(&texts[&trait_file], trait_at)?;

    // Every implementation, and every reference: calls, and the implementations' names. An
    // implementation the answer does not place would keep the parameter the trait lost (#446).
    let impls = crate::tools::execute_lsp_query(
        remote,
        root,
        &trait_file,
        "textDocument/implementation",
        position(&trait_file, tl, tc)?,
    )
    .await
    .and_then(|answer| crate::refactor::lsp_locations(&answer, "implementations"))
    .with_context(|| {
        format!("cannot find the implementations of `{method}`; nothing was planned")
    })?;
    let refs = crate::signature::references(remote, root, &trait_file, tl, tc)
        .await
        .with_context(|| format!("cannot find the calls to `{method}`; nothing was planned"))?;
    let mut declarations: Vec<(PathBuf, u32, u32)> = vec![(trait_file.clone(), tl, tc)];
    declarations.extend(impls.into_iter().map(|(p, l, c)| (canon(&p), l, c)));
    for (path, _, _) in &declarations {
        if !texts.contains_key(path) {
            texts.insert(
                path.clone(),
                std::fs::read_to_string(path)
                    .with_context(|| format!("cannot read {}", path.display()))?,
            );
        }
    }

    let mut cuts: BTreeMap<PathBuf, Vec<(usize, usize)>> = BTreeMap::new();
    let mut blocked = Vec::new();
    let mut unmatched = Vec::new();
    let mut declared_lines = Vec::new();
    let mut parameter = String::new();
    let mut has_receiver = false;
    for (path, line, col) in &declarations {
        let text = &texts[path];
        let shown = format!("{}:{line}", display(root, path));
        let at = crate::signature::offset_of(text, *line, *col).with_context(|| {
            format!("{shown}:{col}, a declaration of `{method}`, is not in its file; nothing was planned")
        })?;
        // A stale position would take the parameter out of whatever function follows it.
        anyhow::ensure!(
            text[at..].starts_with(method.as_str())
                && !text[at + method.len()..].starts_with(is_ident),
            "the analyzer places a declaration of `{method}` at {shown}:{col}, but the file says \
             otherwise; nothing was planned"
        );
        let (_, open, close) = crate::signature::param_span(text, at)
            .with_context(|| format!("{shown}: no parameter list after the name"))?;
        let list = &text[open..close];
        let (receiver, declared) = crate::signature::parse_declared(list);
        has_receiver = receiver.is_some();
        let Some(removed) = declared.get(index) else {
            unmatched.push(format!(
                "{shown}: declares {} parameter(s), not {}",
                declared.len(),
                index + 1
            ));
            continue;
        };
        if path == &trait_file && at == trait_at {
            parameter = removed.name.clone();
        }
        declared_lines.push(format!("{shown} ({})", removed.name));
        // The body, when there is one, must not use it.
        let after = &text[close..];
        if let Some(brace) = after.find(['{', ';'])
            && after.as_bytes()[brace] == b'{'
            && let Some(end) = crate::parameter_object::matching_bracket(text, close + brace)
        {
            let name = removed.name.as_str();
            if !name.starts_with('_') && mentions(&text[close + brace..end], name) {
                blocked.push(format!(
                    "{shown}: the body uses `{name}`; remove that use first"
                ));
            }
        }
        let offset = usize::from(receiver.is_some());
        let spans = item_spans(list);
        let Some((from, to)) = removal(&spans, index + offset) else {
            unmatched.push(format!(
                "{shown}: the parameter list does not split into its parameters"
            ));
            continue;
        };
        cuts.entry(path.clone())
            .or_default()
            .push((open + from, open + to));
    }

    let mut calls = 0;
    for (path, line, col) in refs {
        let path = canon(&path);
        if declarations
            .iter()
            .any(|(p, l, c)| *p == path && *l == line && *c == col)
        {
            continue;
        }
        let text = &*crate::refactor::referenced_text(&mut texts, &path)?;
        let shown = format!("{}:{line}", display(root, &path));
        let Some(at) = crate::signature::offset_of(text, line, col) else {
            unmatched.push(format!(
                "{shown}:{col}: the analyzer's position is not in the file"
            ));
            continue;
        };
        if !text[at..].starts_with(method.as_str())
            || text[at + method.len()..].starts_with(is_ident)
        {
            unmatched.push(format!(
                "{shown}:{col}: the analyzer places `{method}` here, but the file says otherwise"
            ));
            continue;
        }
        let name_end = at + method.len();
        let mut rest_at = name_end + (text[name_end..].len() - text[name_end..].trim_start().len());
        if text[rest_at..].starts_with("::<")
            && let Some(close) = {
                let lt = rest_at + 2;
                let mut depth = 0i32;
                text[lt..].char_indices().find_map(|(i, c)| {
                    match c {
                        '<' => depth += 1,
                        '>' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(lt + i);
                            }
                        }
                        _ => {}
                    }
                    None
                })
            }
        {
            rest_at = close + 1;
        }
        if !text[rest_at..].starts_with('(') {
            unmatched.push(format!(
                "{shown}: `{method}` is used as a value, not called; what calls it passes the \
                 argument"
            ));
            continue;
        }
        let Some(close) = crate::parameter_object::matching_bracket(text, rest_at) else {
            unmatched.push(format!("{shown}: the call's arguments do not close"));
            continue;
        };
        let args = &text[rest_at + 1..close];
        let spans = item_spans(args);
        let method_call = text[..at].trim_end().ends_with('.');
        let arg_index = index + usize::from(!method_call && has_receiver);
        let Some((from, to)) = spans.get(arg_index).copied() else {
            unmatched.push(format!(
                "{shown}: the call passes {} argument(s), fewer than the declaration",
                spans.len()
            ));
            continue;
        };
        let arg = &args[from..to];
        if let Some(effect) = effect_of(arg)
            && !force
        {
            blocked.push(format!(
                "{shown}: the argument `{arg}` {effect}; removing it would drop that (pass \
                 `force` to remove it anyway)"
            ));
        }
        if let Some((cut_from, cut_to)) = removal(&spans, arg_index) {
            cuts.entry(path.clone())
                .or_default()
                .push((rest_at + 1 + cut_from, rest_at + 1 + cut_to));
        }
        calls += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut ranges) in cuts {
        let mut text = texts[&path].clone();
        ranges.sort_by_key(|r| std::cmp::Reverse(r.0));
        ranges.dedup();
        for (from, to) in ranges {
            text.replace_range(from..to, "");
        }
        rewritten.insert(path, text);
    }

    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &[]).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect();

    let mut applied = false;
    // `force` drops a used parameter or an argument's effect on purpose; a call or declaration
    // this did not rewrite would still pass or declare it.
    if apply && unmatched.is_empty() && ((blocked.is_empty() && diagnostics.is_empty()) || force) {
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }
    Ok(TraitParameter {
        method: format!("{trait_name}::{method}"),
        parameter,
        index,
        root: root.clone(),
        declarations: declared_lines,
        calls,
        blocked,
        unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
