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
use super::rewrite::{CallRewriteOutcome, rewrite_call_site};
use super::syntax::{base_name, display, is_ident, receiver_as_type, snake_case, swap_names};
use super::types::MovedMethod;

/// Makes the method whose name is at `line`:`col` of `file` a method of the type of its
/// parameter `to_param`. Nothing is written unless `apply`, nothing blocks it and the analyzer
/// accepts the result, or `force`.
#[allow(clippy::too_many_arguments)]
pub async fn move_method(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    to_param: &str,
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
        "the position is not the name of a method declaration"
    );
    let (name, open, close) =
        crate::signature::param_span(&text, name_at).context("the method has no parameter list")?;
    let list = text[open..close].to_string();
    let (receiver, declared) = crate::signature::parse_declared(&list);
    let receiver = receiver.with_context(|| {
        format!("`{name}` takes no `self`; an associated function has no receiver to swap")
    })?;
    let (owner, impl_at, impl_open, impl_close) = crate::extract_field::impl_blocks(&text)
        .into_iter()
        .filter(|(_, _, o, c)| *o < name_at && name_at < *c)
        .min_by_key(|(_, _, o, c)| c - o)
        .context("the method is not inside an `impl` block")?;
    let impl_header = &text[impl_at..impl_open];
    anyhow::ensure!(
        !impl_header.contains(" for "),
        "`{name}` implements a trait method; it belongs to the trait, not to `{owner}`"
    );
    anyhow::ensure!(
        !impl_header
            .trim_start_matches("impl")
            .trim_start()
            .starts_with('<'),
        "`impl` blocks with generic parameters are not handled"
    );
    let index = declared
        .iter()
        .position(|d| d.name == to_param)
        .with_context(|| {
            format!(
                "`{name}` has no parameter `{to_param}`; it takes {}",
                declared
                    .iter()
                    .map(|d| d.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let target = &declared[index];
    let target_ty = crate::signature::split_params(&target.raw)
        .first()
        .and_then(|p| p.split_once(':').map(|(_, t)| t.trim().to_string()))
        .context("the parameter has no type")?;
    let target_name =
        base_name(target_ty.trim_start_matches('&').trim_start_matches("mut ")).to_string();
    anyhow::ensure!(
        target_name.chars().next().is_some_and(char::is_uppercase)
            && !target_ty.contains("impl ")
            && !target_ty.contains("dyn "),
        "`{to_param}: {target_ty}` is not a named type a method can move to"
    );
    let (new_receiver, _) = crate::to_method::receiver_for(&target.raw, &target_name)
        .with_context(|| format!("`{target_ty}` cannot become a receiver"))?;
    let (old_as_type, old_mut) = receiver_as_type(&receiver, "{owner}")
        .with_context(|| format!("the receiver `{receiver}` has a type of its own; not handled"))?;

    // The body, and names that would clash.
    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{name}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the method's body does not close")?;
    let body = &text[body_open..=body_close];
    let new_param = snake_case(&owner);
    anyhow::ensure!(
        !declared.iter().any(|d| d.name == new_param)
            && !body.contains(&format!("let {new_param}"))
            && !body.contains(&format!("let mut {new_param}")),
        "`{name}` already has a `{new_param}`, the name the old receiver would take"
    );

    // Where the target type is declared, and how each file spells the two types.
    let ty_offset = open
        + list
            .find(&format!("{to_param}:"))
            .map(|i| i + to_param.len() + 1)
            .unwrap_or(0);
    let ty_at = ty_offset
        + text[ty_offset..]
            .find(target_name.as_str())
            .context("the parameter's type is not in the list")?;
    let (tl, tc) = crate::signature::position_at(&text, ty_at)?;
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    let answer = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/definition",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": tl - 1, "character": tc - 1 },
        }),
    )
    .await
    .with_context(|| format!("the analyzer does not say where `{target_name}` is declared"))?;
    let def = answer
        .as_array()
        .and_then(|a| a.first())
        .or_else(|| answer.get("uri").map(|_| &answer))
        .context("the analyzer does not say where the type is declared")?;
    let def_uri = def
        .get("uri")
        .or_else(|| def.get("targetUri"))
        .and_then(|u| u.as_str())
        .context("the type's declaration has no file")?;
    let def_line = def
        .pointer("/range/start/line")
        .or_else(|| def.pointer("/targetSelectionRange/start/line"))
        .and_then(|v| v.as_u64())
        .context("the type's declaration has no position")? as u32
        + 1;
    let target_file = canon(Path::new(&crate::remote_fs::uri_to_path(def_uri)));
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

    // The method as it will read in its new home.
    let old_as_type = old_as_type.replace("{owner}", &owner_in_target);
    let binding = if old_mut {
        format!("mut {new_param}")
    } else {
        new_param.clone()
    };
    let mut params: Vec<String> = vec![new_receiver.clone()];
    for (i, d) in declared.iter().enumerate() {
        if i == index {
            params.push(format!("{binding}: {old_as_type}"));
        } else {
            params.push(d.raw.trim().to_string());
        }
    }
    // The body's uses of the parameter, as the analyzer resolves them: another binding of the
    // same name (`if let Some(tax)`, `for tax in`, a match arm) keeps its own uses (#207).
    let param_at = open
        + list
            .match_indices(to_param)
            .map(|(i, _)| i)
            .find(|i| {
                !list[..*i].chars().next_back().is_some_and(is_ident)
                    && !list[i + to_param.len()..]
                        .chars()
                        .next()
                        .is_some_and(is_ident)
            })
            .context("the parameter's name is not in the list")?;
    let (pl, pc) = crate::signature::position_at(&text, param_at)?;
    let uses = crate::signature::references(remote, root, file, pl, pc)
        .await
        .with_context(|| {
            format!(
                "cannot find the uses of `{to_param}`, which become `self`; nothing was planned"
            )
        })?;
    let mut param_uses: Vec<usize> = Vec::new();
    for (path, l, c) in uses {
        if canon(&path) != *file {
            continue;
        }
        let at = crate::signature::offset_of(&text, l, c).with_context(|| {
            format!(
                "the analyzer places a use of `{to_param}` at {}:{l}:{c}, which is not in the \
                 file; nothing was planned",
                display(root, file)
            )
        })?;
        if body_open <= at && at <= body_close {
            anyhow::ensure!(
                text[at..].starts_with(to_param)
                    && !text[at + to_param.len()..].starts_with(is_ident),
                "the analyzer places a use of `{to_param}` at {}:{l}:{c}, but the file says \
                 otherwise; nothing was planned",
                display(root, file)
            );
            param_uses.push(at - body_open);
        }
    }
    param_uses.sort_unstable();
    param_uses.dedup();
    let map = [
        ("self", new_param.as_str()),
        ("Self", owner_in_target.as_str()),
    ];
    let (item_start, span_end) = item_span(&text, name_at, body_close);
    let span_start = if text[..item_start].ends_with("\n\n") {
        item_start - 1
    } else {
        item_start
    };
    let head = &text[item_start..name_at];
    let generics = &text[name_at + name.len()..open - 1];
    let between = swap_names(&text[close + 1..body_open], &map);
    const MARK: &str = "\u{1}\u{2}\u{1}";
    let mut marked = body.to_string();
    for at in param_uses.iter().rev() {
        marked.replace_range(*at..*at + to_param.len(), MARK);
    }
    let new_body = swap_names(&marked, &map).replace(MARK, "self");
    let method_text = format!(
        "{head}{name}{generics}({}){between}{new_body}\n",
        params.join(", ")
    );
    let signature = format!(
        "fn {name}{generics}({}){}",
        params.join(", "),
        between.trim_end()
    );

    // Every file this touches, as edits against its text now.
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
    let target_text = texts[&target_file].clone();
    let (insert_at, inserted) = insertion(&target_text, &target_name, def_line, &method_text)?;
    edits
        .entry(target_file.clone())
        .or_default()
        .push((insert_at, insert_at, inserted));

    // The calls: the receiver and the argument swap places.
    let mut blocked = Vec::new();
    let mut unmatched = Vec::new();
    let mut calls = 0;
    let (nl, nc) = crate::signature::position_at(&text, name_at)?;
    let refs = crate::signature::references(remote, root, file, nl, nc)
        .await
        .with_context(|| format!("cannot find the calls to `{name}`; nothing was planned"))?;
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
        match rewrite_call_site(
            &body,
            at,
            &site,
            c,
            &name,
            &path,
            file,
            span_start,
            span_end,
            index,
            &receiver,
            &target_name,
            &target_module,
            force,
        )? {
            CallRewriteOutcome::Edit {
                call_start,
                close_paren,
                new_call,
                blocked: b,
            } => {
                if let Some(b) = b {
                    blocked.push(b);
                }
                edits
                    .entry(path.clone())
                    .or_default()
                    .push((call_start, close_paren, new_call));
                calls += 1;
            }
            CallRewriteOutcome::Blocked(b) => {
                blocked.push(b);
            }
            CallRewriteOutcome::Unmatched(u) => {
                unmatched.push(u);
            }
        }
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
    // `force` accepts the evaluation order and the analyzer's errors; a call this did not
    // rewrite would call a method that is gone.
    if apply && unmatched.is_empty() && ((blocked.is_empty() && diagnostics.is_empty()) || force) {
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }
    Ok(MovedMethod {
        method: name,
        from_type: owner,
        to_type: target_name,
        root: root.clone(),
        signature,
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
