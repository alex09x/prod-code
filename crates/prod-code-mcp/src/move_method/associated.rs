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

use super::edits::{cut_from_impl, errors_of, insertion, item_span};
use super::syntax::{display, is_ident, swap_names};
use super::types::MovedMethod;

/// Moves the associated function (no `self`) whose name is at `line`:`col` of `file` into the
/// inherent `impl` of the type `to_type`: `Self` in it is spelled out as the old type, and every
/// path to it (`Order::f`, `Self::f`, called or used as a value) names the new type. Nothing is
/// written unless `apply`, nothing blocks it and the analyzer accepts the result, or `force`.
#[allow(clippy::too_many_arguments)]
pub async fn move_associated_function(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    to_type: &str,
    apply: bool,
    force: bool,
) -> Result<MovedMethod> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let root = &canon(root);
    let file = &canon(file);
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = crate::signature::offset_of(&text, line, col)
        .context("the position is not inside the file")?;
    let name_at = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    anyhow::ensure!(
        text[..name_at].trim_end().ends_with("fn"),
        "the position is not the name of a function declaration"
    );
    let (name, open, close) = crate::signature::param_span(&text, name_at)
        .context("the function has no parameter list")?;
    let (receiver, _) = crate::signature::parse_declared(&text[open..close]);
    anyhow::ensure!(
        receiver.is_none(),
        "`{name}` takes `self`; a method moves to the type of one of its parameters (`to_param`)"
    );
    let (owner, impl_at, impl_open, impl_close) = crate::extract_field::impl_blocks(&text)
        .into_iter()
        .filter(|(_, _, o, c)| *o < name_at && name_at < *c)
        .min_by_key(|(_, _, o, c)| c - o)
        .context("the function is not inside an `impl` block")?;
    let impl_header = &text[impl_at..impl_open];
    anyhow::ensure!(
        !impl_header.contains(" for "),
        "`{name}` implements a trait's function; it belongs to the trait, not to `{owner}`"
    );
    anyhow::ensure!(
        !impl_header
            .trim_start_matches("impl")
            .trim_start()
            .starts_with('<'),
        "`impl` blocks with generic parameters are not handled"
    );
    anyhow::ensure!(owner != to_type, "`{name}` is already in `{owner}`");

    // The type it goes to, by name.
    let hits = crate::tools::workspace_symbol_search(remote, root, to_type, Some(file), 50).await?;
    let found: Vec<_> = hits
        .into_iter()
        .filter(|h| h.name == to_type && matches!(h.kind, "Struct" | "Enum" | "Class"))
        .collect();
    let hit = match found.as_slice() {
        [one] => one,
        [] => anyhow::bail!("no struct or enum named `{to_type}` in the workspace"),
        many => anyhow::bail!(
            "`{to_type}` names {} types; the move does not guess: {}",
            many.len(),
            many.iter()
                .map(|h| h.render(root))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    };
    let target_file = canon(&hit.path);
    let def_line = hit.line;
    let (_, owner_module) = crate::move_item::module_of(file)?;
    let (_, target_module) = crate::move_item::module_of(&target_file)?;
    let owner_in_target = if owner_module.segments == target_module.segments {
        owner.clone()
    } else {
        format!(
            "{}::{owner}",
            owner_module.spelled_from(&target_module.krate)
        )
    };

    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{name}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the function's body does not close")?;
    let (item_start, span_end) = item_span(&text, name_at, body_close);
    let span_start = if text[..item_start].ends_with("\n\n") {
        item_start - 1
    } else {
        item_start
    };
    let map = [("Self", owner_in_target.as_str())];
    let method_text = format!(
        "{}{}\n",
        &text[item_start..name_at],
        swap_names(&text[name_at..=body_close], &map)
    );
    let signature = format!(
        "fn {}",
        swap_names(&text[name_at..body_open], &map).trim_end()
    );

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.clone(), text.clone());
    if !texts.contains_key(&target_file) {
        texts.insert(
            target_file.clone(),
            std::fs::read_to_string(&target_file)
                .with_context(|| format!("cannot read {}", target_file.display()))?,
        );
    }
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let (cut_start, cut_end) = cut_from_impl(
        &text,
        (impl_at, impl_open, impl_close),
        (span_start, span_end),
    );
    edits
        .entry(file.clone())
        .or_default()
        .push((cut_start, cut_end, String::new()));
    let (insert_at, inserted) = insertion(&texts[&target_file], to_type, def_line, &method_text)?;
    edits
        .entry(target_file.clone())
        .or_default()
        .push((insert_at, insert_at, inserted));

    // Every path to it names the new type.
    let mut unmatched = Vec::new();
    let mut calls = 0;
    let (nl, nc) = crate::signature::position_at(&text, name_at)?;
    let refs = crate::signature::references(remote, root, file, nl, nc)
        .await
        .with_context(|| format!("cannot find the paths to `{name}`; nothing was planned"))?;
    for (path, l, c) in refs {
        let path = canon(&path);
        let body = crate::refactor::referenced_text(&mut texts, &path)?.clone();
        let site = format!("{}:{l}", display(root, &path));
        let Some(at) = crate::signature::offset_of(&body, l, c) else {
            unmatched.push(format!(
                "{site}:{c}: the analyzer's position is not in the file"
            ));
            continue;
        };
        if !body[at..].starts_with(name.as_str()) || body[at + name.len()..].starts_with(is_ident) {
            unmatched.push(format!(
                "{site}:{c}: the analyzer places `{name}` here, but the file says otherwise"
            ));
            continue;
        }
        let before = body[..at].trim_end();
        let Some(qualifier) = before.strip_suffix("::") else {
            unmatched.push(format!("{site}: `{name}` is named without its type's path"));
            continue;
        };
        let path_start = qualifier
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_ident(*c) || *c == ':')
            .last()
            .map_or(qualifier.len(), |(i, _)| i);
        let (_, caller_module) = crate::move_item::module_of(&path)?;
        let target_path = if caller_module.segments == target_module.segments {
            to_type.to_string()
        } else {
            format!(
                "{}::{to_type}",
                target_module.spelled_from(&caller_module.krate)
            )
        };
        // Inside the moved function itself the path moves with it.
        if path == *file && span_start <= at && at < span_end {
            continue;
        }
        edits
            .entry(path.clone())
            .or_default()
            .push((path_start, at, format!("{target_path}::")));
        calls += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut t = texts[&path].clone();
        file_edits.sort_by_key(|(from, _, _)| std::cmp::Reverse(*from));
        for (from, to, replacement) in file_edits {
            t.replace_range(from..to, &replacement);
        }
        rewritten.insert(path, t);
    }
    let diagnostics = errors_of(remote, root, &rewritten).await?;
    let mut applied = false;
    // `force` overrides the analyzer; a path this did not rewrite still names the old type.
    if apply && unmatched.is_empty() && (diagnostics.is_empty() || force) {
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }
    Ok(MovedMethod {
        method: name,
        from_type: owner,
        to_type: to_type.to_string(),
        root: root.clone(),
        signature,
        calls,
        blocked: Vec::new(),
        unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
