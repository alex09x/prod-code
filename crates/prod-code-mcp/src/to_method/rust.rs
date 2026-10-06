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

use super::helpers::{display, is_ident, path_start, receiver_for, receiver_of};
use super::types::MadeMethod;

/// Makes the associated function declared at `line`:`col` of `file` a method of its type.
pub async fn convert_to_method(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    apply: bool,
    force: bool,
) -> Result<MadeMethod> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = crate::signature::offset_of(&text, line, col)
        .context("the position is not inside the file")?;
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    anyhow::ensure!(
        text[..start].trim_end().ends_with("fn"),
        "the position is not the name of a function declaration"
    );
    let (name, open, close) =
        crate::signature::param_span(&text, start).context("the function has no parameter list")?;
    anyhow::ensure!(
        crate::make_static::split_receiver(&text[open..close]).is_none(),
        "`{name}` already takes `self`"
    );
    let (owner, impl_at, impl_open, _) = crate::extract_field::impl_blocks(&text)
        .into_iter()
        .filter(|(_, _, o, c)| *o < start && start < *c)
        .min_by_key(|(_, _, o, c)| c - o)
        .with_context(|| {
            format!(
                "`{name}` is not inside an `impl` block; a free function has to be moved into one \
                 before it can become a method"
            )
        })?;
    anyhow::ensure!(
        !text[impl_at..impl_open].contains(" for "),
        "`{name}` implements a trait function, and the trait decides whether it takes `self`; \
         change the trait instead"
    );
    let params = crate::signature::split_params(&text[open..close]);
    let first = params
        .first()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .with_context(|| format!("`{name}` takes no parameters, so nothing can become `self`"))?;
    let (receiver, binding) = receiver_for(&first, &owner).with_context(|| {
        format!(
            "the first parameter of `{name}`, `{first}`, is not `{owner}`, `&{owner}` or `&mut \
             {owner}`, so it cannot become the receiver"
        )
    })?;
    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{name}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the function's body does not close")?;

    // The declaration: the first parameter becomes the receiver.
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let rest: Vec<String> = params[1..].iter().map(|p| p.trim().to_string()).collect();
    let new_params = std::iter::once(receiver.clone())
        .chain(rest)
        .collect::<Vec<_>>()
        .join(", ");
    edits
        .entry(file.to_path_buf())
        .or_default()
        .push((open, close - open, new_params));

    // The body: every use of the parameter, as the analyzer resolves it, becomes `self`.
    let first_at = open + text[open..close].find(first.as_str()).unwrap_or(0);
    let binding_at = first_at
        + first
            .find(binding.as_str())
            .context("the parameter's name is not in its declaration")?;
    let (bl, bc) = crate::signature::position_at(&text, binding_at)?;
    let mut renamed_uses = 0usize;
    let uses = crate::signature::references(remote, root, file, bl, bc)
        .await
        .with_context(|| {
            format!("cannot find the uses of `{binding}`, which become `self`; nothing was planned")
        })?;
    for (path, l, c) in uses {
        if path != file {
            continue;
        }
        // A use left behind names a parameter that no longer exists (#446).
        let use_at = crate::signature::offset_of(&text, l, c).with_context(|| {
            format!(
                "the analyzer places a use of `{binding}` at {}:{l}:{c}, which is not in the \
                 file; nothing was planned",
                display(root, file)
            )
        })?;
        if !(body_open < use_at && use_at < body_close) {
            continue;
        }
        anyhow::ensure!(
            text[use_at..].starts_with(&binding)
                && !text[use_at + binding.len()..].starts_with(is_ident),
            "the analyzer places a use of `{binding}` at {}:{l}:{c}, but the file says otherwise; \
             nothing was planned",
            display(root, file)
        );
        edits.entry(file.to_path_buf()).or_default().push((
            use_at,
            binding.len(),
            "self".to_string(),
        ));
        renamed_uses += 1;
    }

    // The calls: `Owner::name(first, rest)` → `first.name(rest)`.
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let mut rewritten_calls = 0usize;
    let mut unchanged = Vec::new();
    let mut unmatched = Vec::new();
    let (nl, nc) = crate::signature::position_at(&text, start)?;
    let refs = crate::signature::references(remote, root, file, nl, nc)
        .await
        .with_context(|| format!("cannot find the calls to `{name}`; nothing was planned"))?;
    for (path, l, c) in refs {
        let body = crate::refactor::referenced_text(&mut texts, &path)?.clone();
        let site = format!("{}:{l}:{c}", display(root, &path));
        let Some(at) = crate::signature::offset_of(&body, l, c) else {
            unmatched.push(format!("{site} (the position is not in the file)"));
            continue;
        };
        if !body[at..].starts_with(name.as_str()) || body[at + name.len()..].starts_with(is_ident) {
            unmatched.push(format!(
                "{site} (the analyzer places `{name}` here, but the file says otherwise)"
            ));
            continue;
        }
        if path == file && body_open < at && at < body_close {
            unchanged.push(format!(
                "{site} (a call inside `{name}` itself: `{owner}::{name}(self, …)` stays valid)"
            ));
            continue;
        }
        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(&body, at + name.len())
        else {
            unchanged.push(format!(
                "{site} (the function used as a value: a method is still reachable by its path)"
            ));
            continue;
        };
        let from = path_start(&body, at);
        if from == at {
            unchanged.push(format!("{site} (a call without a path in front)"));
            continue;
        }
        let args = crate::parameter_object::split_args(&body[args_start..args_end]);
        let Some(first_arg) = args.first().filter(|a| !a.trim().is_empty()) else {
            unchanged.push(format!("{site} (a call with no first argument)"));
            continue;
        };
        let remaining: Vec<&str> = args[1..].iter().map(|a| a.trim()).collect();
        edits.entry(path.clone()).or_default().push((
            from,
            args_end + 1 - from,
            format!(
                "{}.{name}({})",
                receiver_of(first_arg),
                remaining.join(", ")
            ),
        ));
        rewritten_calls += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts
            .get(&path)
            .cloned()
            .unwrap_or_else(|| std::fs::read_to_string(&path).unwrap_or_default());
        file_edits.sort_by_key(|(at, _, _)| *at);
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }

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
        // `force` overrides the analyzer, not a position this could not read (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{name}` could not be read; nothing was written:\n  {}",
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

    Ok(MadeMethod {
        owner,
        method: name,
        root: root.to_path_buf(),
        file: display(root, file),
        parameter: first,
        receiver,
        renamed_uses,
        rewritten_calls,
        unchanged,
        unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
