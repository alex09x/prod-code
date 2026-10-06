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

use crate::signature::call_sites::call_sites;
use crate::signature::effects::{effect_hazards, param_facts};
use crate::signature::modifiers::{
    check_dropped_parameters, in_async_fn, with_async, with_return_type, with_visibility,
};
use crate::signature::parse::{offset_of, param_span, parse_declared};
use crate::signature::plan::{call_site_rule, format_list, plan, reorders_to_itself};
use crate::signature::references::{
    locate_declaration, references, structural_replace, whole_file_edit,
};
use crate::signature::rewrite::attribute;
use crate::signature::types::{Modifiers, Param, Plan, SignatureChange};
use crate::signature::util::{display, normalize, read_caller};

/// Changes the parameter list of the function at `file:line:col`.
#[allow(clippy::too_many_arguments)]
pub async fn change(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    request: &[Param],
    apply: bool,
    force: bool,
) -> Result<SignatureChange> {
    change_with(
        remote,
        root,
        file,
        line,
        col,
        request,
        &Modifiers::default(),
        apply,
        force,
    )
    .await
}

/// [`change`], with the return type and the visibility changed in the same edit.
///
/// A `.go` file goes to [`crate::signature_go::change_with`], which reorders named parameters
/// through gopls and refuses everything else; it uses this module's helpers but never this
/// function, so the dispatch is one step deep.
#[allow(clippy::too_many_arguments)]
pub async fn change_with(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    request: &[Param],
    modifiers: &Modifiers,
    apply: bool,
    force: bool,
) -> Result<SignatureChange> {
    if file.extension().is_some_and(|e| e == "go") {
        return crate::signature_go::change_with(
            remote, root, file, line, col, request, modifiers, apply, force,
        )
        .await;
    }
    let ext = file.extension().and_then(|s| s.to_str()).unwrap_or("");
    if matches!(
        ext,
        "ts" | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "cpp"
            | "cc"
            | "cxx"
            | "c"
            | "h"
            | "hpp"
            | "hxx"
            | "swift"
    ) {
        return crate::signature_polyglot::change_with(
            remote, root, file, line, col, request, modifiers, apply, force,
        )
        .await;
    }
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset =
        offset_of(&text, line, col).context("the declaration is not at the resolved position")?;
    let (name, open, close) = param_span(&text, offset)
        .with_context(|| format!("no function declaration at {}:{line}:{col}", file.display()))?;
    let old_inner = text[open..close].to_string();
    let (receiver, declared) = parse_declared(&old_inner);
    let Plan {
        list: new_params,
        args,
        dropped,
    } = plan(&declared, request)?;

    // A parameter the body still uses cannot just disappear. The analyzer knows where a local
    // is used; the caller gets the list instead of a file that no longer compiles.
    check_dropped_parameters(
        remote, root, file, &declared, &dropped, open, close, &text, force,
    )
    .await?;

    let adds: Vec<&str> = request
        .iter()
        .filter_map(|p| match p {
            Param::Add { value, .. } => Some(value.as_str()),
            Param::Keep(_) => None,
        })
        .collect();
    let order_changed =
        args.len() != declared.len() || args.iter().enumerate().any(|(i, a)| *a != Some(i));
    let head_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let was_async = text[head_start..offset]
        .split_whitespace()
        .any(|w| w == "async");
    let async_wanted = modifiers.asyncness.filter(|w| *w != was_async);
    anyhow::ensure!(
        async_wanted.is_none() || !order_changed,
        "change `async` and the parameter list in two steps"
    );

    // Every reference to the function, asked once and before anything is rewritten: the
    // effects check, the `.await`s and the reconciliation all stand on it. A question that
    // fails stops the change, forced or not — an empty list would read as "no callers".
    let refs = references(remote, root, file, line, col)
        .await
        .with_context(|| format!("cannot list the references to `{name}`; nothing was written"))?;

    // What the call sites will do, not only how they are written. `force` writes a change that
    // does not compile, which the author then sees; it does not write one that compiles and
    // quietly runs differently.
    if order_changed {
        let calls = call_sites(root, &refs)?;
        let facts = param_facts(remote, root, file, &text, open, close, &declared).await;
        let hazards = effect_hazards(&name, &declared, &facts, receiver.is_some(), &args, &calls);
        anyhow::ensure!(
            hazards.is_empty(),
            "the new parameter list would change what the program does, or it cannot be shown \
             that it does not; nothing was written, and `force` does not override this:\n  {}\n\
             bind such an argument to a local of the parameter's own type before the call \
             (`let v: T = …;`) and pass the local, keep owned parameters and references in their \
             order, or remove them in a step of their own",
            hazards.join("\n  ")
        );
    }

    // Call sites first, while the declaration still has the arity the rule matches.
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut rule = String::new();
    if order_changed {
        rule = call_site_rule(&name, receiver.is_some(), declared.len(), &args, &adds);
        let edit = structural_replace(remote, root, file, &rule).await?;
        for (path, new_text) in crate::tools::rewritten_files(&edit) {
            rewritten.insert(PathBuf::from(path), new_text);
        }
    }

    // `async` in or out: every call the analyzer knows gains or loses its `.await`.
    let mut asyncness_change = None;
    let mut not_async = Vec::new();
    if let Some(want) = async_wanted {
        asyncness_change = Some((was_async, want));
        let mut edits: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for (path, rl, rc) in &refs {
            let t = match rewritten.get(path) {
                Some(t) => t.clone(),
                None => read_caller(path)?,
            };
            let Some(end) = offset_of(&t, *rl, *rc)
                .and_then(|at| param_span(&t, at))
                .map(|(_, _, close)| close + 1)
            else {
                continue; // not a call: the function used as a value
            };
            if want != t[end..].starts_with(".await") {
                edits.entry(path.clone()).or_default().push(end);
                if want && !in_async_fn(&t, end) {
                    not_async.push(format!("{}:{rl}:{rc}", display(root, path)));
                }
            }
        }
        for (path, mut ends) in edits {
            let mut t = match rewritten.get(&path) {
                Some(t) => t.clone(),
                None => read_caller(&path)?,
            };
            ends.sort_unstable();
            for end in ends.into_iter().rev() {
                if want {
                    t.insert_str(end, ".await");
                } else {
                    t.replace_range(end..end + ".await".len(), "");
                }
            }
            rewritten.insert(path, t);
        }
    }

    // Reconcile what the analyzer knows is a reference against what the rewrite of the calls
    // did, before the declaration's own edit joins it. Only a change that rewrites calls has
    // anything to reconcile: a new return type or visibility leaves every reference as it is,
    // and its callers are type-checked below instead.
    let mut unmatched = Vec::new();
    let mut unexpected = Vec::new();
    if order_changed || async_wanted.is_some() {
        let mut by_file: BTreeMap<PathBuf, Vec<(u32, u32)>> = BTreeMap::new();
        for (path, l, c) in &refs {
            by_file.entry(path.clone()).or_default().push((*l, *c));
        }
        for path in rewritten.keys() {
            by_file.entry(path.clone()).or_default();
        }
        let needs = |call: &[String], awaited: bool| match async_wanted {
            Some(want) => awaited != want,
            None => !reorders_to_itself(call, &args),
        };
        for (path, spots) in &mut by_file {
            spots.sort_unstable();
            spots.dedup();
            let old = if path == file {
                text.clone()
            } else {
                read_caller(path)?
            };
            let new = rewritten.get(path).unwrap_or(&old);
            let (u, x) = attribute(&display(root, path), &old, new, &name, spots, &needs);
            unmatched.extend(u);
            unexpected.extend(x);
        }
    }

    // Then the declaration, on top of whatever the rewrite did to its file (a recursive
    // function calls itself, and the call site is inside the body being edited).
    let base = rewritten.get(file).cloned().unwrap_or_else(|| text.clone());
    let (open, close) = if base == text {
        (open, close)
    } else {
        // Not at its old line and column: a call site above it may have come back from the
        // rewrite on fewer lines than it went in with (#58). The declaration itself is not a
        // call and the rewrite never touches it, so its own text is still there to find.
        locate_declaration(&base, &name, &old_inner).with_context(|| {
            format!(
                "`{name}`'s call sites were rewritten, but its declaration `fn {name}({})` is no \
                 longer in {} exactly once — a rewrite in the same file changed it or duplicated \
                 it. Nothing was written.",
                normalize(&old_inner),
                display(root, file)
            )
        })?
    };
    let new_inner = format_list(&old_inner, receiver.as_deref(), &new_params);
    let mut decl_text = String::with_capacity(base.len());
    decl_text.push_str(&base[..open]);
    decl_text.push_str(&new_inner);
    decl_text.push_str(&base[close..]);
    // The return type after the new list, then the visibility in front of `fn`: both in the
    // declaration's own text, in the same edit.
    let new_close = open + new_inner.len();
    let mut returns_change = None;
    if let Some(returns) = &modifiers.returns {
        let (was, out) = with_return_type(&decl_text, new_close, returns);
        if was.trim() != returns.trim() {
            returns_change = Some((was, returns.trim().to_string()));
            decl_text = out;
        }
    }
    let mut visibility_change = None;
    if let Some(visibility) = &modifiers.visibility {
        let name_at = decl_text[..open]
            .rfind(&format!("fn {name}"))
            .map(|i| i + 3)
            .context("the declaration's `fn` keyword is not where the parameter list says")?;
        let (was, out) = with_visibility(&decl_text, name_at, visibility)
            .context("the declaration's visibility could not be read")?;
        if was != visibility.trim() {
            visibility_change = Some((was, visibility.trim().to_string()));
            decl_text = out;
        }
    }
    if let Some((_, want)) = asyncness_change {
        let fn_at = decl_text[..open]
            .rfind(&format!("fn {name}"))
            .context("the declaration's `fn` keyword is not where the parameter list says")?;
        decl_text = with_async(&decl_text, fn_at, want);
    }
    rewritten.insert(file.to_path_buf(), decl_text);

    // The whole change judged together: the declaration and the call sites in one overlay.
    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    // A caller the rewrite did not touch still has to fit a new return type or visibility.
    let callers: Vec<PathBuf> = {
        let mut files: Vec<PathBuf> = refs
            .iter()
            .map(|(p, _, _)| p.clone())
            .filter(|p| !rewritten.contains_key(p))
            .collect();
        files.sort();
        files.dedup();
        files
    };
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &callers).await?;
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

    let edit = apply.then(|| whole_file_edit(&rewritten));
    let mut change = SignatureChange {
        symbol: name,
        root: root.to_path_buf(),
        file: display(root, file),
        old_signature: normalize(&old_inner),
        new_signature: normalize(&new_inner),
        rule,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        unmatched,
        unexpected,
        diagnostics,
        applied: false,
        returns: returns_change,
        visibility: visibility_change,
        asyncness: asyncness_change,
        not_async,
    };
    if let Some(edit) = edit {
        change.ensure_writable(force)?;
        anyhow::ensure!(
            change.diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Fix the request, \
             or pass `force: true` to write it anyway:\n  {}",
            change.diagnostics.len(),
            change.diagnostics.join("\n  ")
        );
        crate::refactor::apply_workspace_edit(root, &edit)?;
        change.applied = true;
    }
    Ok(change)
}
