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

use crate::make_static::helpers::{
    display, is_ident, mentions, receiver_has_effects, split_receiver,
};
use crate::make_static::types::MadeStatic;

/// Makes the method declared at `line`:`col` of `file` an associated function.
pub async fn make_static(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    apply: bool,
    force: bool,
) -> Result<MadeStatic> {
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
        "the position is not the name of a method declaration"
    );
    let (name, open, close) =
        crate::signature::param_span(&text, start).context("the method has no parameter list")?;
    let (receiver, rest) = split_receiver(&text[open..close]).with_context(|| {
        format!("`{name}` takes no `self`: it is already an associated function")
    })?;
    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{name}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the method's body does not close")?;
    anyhow::ensure!(
        !mentions(&text[body_open..body_close], "self"),
        "`{name}` uses `self`; only a method that never does can lose its receiver"
    );
    let (owner, impl_at, impl_open, _) = crate::extract_field::impl_blocks(&text)
        .into_iter()
        .filter(|(_, _, o, c)| *o < start && start < *c)
        .min_by_key(|(_, _, o, c)| c - o)
        .context("the method is not inside an `impl` block")?;
    // A trait decides whether its methods take a receiver; an implementation cannot drop it.
    anyhow::ensure!(
        !text[impl_at..impl_open].contains(" for "),
        "`{name}` implements a trait method, and the trait decides whether it takes `self`; \
         change the trait instead"
    );

    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    edits
        .entry(file.to_path_buf())
        .or_default()
        .push((open, close - open, rest));
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let mut rewritten_calls = 0usize;
    let mut blocked = Vec::new();
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
        if !body[at..].starts_with(name.as_str()) {
            unmatched.push(format!(
                "{site} (the analyzer places `{name}` here, but the file says otherwise)"
            ));
            continue;
        }
        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(&body, at + name.len())
        else {
            unmatched.push(format!("{site} (not a call: the method used as a value)"));
            continue;
        };
        let before = body[..at].trim_end();
        if let Some(dot) = before.strip_suffix('.').map(|b| b.len()) {
            // `receiver.method(args)` → `Owner::method(args)`.
            let recv_start = crate::encapsulate_field::chain_start(&body, dot);
            let recv = body[recv_start..dot].to_string();
            if receiver_has_effects(&recv) {
                blocked.push(format!(
                    "{site} `{}` is evaluated for what it does",
                    recv.trim()
                ));
                // `force` says dropping it is intended: the call is still rewritten, or it
                // would call as a method what no longer takes `self` (#209).
                if !force {
                    continue;
                }
            }
            edits.entry(path.clone()).or_default().push((
                recv_start,
                at + name.len() - recv_start,
                format!("{owner}::{name}"),
            ));
        } else if before.ends_with("::") {
            // `Owner::method(receiver, args)` → `Owner::method(args)`.
            let args = crate::parameter_object::split_args(&body[args_start..args_end]);
            let Some(first) = args.first() else {
                unmatched.push(format!("{site} (a call with no receiver argument)"));
                continue;
            };
            if receiver_has_effects(first.trim_start_matches('&').trim_start_matches("mut ")) {
                blocked.push(format!(
                    "{site} `{}` is evaluated for what it does",
                    first.trim()
                ));
                if !force {
                    continue;
                }
            }
            let remaining: Vec<&str> = args[1..].iter().map(|a| a.trim()).collect();
            edits.entry(path.clone()).or_default().push((
                args_start,
                args_end - args_start,
                remaining.join(", "),
            ));
        } else {
            unmatched.push(format!("{site} (neither a method call nor a path call)"));
            continue;
        }
        rewritten_calls += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
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
        anyhow::ensure!(
            blocked.is_empty() || force,
            "{} call site(s) would drop a receiver that does something; nothing was written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
        );
        // `force` drops a receiver on purpose; it does not write past a reference this did not
        // rewrite (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{name}` were not rewritten, and would still pass a receiver; \
             nothing was written:\n  {}",
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

    Ok(MadeStatic {
        owner,
        method: name,
        root: root.to_path_buf(),
        file: display(root, file),
        receiver,
        rewritten_calls,
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
