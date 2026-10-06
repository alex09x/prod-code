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
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use super::syntax::{is_caller_independent, is_ident};
use super::types::{InlinedParameter, display};

/// Inlines the parameter at `line`:`col` of `file`.
#[allow(clippy::too_many_arguments)]
pub async fn inline_parameter(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    apply: bool,
    force: bool,
) -> Result<InlinedParameter> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let (fn_at, param, _) = crate::signature::parameter_at(&text, line, col)
        .context("the position is not on a parameter of a function declaration")?;
    let (function, open, close) =
        crate::signature::param_span(&text, fn_at).context("the function has no parameter list")?;
    let (receiver, declared) = crate::signature::parse_declared(&text[open..close]);
    let index = declared
        .iter()
        .position(|d| d.name == param)
        .context("the parameter is not in the list")?;
    let raw = declared[index].raw.clone();
    anyhow::ensure!(
        raw.contains(':'),
        "`{param}` has no type written; a `let` for it needs one"
    );
    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{function}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the function's body does not close")?;

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    let mut values: Vec<(String, String)> = Vec::new();
    let (fl, fc) = crate::signature::position_at(&text, fn_at)?;
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let refs = crate::signature::references(remote, root, file, fl, fc)
        .await
        .with_context(|| format!("cannot find the calls to `{function}`; nothing was planned"))?;
    for (path, l, c) in refs {
        let body = crate::refactor::referenced_text(&mut texts, &path)?.clone();
        let site = format!("{}:{l}:{c}", display(root, &path));
        let Some(at) = crate::signature::offset_of(&body, l, c) else {
            unmatched.push(format!("{site} (the position is not in the file)"));
            continue;
        };
        if !body[at..].starts_with(function.as_str())
            || body[at + function.len()..].starts_with(is_ident)
        {
            unmatched.push(format!(
                "{site} (the analyzer places `{function}` here, but the file says otherwise)"
            ));
            continue;
        }
        let same_file = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()) == canonical;
        if same_file && body_open < at && at < body_close {
            unmatched.push(format!(
                "{site} (a call inside `{function}` itself passes its own `{param}`)"
            ));
            continue;
        }
        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(&body, at + function.len())
        else {
            unmatched.push(format!(
                "{site} (the function used as a value: it would change type)"
            ));
            continue;
        };
        let args = crate::parameter_object::split_args(&body[args_start..args_end]);
        let method_syntax = body[..at].trim_end().ends_with('.');
        let arg_index = if receiver.is_some() && !method_syntax {
            index + 1
        } else {
            index
        };
        let Some(arg) = args.get(arg_index) else {
            unmatched.push(format!("{site} (the call has no argument for `{param}`)"));
            continue;
        };
        values.push((site, arg.trim().to_string()));
        let remaining: Vec<&str> = args
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != arg_index)
            .map(|(_, a)| a.trim())
            .collect();
        edits.entry(path.clone()).or_default().push((
            args_start,
            args_end - args_start,
            remaining.join(", "),
        ));
    }

    let first = values.first().map(|(_, v)| v.clone()).with_context(|| {
        format!("no call passes a value for `{param}`, so there is none to inline")
    })?;
    let differing: Vec<String> = values
        .iter()
        .filter(|(_, v)| *v != first)
        .map(|(site, v)| format!("{site} passes `{v}`"))
        .collect();
    anyhow::ensure!(
        differing.is_empty(),
        "the calls do not agree on `{param}`: {} of {} pass `{first}`, and\n  {}",
        values.len() - differing.len(),
        values.len(),
        differing.join("\n  ")
    );
    anyhow::ensure!(
        is_caller_independent(&first),
        "every call passes `{first}` for `{param}`, but it may name something of the caller's \
         (a local, or an expression over one); only a literal, a constant or a path is inlined"
    );

    // The declaration: without the parameter, and the value bound at the top of the body.
    let mut kept: Vec<String> = receiver.into_iter().collect();
    kept.extend(
        declared
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, d)| d.raw.clone()),
    );
    let first_line_indent = text[body_open + 1..]
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .unwrap_or(4);
    let own = edits.entry(file.to_path_buf()).or_default();
    own.push((open, close - open, kept.join(", ")));
    own.push((
        body_open + 1,
        0,
        format!("\n{}let {raw} = {first};", " ".repeat(first_line_indent)),
    ));

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }
    let rewritten_calls = values.len();

    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, &[]).await?;
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
    if apply {
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{function}` are not a call passing `{param}`; nothing was \
             written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `force: true` \
             to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(InlinedParameter {
        function,
        parameter: param,
        value: first,
        root: root.to_path_buf(),
        file: display(root, file),
        rewritten_calls,
        unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
