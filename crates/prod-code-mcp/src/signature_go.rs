/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Go parameter reordering and removal through gopls (#448), with the same arguments and report
//! as [`crate::signature::change_with`], which hands every `.go` file here; so do the MCP tool
//! and the CLI through it. Nothing here calls back into `change_with`: only its helpers
//! (references, unreported callers, the whole-file edit), so the dispatch cannot recurse.
//!
//! gopls changes a signature when a rename is asked at the `func` keyword of a declaration and
//! the new name is the new signature: `func(b, a int) error`. Its v0.23.0 implementation accepts
//! the declared parameters reordered, some of them left out (adding one, changing a type or the
//! results is refused), and rewrites every call by inlining a wrapper. A deliberately narrow
//! result replacement is performed here instead: it changes only one primitive result token of
//! an ordinary free function or named value/pointer receiver method, then proves every direct
//! caller with the remote Go compiler.
//! That inliner runs with
//! effect analysis switched off, so `f(mark(a), mark(b))` comes back as `f(mark(b), mark(a))`,
//! and the argument of a removed parameter is dropped whatever it does: `f(1, g())` becomes
//! `f(1)` and `g` no longer runs. A removed parameter the body still reads is left in the body.
//! gopls's own edits are therefore used as they are, never re-spelled here, but they are not
//! trusted blindly:
//!
//! - a removed parameter must be proven unused, twice: its name appears nowhere in the body, and
//!   gopls lists its declaration and no other reference;
//! - every reference to the function is asked for first; a use that is not a call (a function
//!   value, a method value), a call whose arity cannot be checked, a reorder of two arguments
//!   where either can have an effect the other sees, and a dropped argument that is anything but
//!   a literal or a plain variable (a selector can dereference nil, an index can panic) are
//!   refused before gopls is asked;
//! - gopls's edit is checked against the change that was asked for: every call's arguments must
//!   come back as exactly the kept old arguments in the new order, the declaration's parameters
//!   likewise, nothing outside those lists may change, no comment may change anywhere, in no
//!   file outside the checkout;
//! - the whole proposal is type-checked in an overlay, and written in one transaction or not at
//!   all. `force` overrides none of this.
//!
//! A reference list, a rename or a validation that cannot be had stops the change.

use crate::signature::{Modifiers, Param, SignatureChange};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// The gopls release whose behaviour this adapter was written and tested against.
pub const GOPLS_VERSION: &str = "v0.23.0";

/// The part of the requirement that a refusal leaves open, so that it stays visible.
const STILL_OPEN: &str = "Go signature changes here are limited to reordering named parameters, \
     removing ones proven unused, and adding explicitly typed primitive parameters with literal \
     arguments to ordinary non-generic functions and named value or pointer receiver methods, plus \
     replacing one unnamed primitive result of an ordinary non-generic free function or named value or pointer receiver method; \
     variadics, generic functions or receivers, combined additions with removals, reorders or type \
     changes, named or multiple results, result removal or addition from void, scope-dependent or \
     composite results, method expressions or values, interface signatures or dispatch, and broader modifiers \
     remain open requirements (#448)";

/// A parameter as the declaration declares it, flattened out of Go's grouping: `a, b int` is two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoParam {
    pub name: String,
    /// The type as written, in canonical spacing and without comments.
    pub ty: String,
}

/// A function or method declaration, as its header writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Decl {
    /// Byte offset of the `func` keyword: where gopls is asked.
    func_at: usize,
    name: String,
    name_at: usize,
    /// The receiver, canonical, for a method.
    receiver: Option<String>,
    generic: bool,
    /// Offsets of the parentheses around the parameters.
    open: usize,
    close: usize,
    /// The results, canonical; empty when there are none.
    results: String,
}

/// A call of the function: where it is, its parentheses and its arguments as written.
#[derive(Debug, Clone)]
struct Call {
    path: PathBuf,
    at: String,
    open: usize,
    close: usize,
    args: Vec<String>,
}

/// What evaluating an argument can do, as far as its text shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgKind {
    /// A number, string or rune literal, or a function literal: nothing to evaluate that another
    /// argument can change.
    Literal,
    /// A read of a variable or a field (`x`, `a.b`, `&x`). `true`, `false` and `nil` are here
    /// too: they are predeclared identifiers, not keywords, and a program may declare a variable
    /// of that name (`true := 1`), so their spelling proves nothing.
    Place,
    /// Anything else: a call, a conversion, a receive, an index, an operator.
    Effectful,
}

/// Reorders or removes named parameters of the Go function or method at `file:line:col` (1-based; the
/// position may be anywhere from its `func` keyword to the `)` closing its parameters).
///
/// `request` names each retained parameter once; omitted parameters must be provably unused.
/// `modifiers` must be empty.
/// `force` is accepted for the common dispatch and overrides no refusal.
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
    let _ = force;
    anyhow::ensure!(
        file.extension().is_some_and(|e| e == "go"),
        "{} is not a Go file",
        file.display()
    );
    refuse_non_result_modifiers(modifiers)?;
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    anyhow::ensure!(
        line > 0 && col > 0,
        "{}:{line}:{col} is not in the file",
        file.display()
    );
    let at = offset_at(&text, line - 1, col - 1)
        .with_context(|| format!("{}:{line}:{col} is not in the file", file.display()))?;
    let decl = declarations(&text)
        .into_iter()
        .find(|d| d.func_at <= at && at <= d.close)
        .with_context(|| {
            refusal(format!(
                "{}:{line}:{col} is not in the header of a declared Go function or method (a \
                 function literal, a function type or an interface method has no declaration \
                 gopls can change)",
                display(root, file)
            ))
        })?;
    let declared = parameters(&text[decl.open + 1..decl.close]).map_err(|why| {
        refusal(format!(
            "`{}` cannot be reordered by name: {why}; name every parameter first",
            decl.name
        ))
    })?;
    if let Some(result) = modifiers.returns.as_deref() {
        return replace_result(
            remote, root, file, text, decl, declared, request, result, apply,
        )
        .await;
    }
    if request.iter().any(|p| matches!(p, Param::Add { .. })) {
        return add_parameters(remote, root, file, text, decl, declared, request, apply).await;
    }
    let order = permutation(&declared, request)?;
    let arity = declared.len();
    let variadic = declared.last().is_some_and(|p| p.ty.starts_with("..."));
    if variadic && order.contains(&(arity - 1)) && order.last() != Some(&(arity - 1)) {
        return Err(refusal(format!(
            "`{}` is variadic, and Go allows `...` only on the last parameter",
            declared[arity - 1].name
        )));
    }
    let removed: Vec<usize> = (0..arity).filter(|i| !order.contains(i)).collect();
    let kind = match (removed.is_empty(), is_subsequence(&order)) {
        (true, _) => "reorder",
        (false, true) => "removal",
        (false, false) => "removal and reorder",
    };
    // The body, and where each removed parameter is named in the declaration: what the proof
    // that it is unused is about.
    let mut removed_at = Vec::new();
    let mut body = (0, 0);
    if !removed.is_empty() {
        let names = removed_names(&declared, &removed);
        if decl.generic {
            return Err(refusal(format!(
                "removing {names} from the generic `{}` is not supported: a type argument may be \
                 inferred from the removed argument, and gopls cannot change a generic \
                 function's signature while it has calls",
                decl.name
            )));
        }
        body = body_open(&text, decl.close + 1)
            .and_then(|open| closing(&text, open).map(|close| (open, close)))
            .with_context(|| {
                refusal(format!(
                    "removing {names} is refused: `{}` has no body here (a function implemented \
                     in assembly reads its arguments by position)",
                    decl.name
                ))
            })?;
        let named_at = parameter_names_at(&text, decl.open, decl.close);
        for &i in &removed {
            let name = &declared[i].name;
            let at = named_at
                .get(i)
                .copied()
                .filter(|&at| ident_at(&text, at) == Some(name.as_str()))
                .with_context(|| {
                    refusal(format!(
                        "removing `{name}` is refused: its name cannot be found in the \
                         declaration of `{}`",
                        decl.name
                    ))
                })?;
            let uses: Vec<String> = identifier_uses(&text, body.0, body.1, name)
                .into_iter()
                .map(|o| position(root, file, &text, o))
                .collect();
            if !uses.is_empty() {
                return Err(refusal(format!(
                    "removing `{name}` is refused: the body of `{}` still uses it at {}",
                    decl.name,
                    uses.join(", ")
                )));
            }
            removed_at.push((i, at));
        }
    }
    let new_params: Vec<GoParam> = order.iter().map(|&i| declared[i].clone()).collect();
    let signature = format!(
        "func({}){}",
        new_params
            .iter()
            .map(|p| format!("{} {}", p.name, p.ty))
            .collect::<Vec<_>>()
            .join(", "),
        if decl.results.is_empty() {
            String::new()
        } else {
            format!(" {}", decl.results)
        }
    );

    // Every reference, before anything is asked of gopls: what cannot be rewritten or would run
    // differently is refused with its location.
    let canonical_root = std::fs::canonicalize(root)
        .with_context(|| format!("cannot resolve the checkout {}", root.display()))?;
    let (name_line, name_col) = line_col_utf16(&text, decl.name_at);
    let mut refs = crate::signature::references(remote, root, file, name_line + 1, name_col + 1)
        .await
        .with_context(|| {
            format!(
                "cannot list the references to `{}`; nothing was written",
                decl.name
            )
        })?;
    refs.sort();
    refs.dedup();
    let mut originals: BTreeMap<PathBuf, String> = BTreeMap::new();
    originals.insert(file.to_path_buf(), text.clone());
    let mut calls = Vec::new();
    let mut values = Vec::new();
    for (reported, l, c) in &refs {
        ensure_inside(&canonical_root, reported)?;
        // One spelling per file, the declaring file's own when the reference is in it.
        let path = originals
            .keys()
            .find(|k| same_file(k, reported))
            .cloned()
            .unwrap_or_else(|| reported.clone());
        if !originals.contains_key(&path) {
            let t = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}; nothing was written", path.display()))?;
            originals.insert(path.clone(), t);
        }
        let t = &originals[&path];
        let where_ = format!("{}:{l}:{c}", display(root, &path));
        let name_at = offset_at(t, l.saturating_sub(1), c.saturating_sub(1))
            .filter(|o| t[*o..].starts_with(decl.name.as_str()))
            .with_context(|| {
                format!(
                    "the reference {where_} does not point at `{}`; the file may have changed \
                     since it was read. Nothing was written",
                    decl.name
                )
            })?;
        match call_parens(t, name_at) {
            Some((open, close)) => calls.push(Call {
                path: path.clone(),
                at: where_,
                open,
                close,
                args: split_list(&strip_comments(&t[open + 1..close])),
            }),
            None => values.push(where_),
        }
    }
    if !values.is_empty() {
        return Err(refusal(format!(
            "`{}` is used as a value, not called, at {}; a function value keeps the old \
             signature and cannot be rewritten",
            decl.name,
            values.join(", ")
        )));
    }
    // The analyzer's word that each removed parameter is unused: its declaration, and nothing else.
    for &(i, at) in &removed_at {
        let name = &declared[i].name;
        let uses = parameter_uses(remote, root, file, &text, name, at, body)
            .await
            .map_err(|why| {
                refusal(format!(
                    "removing `{name}` is refused: gopls's references cannot prove it unused: \
                     {why:#}"
                ))
            })?;
        if !uses.is_empty() {
            return Err(refusal(format!(
                "removing `{name}` is refused: the body of `{}` still uses it at {} (gopls)",
                decl.name,
                uses.iter()
                    .map(|&o| position(root, file, &text, o))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    let hazards = effect_hazards(&decl.name, &declared, &order, variadic, &calls);
    anyhow::ensure!(
        hazards.is_empty(),
        "the new signature would change what the program does, not only how the calls are \
         written; nothing was written, and `force` does not override this:\n  {}\nbind such an \
         argument to a local before the call and pass the local",
        hazards.join("\n  ")
    );

    // gopls writes the change.
    let (func_line, func_col) = line_col_utf16(&text, decl.func_at);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
        .to_string();
    let edit = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/rename",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": func_line, "character": func_col },
            "newName": signature,
        }),
    )
    .await
    .map_err(|e| {
        let generic = if decl.generic {
            " (gopls cannot reorder a generic function's parameters while it has calls)"
        } else {
            ""
        };
        anyhow::anyhow!(
            "gopls refused the {kind} of `{}` as `{signature}`{generic}: {e:#}; nothing was \
             written. {STILL_OPEN}",
            decl.name
        )
    })?;
    anyhow::ensure!(
        !edit.is_null(),
        "gopls answered the {kind} of `{}` with no edit; nothing was written. {STILL_OPEN}",
        decl.name
    );
    let edits = edits_by_file(&canonical_root, &edit, &mut originals)?;

    // What gopls wrote, against what was asked.
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut unexpected = Vec::new();
    for (path, list) in &edits {
        let old = &originals[path];
        let new = splice(old, list);
        // gopls prints the new parameter list afresh, and a comment inside it would be lost.
        if comments(old) != comments(&new) {
            unexpected.push(format!(
                "{}: gopls's edit drops or changes a comment",
                display(root, path)
            ));
        }
        rewritten.insert(path.clone(), new);
        let mut allowed: Vec<(usize, usize)> = calls
            .iter()
            .filter(|c| &c.path == path)
            .map(|c| (c.open + 1, c.close))
            .collect();
        if path == file {
            allowed.push((decl.open + 1, decl.close));
        }
        for (s, e, _) in list {
            if !allowed.iter().any(|(a, b)| a <= s && e <= b) {
                let (l, _) = line_col_utf16(old, *s);
                unexpected.push(format!(
                    "{}:{}: gopls changed text outside the argument and parameter lists",
                    display(root, path),
                    l + 1
                ));
            }
        }
    }
    let mut unmatched = Vec::new();
    for call in &calls {
        let expected = permuted(&call.args, &order, arity, variadic);
        let got = match edits.get(&call.path) {
            Some(list) => map_offset(list, call.open).and_then(|open| {
                let new = &rewritten[&call.path];
                (new.as_bytes().get(open) == Some(&b'('))
                    .then(|| closing(new, open))
                    .flatten()
                    .map(|close| split_list(&strip_comments(&new[open + 1..close])))
            }),
            None => Some(call.args.clone()),
        };
        let same = got.as_ref().is_some_and(|g| {
            g.len() == expected.len()
                && g.iter()
                    .zip(&expected)
                    .all(|(a, b)| canonical(a) == canonical(b))
        });
        if !same {
            unmatched.push(format!(
                "{}: gopls wrote ({}) where the new arguments are ({})",
                call.at,
                got.map_or_else(|| "an unreadable call".to_string(), |g| g.join(", ")),
                expected.join(", ")
            ));
        }
    }
    let new_decl = edits
        .get(file)
        .and_then(|list| map_offset(list, decl.func_at))
        .and_then(|func_at| header(&rewritten[file], func_at));
    let new_signature = match &new_decl {
        Some(d) if d.name == decl.name && d.receiver == decl.receiver => {
            let new_text = &rewritten[file];
            let got = parameters(&new_text[d.open + 1..d.close]).unwrap_or_default();
            if got != new_params || d.results != decl.results {
                unexpected.push(format!(
                    "{}: gopls declared ({}){} where ({}){} was asked",
                    display(root, file),
                    list_text(&got),
                    suffix(&d.results),
                    list_text(&new_params),
                    suffix(&decl.results)
                ));
            }
            normalize(&new_text[d.open + 1..d.close])
        }
        _ => {
            unexpected.push(format!(
                "{}: gopls did not rewrite the declaration of `{}`",
                display(root, file),
                decl.name
            ));
            normalize(&text[decl.open + 1..decl.close])
        }
    };

    // The whole proposal, judged together; files that call the name but were not reported are
    // checked with it.
    let proposal: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let mut also: Vec<PathBuf> = originals
        .keys()
        .filter(|p| !rewritten.contains_key(*p))
        .cloned()
        .collect();
    let checked: Vec<PathBuf> = originals.keys().cloned().collect();
    also.extend(crate::signature::unreported_callers(
        root, file, &decl.name, &checked,
    ));
    let reports = crate::diagnostics::validate_texts(remote, root, &proposal, &also)
        .await
        .context("the proposal could not be validated; nothing was written")?;
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
            unmatched.is_empty() && unexpected.is_empty(),
            "gopls's edit is not the {kind} that was asked for; nothing was written:\n  {}",
            unmatched
                .iter()
                .chain(&unexpected)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty(),
            "the changed program does not type-check ({} error(s)); nothing was written, and \
             `force` does not override this:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        for (path, old) in &originals {
            let now = std::fs::read_to_string(path).unwrap_or_default();
            anyhow::ensure!(
                now == *old,
                "{} changed while the {kind} was planned; nothing was written",
                display(root, path)
            );
        }
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }

    Ok(SignatureChange {
        symbol: decl.name.clone(),
        root: root.to_path_buf(),
        file: display(root, file),
        old_signature: normalize(&text[decl.open + 1..decl.close]),
        new_signature,
        rule: signature.clone(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        unmatched,
        unexpected,
        diagnostics,
        applied,
        returns: None,
        visibility: None,
        asyncness: None,
        not_async: Vec::new(),
    })
}

/// Replaces the sole unnamed primitive result of an ordinary free function or named value/pointer
/// receiver method. gopls cannot make this edit, and a result change can make an otherwise
/// untouched caller ill typed, so the
/// reference proof and compiler-shadow gate are mandatory even for a preview and even for a
/// no-op request.
#[allow(clippy::too_many_arguments)]
async fn replace_result(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: String,
    decl: Decl,
    declared: Vec<GoParam>,
    request: &[Param],
    requested_result: &str,
    apply: bool,
) -> Result<SignatureChange> {
    let receiver = decl
        .receiver
        .as_deref()
        .map(ordinary_receiver)
        .transpose()
        .map_err(|why| {
            refusal(format!(
                "changing the result of `{}` is refused: {why}",
                decl.name
            ))
        })?;
    anyhow::ensure!(
        !decl.generic,
        "{}",
        refusal(format!(
            "changing the result of generic function `{}` is not supported",
            decl.name
        ))
    );
    anyhow::ensure!(
        !declared
            .iter()
            .any(|parameter| parameter.ty.starts_with("...")),
        "{}",
        refusal(format!(
            "changing the result of variadic function `{}` is not supported",
            decl.name
        ))
    );
    unchanged_parameters(&declared, request)?;
    anyhow::ensure!(
        primitive_type(&decl.results),
        "{}",
        refusal(format!(
            "changing the results of `{}` is supported only for one unnamed primitive result, not `{}`",
            decl.name,
            if decl.results.is_empty() {
                "no result"
            } else {
                &decl.results
            }
        ))
    );
    let requested_result = requested_result.trim();
    anyhow::ensure!(
        primitive_type(requested_result),
        "{}",
        refusal(format!(
            "the result of `{}` must be an ordinary primitive spelling, not `{requested_result}`",
            decl.name
        ))
    );
    ensure_predeclared_types_unshadowed(root, file, &[&decl.results, requested_result])
        .map_err(|why| {
            refusal(format!(
                "changing the result of `{}` is refused because primitive type identity cannot be proven: {why:#}",
                decl.name
            ))
        })?;
    let body = body_open(&text, decl.close + 1).with_context(|| {
        refusal(format!(
            "changing the result of `{}` is refused because it has no body here",
            decl.name
        ))
    })?;
    closing(&text, body)
        .with_context(|| refusal(format!("the body of `{}` does not close", decl.name)))?;

    // Read every path gopls names before considering a write. In particular, an indirect value,
    // stale coordinate or malformed reference is evidence we do not have, not an empty caller.
    let canonical_root = std::fs::canonicalize(root)
        .with_context(|| format!("cannot resolve the checkout {}", root.display()))?;
    if receiver.is_some() {
        receiver_interface_evidence(remote, root, file, &text, &decl.name, decl.name_at)
            .await
            .map_err(|why| {
                refusal(format!(
                    "changing the result of receiver method `{}` is refused: {why:#}",
                    decl.name
                ))
            })?;
    }
    let (originals, calls) = function_reference_evidence(
        remote,
        root,
        file,
        &canonical_root,
        &text,
        &decl,
        receiver.as_ref().map(|receiver| receiver.ty.as_str()),
    )
    .await
    .map_err(|why| {
        refusal(format!(
            "cannot prove every reference to `{}` is a supported direct call or its declaration: \
             {why:#}",
            decl.name
        ))
    })?;
    let arity = declared.len();
    for call in &calls {
        anyhow::ensure!(
            call.args.len() == arity
                && !call
                    .args
                    .last()
                    .is_some_and(|arg| arg.trim_end().ends_with("...")),
            "{}",
            refusal(format!(
                "{}: the call passes {} argument(s) and `{}` declares {arity}, so its result \
                 replacement cannot be reconciled exactly",
                call.at,
                call.args.len(),
                decl.name
            ))
        );
    }

    let result_at = skip_space(&text, decl.close + 1);
    anyhow::ensure!(
        text[result_at..].starts_with(&decl.results),
        "the primitive result token of `{}` cannot be located; nothing was written",
        decl.name
    );
    let mut rewritten = BTreeMap::new();
    if decl.results != requested_result {
        let replacement = splice(
            &text,
            &[(
                result_at,
                result_at + decl.results.len(),
                requested_result.to_string(),
            )],
        );
        anyhow::ensure!(
            comments(&text) == comments(&replacement),
            "{}: changing its result would drop or change a comment; nothing was written",
            display(root, file)
        );
        rewritten.insert(file.to_path_buf(), replacement);
    }
    let proposal: Vec<(PathBuf, String)> = if rewritten.is_empty() {
        vec![(file.to_path_buf(), text.clone())]
    } else {
        rewritten
            .iter()
            .map(|(path, source)| (path.clone(), source.clone()))
            .collect()
    };
    let compiler = crate::verify::compile_go_shadow(remote, root, file, &proposal)
        .await
        .context(
            "the complete Go result replacement could not be compiled in its private shadow; nothing was written",
        )?;
    anyhow::ensure!(
        compiler.passed,
        "the changed Go project does not compile; nothing was written, and `force` does not \
         override this:\n{}",
        compiler.output.trim()
    );

    let mut applied = false;
    if apply {
        for (path, old) in &originals {
            let now = std::fs::read_to_string(path).unwrap_or_default();
            anyhow::ensure!(
                now == *old,
                "{} changed while the result replacement was planned and compiled; nothing was written",
                display(root, path)
            );
        }
        if !rewritten.is_empty() {
            crate::refactor::apply_workspace_edit(
                root,
                &crate::signature::whole_file_edit(&rewritten),
            )?;
            applied = true;
        }
    }
    Ok(SignatureChange {
        symbol: decl.name,
        root: root.to_path_buf(),
        file: display(root, file),
        old_signature: normalize(&text[decl.open + 1..decl.close]),
        new_signature: normalize(&text[decl.open + 1..decl.close]),
        rule: String::new(),
        rewritten: rewritten
            .into_iter()
            .map(|(path, source)| (path.to_string_lossy().into_owned(), source))
            .collect(),
        unmatched: Vec::new(),
        unexpected: Vec::new(),
        diagnostics: Vec::new(),
        applied,
        returns: Some((decl.results, requested_result.to_string())),
        visibility: None,
        asyncness: None,
        not_async: Vec::new(),
    })
}

/// Result replacement has no call-site edits. Reordering, removing or adding a parameter would
/// make this a different operation and is refused before the compiler can normalize it away.
fn unchanged_parameters(declared: &[GoParam], request: &[Param]) -> Result<()> {
    anyhow::ensure!(
        request.len() == declared.len(),
        "{}",
        refusal(
            "a Go result replacement requires the existing named parameter list exactly unchanged"
                .to_string()
        )
    );
    for (expected, requested) in declared.iter().zip(request) {
        anyhow::ensure!(
            matches!(requested, Param::Keep(name) if name == &expected.name),
            "{}",
            refusal(format!(
                "a Go result replacement requires the existing named parameter list exactly unchanged; expected `{}`",
                expected.name
            ))
        );
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Addition {
    /// How many old parameters precede this one.
    boundary: usize,
    name: String,
    ty: String,
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Receiver {
    binding: String,
    ty: String,
}

/// Adds explicitly typed primitive parameters to an ordinary function or named value/pointer
/// receiver method. Unlike reorders and removals, gopls has no native edit for this shape, so its
/// complete reference answer is used as the proof obligation and the adapter makes insertion-only
/// edits itself.
#[allow(clippy::too_many_arguments)]
async fn add_parameters(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: String,
    decl: Decl,
    declared: Vec<GoParam>,
    request: &[Param],
    apply: bool,
) -> Result<SignatureChange> {
    let receiver = decl
        .receiver
        .as_deref()
        .map(ordinary_receiver)
        .transpose()
        .map_err(|why| {
            refusal(format!(
                "adding parameters to `{}` is refused: {why}",
                decl.name
            ))
        })?;
    anyhow::ensure!(
        !decl.generic,
        "{}",
        refusal(format!(
            "adding parameters to the generic function `{}` is not supported",
            decl.name
        ))
    );
    anyhow::ensure!(
        !declared.iter().any(|p| p.ty.starts_with("...")),
        "{}",
        refusal(format!(
            "adding parameters to the variadic function `{}` is not supported",
            decl.name
        ))
    );
    let body_open = body_open(&text, decl.close + 1).with_context(|| {
        refusal(format!(
            "adding parameters to `{}` is refused because it has no body here (an assembly or \
             external declaration cannot be proven safe)",
            decl.name
        ))
    })?;
    let body = (
        body_open,
        closing(&text, body_open)
            .with_context(|| refusal(format!("the body of `{}` does not close", decl.name)))?,
    );
    let additions = addition_plan(&declared, request)?;
    for added in &additions {
        if receiver
            .as_ref()
            .is_some_and(|receiver| receiver.binding == added.name)
        {
            return Err(refusal(format!(
                "adding `{}` is refused because it duplicates the receiver binding of `{}`",
                added.name, decl.name
            )));
        }
        let captures: Vec<String> = identifier_uses(&text, body.0, body.1, &added.name)
            .into_iter()
            .map(|at| position(root, file, &text, at))
            .collect();
        if !captures.is_empty() {
            return Err(refusal(format!(
                "adding `{}` is refused because it would shadow existing references in the body \
                 of `{}` at {}",
                added.name,
                decl.name,
                captures.join(", ")
            )));
        }
    }

    let canonical_root = std::fs::canonicalize(root)
        .with_context(|| format!("cannot resolve the checkout {}", root.display()))?;
    if receiver.is_some() {
        receiver_interface_evidence(remote, root, file, &text, &decl.name, decl.name_at)
            .await
            .map_err(|why| {
                refusal(format!(
                    "adding parameters to `{}` is refused because its interface implementations \
                     cannot be proven absent: {why:#}",
                    decl.name
                ))
            })?;
    }
    let (originals, calls) = function_reference_evidence(
        remote,
        root,
        file,
        &canonical_root,
        &text,
        &decl,
        receiver.as_ref().map(|receiver| receiver.ty.as_str()),
    )
    .await
    .map_err(|why| {
        refusal(format!(
            "cannot prove every reference to `{}` is a supported direct call or its declaration: \
             {why:#}",
            decl.name
        ))
    })?;
    let arity = declared.len();
    for call in &calls {
        anyhow::ensure!(
            call.args.len() == arity
                && !call
                    .args
                    .last()
                    .is_some_and(|arg| arg.trim_end().ends_with("...")),
            "{}",
            refusal(format!(
                "{}: the call passes {} argument(s) and `{}` declares {arity}, so the insertion \
                 cannot be reconciled exactly",
                call.at,
                call.args.len(),
                decl.name
            ))
        );
    }

    let groups = addition_groups(&additions);
    let mut edits: BTreeMap<PathBuf, Vec<TextEdit>> = BTreeMap::new();
    let declaration_edits = insertion_edits(&text, decl.open, decl.close, arity, &groups, true)
        .map_err(|why| {
            refusal(format!(
                "the declaration of `{}` cannot be extended safely: {why}",
                decl.name
            ))
        })?;
    edits
        .entry(file.to_path_buf())
        .or_default()
        .extend(declaration_edits);
    for call in &calls {
        let old = &originals[&call.path];
        let call_edits = insertion_edits(old, call.open, call.close, arity, &groups, false)
            .map_err(|why| refusal(format!("{} cannot be extended safely: {why}", call.at)))?;
        edits
            .entry(call.path.clone())
            .or_default()
            .extend(call_edits);
    }
    for list in edits.values_mut() {
        list.sort_by_key(|(start, end, _)| (*start, *end));
        for pair in list.windows(2) {
            anyhow::ensure!(
                pair[0].1 <= pair[1].0 && !(pair[0].0 == pair[1].0 && pair[0].1 == pair[1].1),
                "the insertion plan for `{}` overlaps itself; nothing was written",
                decl.name
            );
        }
    }

    let mut rewritten = BTreeMap::new();
    for (path, list) in &edits {
        let old = &originals[path];
        let new = splice(old, list);
        anyhow::ensure!(
            comments(old) == comments(&new),
            "{}: adding parameters would drop or change a comment; nothing was written",
            display(root, path)
        );
        rewritten.insert(path.clone(), new);
    }

    let expected_params = requested_go_params(&declared, &additions);
    let new_decl = edits
        .get(file)
        .and_then(|list| map_offset(list, decl.func_at))
        .and_then(|at| header(&rewritten[file], at))
        .context("the inserted declaration cannot be read back; nothing was written")?;
    let got_params =
        parameters(&rewritten[file][new_decl.open + 1..new_decl.close]).map_err(|why| {
            anyhow::anyhow!(
                "the inserted declaration cannot be read back: {why}; nothing was written"
            )
        })?;
    anyhow::ensure!(
        new_decl.name == decl.name
            && new_decl.receiver == decl.receiver
            && new_decl.results == decl.results
            && got_params == expected_params,
        "the insertion did not produce exactly the requested declaration of `{}`; nothing was written",
        decl.name
    );
    for call in &calls {
        let list = &edits[&call.path];
        let open = map_offset(list, call.open)
            .context("an inserted call cannot be located again; nothing was written")?;
        let new = &rewritten[&call.path];
        let close =
            closing(new, open).context("an inserted call does not close; nothing was written")?;
        let got = split_list(&strip_comments(&new[open + 1..close]));
        let expected = requested_arguments(&call.args, request);
        anyhow::ensure!(
            got.len() == expected.len()
                && got
                    .iter()
                    .zip(&expected)
                    .all(|(actual, expected)| canonical(actual) == canonical(expected)),
            "{}: the insertion wrote ({}) instead of exactly ({}); nothing was written",
            call.at,
            got.join(", "),
            expected.join(", ")
        );
    }

    let proposal: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(path, source)| (path.clone(), source.clone()))
        .collect();
    let compiler = crate::verify::compile_go_shadow(remote, root, file, &proposal)
        .await
        .context(
            "the complete Go proposal could not be compiled in its private shadow; nothing was written",
        )?;
    anyhow::ensure!(
        compiler.passed,
        "the changed Go project does not compile; nothing was written, and `force` does not \
         override this:\n{}",
        compiler.output.trim()
    );

    let mut applied = false;
    if apply {
        for (path, old) in &originals {
            let now = std::fs::read_to_string(path).unwrap_or_default();
            anyhow::ensure!(
                now == *old,
                "{} changed while the addition was planned and compiled; nothing was written",
                display(root, path)
            );
        }
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }

    let signature = format!(
        "func({}){}",
        list_text(&expected_params),
        suffix(&decl.results)
    );
    let new_signature = normalize(&rewritten[file][new_decl.open + 1..new_decl.close]);
    Ok(SignatureChange {
        symbol: decl.name,
        root: root.to_path_buf(),
        file: display(root, file),
        old_signature: normalize(&text[decl.open + 1..decl.close]),
        new_signature,
        rule: signature,
        rewritten: rewritten
            .into_iter()
            .map(|(path, source)| (path.to_string_lossy().into_owned(), source))
            .collect(),
        unmatched: Vec::new(),
        unexpected: Vec::new(),
        diagnostics: Vec::new(),
        applied,
        returns: None,
        visibility: None,
        asyncness: None,
        not_async: Vec::new(),
    })
}

/// The only addition shape supported in this increment: every old parameter is retained once in
/// its original order, with one or more new parameters inserted between them.
fn addition_plan(declared: &[GoParam], request: &[Param]) -> Result<Vec<Addition>> {
    let mut old_at = 0usize;
    let mut additions = Vec::new();
    let mut new_names = BTreeSet::new();
    for item in request {
        match item {
            Param::Keep(name) => {
                let expected = declared.get(old_at).map(|p| p.name.as_str());
                anyhow::ensure!(
                    expected == Some(name.as_str()),
                    "{}",
                    refusal(format!(
                        "an addition must retain every old parameter exactly once in its declared \
                         order; expected {} next, got `{name}`",
                        expected.map_or("no more old parameters".to_string(), |n| format!("`{n}`"))
                    ))
                );
                old_at += 1;
            }
            Param::Add { name, ty, value } => {
                anyhow::ensure!(
                    go_identifier(name),
                    "{}",
                    refusal(format!("`{name}` is not an ordinary Go parameter name"))
                );
                anyhow::ensure!(
                    !declared.iter().any(|p| p.name == *name) && new_names.insert(name.clone()),
                    "{}",
                    refusal(format!(
                        "the added parameter `{name}` duplicates another parameter"
                    ))
                );
                let ty = ty.trim();
                anyhow::ensure!(
                    primitive_type(ty),
                    "{}",
                    refusal(format!(
                        "the type of added `{name}` must be an ordinary primitive spelling, not \
                         `{ty}`"
                    ))
                );
                let value = value.trim();
                anyhow::ensure!(
                    scalar_literal(value),
                    "{}",
                    refusal(format!(
                        "the argument for added `{name}` must be one numeric, string or rune \
                         literal, not `{value}`"
                    ))
                );
                additions.push(Addition {
                    boundary: old_at,
                    name: name.clone(),
                    ty: ty.to_string(),
                    value: value.to_string(),
                });
            }
        }
    }
    anyhow::ensure!(
        old_at == declared.len(),
        "{}",
        refusal(format!(
            "an addition must retain every old parameter exactly once in its declared order; \
             {} would be removed",
            declared[old_at..]
                .iter()
                .map(|p| format!("`{}`", p.name))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    );
    anyhow::ensure!(!additions.is_empty(), "there is no parameter to add");
    Ok(additions)
}

fn go_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        && !matches!(
            name,
            "break"
                | "default"
                | "func"
                | "interface"
                | "select"
                | "case"
                | "defer"
                | "go"
                | "map"
                | "struct"
                | "chan"
                | "else"
                | "goto"
                | "package"
                | "switch"
                | "const"
                | "fallthrough"
                | "if"
                | "range"
                | "type"
                | "continue"
                | "for"
                | "import"
                | "return"
                | "var"
        )
        && name != "_"
}

fn primitive_type(ty: &str) -> bool {
    matches!(
        ty,
        "string"
            | "byte"
            | "rune"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
            | "float32"
            | "float64"
            | "complex64"
            | "complex128"
    )
}

/// Primitive-looking names are safe only while the declaring package has not shadowed them.
/// Go's package block spans files and declarations are order-independent, so checking only the
/// signature token or accepting a successful compile would mistake a user-defined type or alias
/// for a predeclared primitive.
fn ensure_predeclared_types_unshadowed(root: &Path, file: &Path, types: &[&str]) -> Result<()> {
    let directory = file
        .parent()
        .with_context(|| format!("{} has no containing package directory", file.display()))?;
    let declaring_text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let package = package_name(&declaring_text)
        .with_context(|| format!("cannot identify the Go package in {}", file.display()))?;
    let wanted: BTreeSet<&str> = types.iter().copied().collect();

    for entry in std::fs::read_dir(directory).with_context(|| {
        format!(
            "cannot inspect the Go package directory {}",
            directory.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "cannot inspect an entry in the Go package directory {}",
                directory.display()
            )
        })?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "go") {
            continue;
        }
        let file_type = entry
            .file_type()
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        // `DirEntry::file_type` deliberately does not follow links. Go and the checkout sync
        // do, so silently skipping one could make a package-level alias look predeclared. Do
        // not follow it here: the link might lead outside the checkout we are allowed to read.
        if file_type.is_symlink() {
            anyhow::bail!(
                "cannot prove primitive type identity: linked Go source {} is not inspected",
                display(root, &path)
            );
        }
        if !file_type.is_file() {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        if package_name(&source) != Some(package) {
            continue;
        }
        for name in package_type_names(&source) {
            if wanted.contains(name.as_str()) {
                anyhow::bail!(
                    "`{name}` is declared as a package type in {}; its spelling does not name the predeclared primitive",
                    display(root, &path)
                );
            }
        }
    }
    Ok(())
}

fn package_name(text: &str) -> Option<&str> {
    let at = skip_space(text, 0);
    if !text[at..].starts_with("package")
        || at > 0 && is_ident_byte(text.as_bytes()[at - 1])
        || text
            .as_bytes()
            .get(at + "package".len())
            .is_some_and(|byte| is_ident_byte(*byte))
    {
        return None;
    }
    let start = skip_space(text, at + "package".len());
    let end = identifier_end(text, start);
    (end > start).then_some(&text[start..end])
}

fn identifier_end(text: &str, start: usize) -> usize {
    let mut end = start;
    while end < text.len() && is_ident_byte(text.as_bytes()[end]) {
        end += 1;
    }
    end
}

/// Names introduced by package-level `type` declarations, including parenthesized declaration
/// groups. Comments and literal text are skipped, and declarations inside function bodies are not
/// package declarations.
fn package_type_names(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut names = Vec::new();
    let (mut braces, mut brackets, mut parens, mut at) = (0usize, 0usize, 0usize, 0usize);
    while at < bytes.len() {
        if let Some(end) = skip_opaque(bytes, at) {
            at = end;
            continue;
        }
        match bytes[at] {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b't' if braces == 0
                && brackets == 0
                && parens == 0
                && text[at..].starts_with("type")
                && (at == 0 || !is_ident_byte(bytes[at - 1]))
                && !bytes
                    .get(at + "type".len())
                    .is_some_and(|byte| is_ident_byte(*byte)) =>
            {
                let start = skip_space(text, at + "type".len());
                if bytes.get(start) == Some(&b'(') {
                    let Some(close) = closing(text, start) else {
                        return names;
                    };
                    grouped_type_names(text, start, close, &mut names);
                    at = close + 1;
                    continue;
                }
                let end = identifier_end(text, start);
                if end > start {
                    names.push(text[start..end].to_string());
                }
                at = end;
                continue;
            }
            _ => {}
        }
        at += 1;
    }
    names
}

fn grouped_type_names(text: &str, open: usize, close: usize, names: &mut Vec<String>) {
    let bytes = text.as_bytes();
    let (mut parens, mut brackets, mut braces) = (0usize, 0usize, 0usize);
    let (mut at, mut at_spec_start) = (open + 1, true);
    while at < close {
        if let Some(end) = skip_opaque(bytes, at) {
            if at_spec_start || text[at..end].contains('\n') {
                at_spec_start = true;
            }
            at = end;
            continue;
        }
        if at_spec_start && bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if at_spec_start {
            let end = identifier_end(text, at);
            if end > at {
                names.push(text[at..end].to_string());
                at_spec_start = false;
                at = end;
                continue;
            }
        }
        match bytes[at] {
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b';' if parens == 0 && brackets == 0 && braces == 0 => at_spec_start = true,
            b'\n' if parens == 0 && brackets == 0 && braces == 0 => at_spec_start = true,
            _ => {}
        }
        at += 1;
    }
}

fn scalar_literal(value: &str) -> bool {
    if value.is_empty() || !is_literal(value) || is_func_literal(value) {
        return false;
    }
    match value.as_bytes()[0] {
        quote @ (b'"' | b'\'' | b'`') => {
            value.len() >= 2 && value.as_bytes().last() == Some(&quote)
        }
        _ => true,
    }
}

fn addition_groups(additions: &[Addition]) -> BTreeMap<usize, Vec<&Addition>> {
    let mut groups: BTreeMap<usize, Vec<&Addition>> = BTreeMap::new();
    for addition in additions {
        groups.entry(addition.boundary).or_default().push(addition);
    }
    groups
}

fn requested_go_params(declared: &[GoParam], additions: &[Addition]) -> Vec<GoParam> {
    let groups = addition_groups(additions);
    let mut out = Vec::with_capacity(declared.len() + additions.len());
    for boundary in 0..=declared.len() {
        if let Some(group) = groups.get(&boundary) {
            out.extend(group.iter().map(|added| GoParam {
                name: added.name.clone(),
                ty: added.ty.clone(),
            }));
        }
        if let Some(old) = declared.get(boundary) {
            out.push(old.clone());
        }
    }
    out
}

fn requested_arguments(old: &[String], request: &[Param]) -> Vec<String> {
    let mut old_at = 0usize;
    let mut out = Vec::with_capacity(request.len());
    for item in request {
        match item {
            Param::Keep(_) => {
                out.push(old[old_at].clone());
                old_at += 1;
            }
            Param::Add { value, .. } => out.push(value.trim().to_string()),
        }
    }
    out
}

/// Zero-length edits that insert each group at its boundary. Every original byte remains where it
/// was; an existing comma becomes the separator before a middle or trailing insertion.
fn insertion_edits(
    text: &str,
    open: usize,
    close: usize,
    arity: usize,
    groups: &BTreeMap<usize, Vec<&Addition>>,
    declaration: bool,
) -> std::result::Result<Vec<TextEdit>, String> {
    let commas = top_level_commas(text, open + 1, close);
    let trailing = commas
        .last()
        .copied()
        .filter(|comma| strip_comments(&text[comma + 1..close]).trim().is_empty());
    if arity == 0 {
        if !strip_comments(&text[open + 1..close]).trim().is_empty() {
            return Err("the empty list contains text that cannot be attributed".to_string());
        }
    } else if commas.len() + usize::from(trailing.is_none()) != arity {
        return Err("the list's comma structure does not match its arity".to_string());
    }
    let mut out = Vec::new();
    for (&boundary, group) in groups {
        if boundary > arity {
            return Err(format!(
                "insertion boundary {boundary} is past arity {arity}"
            ));
        }
        if declaration && boundary > 0 && boundary < arity {
            let start = if boundary == 1 {
                open + 1
            } else {
                commas[boundary - 2] + 1
            };
            let end = commas[boundary - 1];
            if !parameter_piece_has_type(&text[start..end]) {
                return Err(format!(
                    "inserting after grouped parameter {} would change that old parameter's type",
                    boundary
                ));
            }
        }
        let inserted = group
            .iter()
            .map(|added| {
                if declaration {
                    format!("{} {}", added.name, added.ty)
                } else {
                    added.value.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let (at, replacement) = if arity == 0 {
            (open + 1, inserted)
        } else if boundary == 0 {
            (open + 1, format!("{inserted}, "))
        } else if boundary < arity {
            (commas[boundary - 1] + 1, format!(" {inserted},"))
        } else if let Some(comma) = trailing {
            if !comments(&text[comma + 1..close]).is_empty() {
                return Err(
                    "a trailing comment makes the last insertion's meaning ambiguous".to_string(),
                );
            }
            (comma + 1, format!(" {inserted},"))
        } else {
            (close, format!(", {inserted}"))
        };
        out.push((at, at, replacement));
    }
    Ok(out)
}

fn top_level_commas(text: &str, from: usize, to: usize) -> Vec<usize> {
    let bytes = text.as_bytes();
    let (mut depth, mut at) = (0i32, from);
    let mut out = Vec::new();
    while at < to {
        if let Some(end) = skip_opaque(bytes, at) {
            at = end;
            continue;
        }
        match bytes[at] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => out.push(at),
            _ => {}
        }
        at += 1;
    }
    out
}

fn parameter_piece_has_type(piece: &str) -> bool {
    let piece = strip_comments(piece);
    let piece = piece.trim();
    let word_end = piece
        .bytes()
        .position(|byte| !is_ident_byte(byte))
        .unwrap_or(piece.len());
    let word = &piece[..word_end];
    let rest = piece[word_end..].trim();
    let keyword = matches!(word, "chan" | "func" | "map" | "struct" | "interface");
    !word.is_empty()
        && !keyword
        && piece[word_end..].starts_with(|c: char| c.is_whitespace())
        && !rest.is_empty()
}

/// A receiver method may satisfy an imported interface even when no source interface declaration
/// or interface-typed call names it. gopls reports those relations from the concrete method, so
/// an empty result is the proof that extending this method does not alter an interface contract.
pub(crate) async fn receiver_interface_evidence(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    declaration_text: &str,
    name: &str,
    name_at: usize,
) -> Result<()> {
    let (line, character) = line_col_utf16(declaration_text, name_at);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
        .to_string();
    let answer = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/implementation",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        }),
    )
    .await
    .context("gopls could not list the method's interface implementations")?;
    let implementations = crate::refactor::lsp_locations(&answer, "interface implementations")?;
    anyhow::ensure!(
        implementations.is_empty(),
        "`{}` has interface implementation evidence at {}; interface dispatch cannot be reconciled",
        name,
        implementations
            .iter()
            .map(|(path, line, column)| format!("{}:{line}:{column}", path.display()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

/// gopls's complete reference answer for the function. The declaration must occur exactly once;
/// every other location must be current, inside the checkout and a direct call.
async fn function_reference_evidence(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    canonical_root: &Path,
    declaration_text: &str,
    decl: &Decl,
    receiver_type: Option<&str>,
) -> Result<(BTreeMap<PathBuf, String>, Vec<Call>)> {
    let (line, character) = line_col_utf16(declaration_text, decl.name_at);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character },
        "context": { "includeDeclaration": true },
    });
    let mut answer = serde_json::Value::Null;
    for attempt in 0..=crate::impact::COLD_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(crate::impact::COLD_WAIT).await;
        }
        answer = crate::tools::execute_lsp_query(
            remote,
            root,
            file,
            "textDocument/references",
            params.clone(),
        )
        .await
        .context("gopls could not list the function's references")?;
        if answer.as_array().is_some_and(|entries| !entries.is_empty()) {
            break;
        }
    }
    let entries = answer
        .as_array()
        .filter(|entries| !entries.is_empty())
        .with_context(|| {
            format!(
                "gopls listed no location, not even the declaration of `{}`: {answer}",
                decl.name
            )
        })?;
    let mut originals = BTreeMap::new();
    originals.insert(file.to_path_buf(), declaration_text.to_string());
    let mut calls = Vec::new();
    let mut declarations = 0usize;
    let mut seen = BTreeSet::new();
    for entry in entries {
        let number = |end: &str, key: &str| {
            entry
                .pointer(&format!("/range/{end}/{key}"))
                .and_then(|value| value.as_u64())
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value < u32::MAX)
        };
        let (Some(uri), Some(line), Some(column), Some(end_line), Some(end_column)) = (
            entry.get("uri").and_then(|value| value.as_str()),
            number("start", "line"),
            number("start", "character"),
            number("end", "line"),
            number("end", "character"),
        ) else {
            anyhow::bail!("a function reference is malformed: {entry}");
        };
        let uri = url::Url::parse(uri).context("a function reference has an invalid URI")?;
        anyhow::ensure!(
            uri.scheme() == "file" && uri.query().is_none() && uri.fragment().is_none(),
            "a function reference is not a plain file URI: {uri}"
        );
        let reported = uri
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("a function reference is not a local file URI: {uri}"))?;
        ensure_inside(canonical_root, &reported)?;
        let path = originals
            .keys()
            .find(|known| same_file(known, &reported))
            .cloned()
            .unwrap_or(reported);
        if !originals.contains_key(&path) {
            originals.insert(
                path.clone(),
                std::fs::read_to_string(&path).with_context(|| {
                    format!("cannot read {}; nothing was written", path.display())
                })?,
            );
        }
        let source = &originals[&path];
        let offset = offset_at(source, line, column)
            .filter(|at| ident_at(source, *at) == Some(decl.name.as_str()))
            .with_context(|| {
                format!(
                    "{}:{}:{} is not on `{}`; the reference is stale",
                    display(root, &path),
                    line + 1,
                    column + 1,
                    decl.name
                )
            })?;
        anyhow::ensure!(
            offset_at(source, end_line, end_column) == offset.checked_add(decl.name.len()),
            "{}:{}:{} does not span exactly `{}`; the reference is stale or malformed",
            display(root, &path),
            line + 1,
            column + 1,
            decl.name
        );
        anyhow::ensure!(
            seen.insert((path.clone(), offset)),
            "{}:{}:{} is listed more than once",
            display(root, &path),
            line + 1,
            column + 1
        );
        if same_file(&path, file) && offset == decl.name_at {
            declarations += 1;
            continue;
        }
        let at = format!("{}:{}:{}", display(root, &path), line + 1, column + 1);
        if let Some(receiver_type) = receiver_type {
            anyhow::ensure!(
                !declares_interface_method(source, &decl.name),
                "`{}` has an interface declaration in {}; interface dispatch cannot be reconciled",
                decl.name,
                display(root, &path)
            );
            receiver_selector_call(source, offset, receiver_type).with_context(|| {
                format!(
                    "`{}` is not a direct selector call at {at}; interface dispatch, method \
                     values and method expressions are not supported",
                    decl.name
                )
            })?;
        }
        let (open, close) = call_parens(source, offset).with_context(|| {
            format!(
                "`{}` is used as a value or another unsupported shape at {at}",
                decl.name
            )
        })?;
        calls.push(Call {
            path,
            at,
            open,
            close,
            args: split_list(&strip_comments(&source[open + 1..close])),
        });
    }
    anyhow::ensure!(
        declarations == 1,
        "gopls listed the declaration of `{}` {declarations} times instead of exactly once",
        decl.name
    );
    if receiver_type.is_some()
        && let Some(path) = interface_method_file(canonical_root, &decl.name)?
    {
        anyhow::bail!(
            "`{}` has an interface declaration in {}; interface dispatch cannot be reconciled",
            decl.name,
            display(root, &path)
        );
    }
    Ok((originals, calls))
}

/// A receiver method satisfying an interface cannot be extended without changing that interface
/// too. The reference response does not distinguish the interface-typed selector from a concrete
/// one, so a current interface declaration of the same method makes the whole plan uncertain.
fn declares_interface_method(text: &str, name: &str) -> bool {
    let bytes = text.as_bytes();
    for at in identifier_uses(text, 0, text.len(), "interface") {
        let open = skip_space(text, at + "interface".len());
        if bytes.get(open) != Some(&b'{') {
            continue;
        }
        let Some(close) = closing(text, open) else {
            continue;
        };
        let mut cursor = open + 1;
        while cursor < close {
            cursor = skip_space(text, cursor);
            if cursor >= close {
                break;
            }
            if let Some(end) = skip_opaque(bytes, cursor) {
                cursor = end;
            } else if matches!(bytes[cursor], b'(' | b'[' | b'{') {
                cursor = closing(text, cursor).map_or(close, |end| end + 1);
            } else if let Some(word) = ident_at(text, cursor) {
                let after = skip_space(text, cursor + word.len());
                if word == name && bytes.get(after) == Some(&b'(') {
                    return true;
                }
                cursor += word.len();
            } else {
                cursor += 1;
            }
        }
    }
    false
}

/// An interface obligation need not be reported by gopls as a reference to a concrete method.
/// Scan the checkout before extending a receiver method so an omitted interface declaration
/// cannot let an interface dispatch compile only after a destructive write.
fn interface_method_file(root: &Path, name: &str) -> Result<Option<PathBuf>> {
    fn visit(dir: &Path, name: &str) -> Result<Option<PathBuf>> {
        for entry in std::fs::read_dir(dir).with_context(|| {
            format!(
                "cannot read {} while checking interface obligations",
                dir.display()
            )
        })? {
            let entry = entry.with_context(|| {
                format!(
                    "cannot inspect {} while checking interface obligations",
                    dir.display()
                )
            })?;
            let path = entry.path();
            let kind = entry.file_type().with_context(|| {
                format!(
                    "cannot inspect {} while checking interface obligations",
                    path.display()
                )
            })?;
            if kind.is_dir() {
                if entry.file_name() != ".git"
                    && let Some(found) = visit(&path, name)?
                {
                    return Ok(Some(found));
                }
            } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "go")
            {
                let text = std::fs::read_to_string(&path).with_context(|| {
                    format!(
                        "cannot read {} while checking interface obligations",
                        path.display()
                    )
                })?;
                if declares_interface_method(&text, name) {
                    return Ok(Some(path));
                }
            }
        }
        Ok(None)
    }
    visit(root, name)
}

/// An unexported method name has package identity. Safe deletion therefore scans only regular Go
/// files in the declaring package for local interface obligations; an interface with the same
/// spelling in a different package is unrelated. Signature changes retain the broader recursive
/// scan above because narrowing that established planner is outside this helper's contract.
pub(crate) fn package_interface_method_file(file: &Path, name: &str) -> Result<Option<PathBuf>> {
    let directory = file
        .parent()
        .with_context(|| format!("{} has no containing package directory", file.display()))?;
    let declaration_text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let package = package_name(&declaration_text)
        .with_context(|| format!("cannot identify the Go package in {}", file.display()))?;
    for entry in std::fs::read_dir(directory).with_context(|| {
        format!(
            "cannot inspect {} while checking package interface obligations",
            directory.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "cannot inspect an entry in {} while checking package interface obligations",
                directory.display()
            )
        })?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "go") {
            continue;
        }
        let kind = entry.file_type().with_context(|| {
            format!(
                "cannot inspect {} while checking package interface obligations",
                path.display()
            )
        })?;
        anyhow::ensure!(
            !kind.is_symlink(),
            "linked Go source {} cannot be inspected for package interface obligations",
            path.display()
        );
        if !kind.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "cannot read {} while checking package interface obligations",
                path.display()
            )
        })?;
        if package_name(&text) == Some(package) && declares_interface_method(&text, name) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// The use of a receiver method must be a selector call. An interface member declaration and a
/// method value have no dot before the name; a selector whose receiver spells the receiver type
/// is a method expression. The latter is rejected even when a local happens to use that name:
/// no spelling-only check can prove that it is a value rather than a type.
fn receiver_selector_call(text: &str, name_at: usize, receiver_type: &str) -> Result<()> {
    let mut dot = name_at;
    while dot > 0 && matches!(text.as_bytes()[dot - 1], b' ' | b'\t') {
        dot -= 1;
    }
    anyhow::ensure!(
        dot > 0 && text.as_bytes()[dot - 1] == b'.',
        "the method name is not preceded by a selector"
    );
    let before = text[..dot - 1].trim_end();
    let receiver_type = receiver_type.trim_start_matches('*');
    let bare_type = before
        .strip_suffix(')')
        .and_then(|before| before.strip_suffix(receiver_type))
        .and_then(|before| before.strip_suffix("(*"))
        .is_some()
        || before == receiver_type
        || before.strip_suffix(receiver_type).is_some_and(|prefix| {
            prefix.is_empty()
                || !is_ident_byte(prefix.as_bytes().last().copied().unwrap_or_default())
        });
    anyhow::ensure!(
        !bare_type,
        "the selector receiver can be the receiver type `{receiver_type}`, a method expression"
    );
    Ok(())
}

/// A receiver is safe for insertion only when it binds one ordinary value name to one named type
/// or pointer-to-named-type. Parameterized receivers and receiver aliases need type information
/// beyond the source proof this adapter has, so they remain refused.
fn ordinary_receiver(receiver: &str) -> Result<Receiver> {
    let receiver = canonical(receiver);
    let binding_end = receiver
        .bytes()
        .position(|byte| !is_ident_byte(byte))
        .unwrap_or(receiver.len());
    let binding = &receiver[..binding_end];
    anyhow::ensure!(
        go_identifier(binding),
        "the receiver does not bind one ordinary name"
    );
    let ty = receiver[binding_end..].trim();
    let named = ty.strip_prefix('*').unwrap_or(ty);
    anyhow::ensure!(
        is_ident(named) && !named.is_empty(),
        "the receiver type `{}` is not an ordinary named value or pointer type",
        ty
    );
    Ok(Receiver {
        binding: binding.to_string(),
        ty: ty.to_string(),
    })
}

/// An error that says why, that nothing was written, and what stays open.
fn refusal(why: String) -> anyhow::Error {
    anyhow::anyhow!("{why}; nothing was written. {STILL_OPEN}")
}

/// Visibility and `async` are not parameters or the narrow result replacement supported here.
fn refuse_non_result_modifiers(modifiers: &Modifiers) -> Result<()> {
    if modifiers.visibility.is_some() {
        return Err(refusal(
            "a Go name is exported by its first letter, not by a modifier; use a rename to \
             change it"
                .to_string(),
        ));
    }
    if modifiers.asyncness.is_some() {
        return Err(refusal(
            "Go functions are neither async nor not".to_string(),
        ));
    }
    Ok(())
}

/// The new parameters as indices into `declared`: each declared parameter at most once, in the
/// new order; one left out is to be removed.
fn permutation(declared: &[GoParam], request: &[Param]) -> Result<Vec<usize>> {
    let mut order = Vec::with_capacity(request.len());
    for want in request {
        let name = match want {
            Param::Keep(name) => name,
            Param::Add { name, .. } => {
                return Err(refusal(format!(
                    "adding the parameter `{name}` is not supported: gopls {GOPLS_VERSION} \
                     refuses new parameters, and passing a new argument at every call site is not \
                     done by hand here"
                )));
            }
        };
        let at = declared
            .iter()
            .position(|d| &d.name == name)
            .with_context(|| {
                format!(
                    "no parameter named `{name}`; the declaration takes {}",
                    list_text(declared)
                )
            })?;
        anyhow::ensure!(!order.contains(&at), "`{name}` is listed twice");
        order.push(at);
    }
    anyhow::ensure!(
        order.len() < declared.len() || !is_subsequence(&order),
        "the requested order is the declared one; there is nothing to change"
    );
    Ok(order)
}

/// Whether the kept parameters keep their declared order.
fn is_subsequence(order: &[usize]) -> bool {
    order.windows(2).all(|w| w[0] < w[1])
}

/// The removed parameters' names, for a message: "`a`, `b`".
fn removed_names(declared: &[GoParam], removed: &[usize]) -> String {
    removed
        .iter()
        .map(|&i| format!("`{}`", declared[i].name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A call's arguments after the change: the kept ones in the new order. `arity` is the number of
/// parameters declared before the change, not the number kept. For a variadic function the
/// arguments from its last parameter on are one tail: kept last where the parameter is kept,
/// spread or not, and gone with it where it is removed.
fn permuted(args: &[String], order: &[usize], arity: usize, variadic: bool) -> Vec<String> {
    let tail = if variadic { arity - 1 } else { usize::MAX };
    let mut out = Vec::with_capacity(args.len());
    for &i in order {
        if i == tail {
            out.extend(args.iter().skip(tail).cloned());
        } else if let Some(a) = args.get(i) {
            out.push(a.clone());
        }
    }
    out
}

/// What the change would do to the program at run time, one line per place: a call whose
/// arguments cannot be matched to the declared parameters, two arguments evaluated the other way
/// round when either can have an effect the other sees, and a dropped argument whose evaluation
/// could do anything at all.
fn effect_hazards(
    name: &str,
    declared: &[GoParam],
    order: &[usize],
    variadic: bool,
    calls: &[Call],
) -> Vec<String> {
    let mut swapped = Vec::new();
    for (p, &later) in order.iter().enumerate() {
        for &earlier in &order[p + 1..] {
            if earlier < later {
                swapped.push((earlier, later));
            }
        }
    }
    let arity = declared.len();
    let mut out = Vec::new();
    for call in calls {
        let spread = call
            .args
            .last()
            .is_some_and(|a| a.trim_end().ends_with("..."));
        let fits = if variadic {
            call.args.len() + 1 >= arity && (!spread || call.args.len() == arity)
        } else {
            call.args.len() == arity && !spread
        };
        if !fits {
            out.push(format!(
                "{}: the call passes {} argument(s) and `{name}` declares {arity}, so what the \
                 reorder does to it cannot be checked",
                call.at,
                call.args.len()
            ));
            continue;
        }
        let kinds: Vec<ArgKind> = call.args.iter().map(|a| classify(a)).collect();
        for &(i, j) in &swapped {
            let independent = kinds[i] == ArgKind::Literal
                || kinds[j] == ArgKind::Literal
                || (kinds[i] == ArgKind::Place && kinds[j] == ArgKind::Place);
            if !independent {
                out.push(format!(
                    "{}: `{}` and `{}` would be evaluated in the opposite order",
                    call.at,
                    call.args[i].trim(),
                    call.args[j].trim()
                ));
            }
        }
        for (i, param) in declared.iter().enumerate() {
            if order.contains(&i) {
                continue;
            }
            // The removed variadic parameter takes the whole tail with it, a spread slice too.
            let dropped: Vec<&String> = if variadic && i == arity - 1 {
                call.args.iter().skip(i).collect()
            } else {
                call.args.get(i).into_iter().collect()
            };
            for arg in dropped {
                let value = arg.trim();
                if !droppable(value.strip_suffix("...").unwrap_or(value)) {
                    out.push(format!(
                        "{}: `{value}` is passed for the removed `{}` and would no longer be \
                         evaluated; only a literal or a plain variable can be dropped",
                        call.at, param.name
                    ));
                }
            }
        }
    }
    out
}

/// Whether evaluating the argument does nothing the program could notice: a number, string, rune
/// or function literal, or a plain variable or its address. Not a selector, which can dereference
/// a nil pointer, nor an index, a call, a receive, a conversion or an operator.
fn droppable(arg: &str) -> bool {
    let text = canonical(arg);
    let mut e = text.as_str();
    while e.starts_with('(') && closing(e, 0) == Some(e.len() - 1) {
        e = &e[1..e.len() - 1];
    }
    is_literal(e) || is_ident(e.strip_prefix('&').unwrap_or(e))
}

fn classify(arg: &str) -> ArgKind {
    let text = canonical(arg);
    let mut e = text.as_str();
    while e.starts_with('(') && closing(e, 0) == Some(e.len() - 1) {
        e = &e[1..e.len() - 1];
    }
    if is_literal(e) {
        ArgKind::Literal
    } else if is_place(e.strip_prefix('&').unwrap_or(e)) {
        ArgKind::Place
    } else {
        ArgKind::Effectful
    }
}

/// A literal by its syntax alone. Not `true`, `false` or `nil`, which a scope can redeclare.
fn is_literal(e: &str) -> bool {
    let s = e.as_bytes();
    if matches!(s.first(), Some(b'"' | b'`' | b'\'')) {
        return skip_opaque(s, 0) == Some(s.len());
    }
    let n = e
        .strip_prefix('-')
        .or_else(|| e.strip_prefix('+'))
        .unwrap_or(e);
    let nb = n.as_bytes();
    if nb.first().is_some_and(|b| b.is_ascii_digit())
        || (nb.first() == Some(&b'.') && nb.get(1).is_some_and(|b| b.is_ascii_digit()))
    {
        return nb.iter().enumerate().all(|(i, &b)| {
            b.is_ascii_alphanumeric()
                || b == b'_'
                || b == b'.'
                || (matches!(b, b'+' | b'-')
                    && i > 0
                    && if n.starts_with("0x") || n.starts_with("0X") {
                        matches!(nb[i - 1], b'p' | b'P')
                    } else {
                        matches!(nb[i - 1], b'e' | b'E')
                    })
        });
    }
    is_func_literal(e)
}

/// `func(…) … { … }` and nothing after it: evaluating it makes a closure and runs nothing.
fn is_func_literal(e: &str) -> bool {
    let Some(rest) = e.strip_prefix("func") else {
        return false;
    };
    let open = e.len() - rest.trim_start().len();
    if e.as_bytes().get(open) != Some(&b'(') {
        return false;
    }
    let Some(close) = closing(e, open) else {
        return false;
    };
    let body = body_open(e, close + 1);
    body.and_then(|b| closing(e, b)) == Some(e.len() - 1)
}

fn is_place(e: &str) -> bool {
    !e.is_empty() && e.split('.').all(is_ident)
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// A replacement of the byte range `start..end` of a file as it is.
type TextEdit = (usize, usize, String);

/// The text edits of gopls's answer, per file, as byte ranges of the file as it is. A file
/// outside the checkout, a file operation, overlapping edits, or an answer that is not
/// well-formed (a file's edits that are not a list, a position that does not fit the protocol's
/// unsigned 32-bit integers or the file) stop the change: a malformed part is never read as
/// "no edits" or as another position.
fn edits_by_file(
    canonical_root: &Path,
    edit: &serde_json::Value,
    originals: &mut BTreeMap<PathBuf, String>,
) -> Result<BTreeMap<PathBuf, Vec<TextEdit>>> {
    let mut raw: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    if let Some(changes) = edit.get("documentChanges").filter(|c| !c.is_null()) {
        let changes = changes.as_array().with_context(|| {
            format!("gopls's `documentChanges` is not a list: {changes}; nothing was written")
        })?;
        for change in changes {
            anyhow::ensure!(
                change.get("kind").is_none(),
                "gopls proposed creating, renaming or deleting a file ({}); nothing was written",
                change
            );
            let uri = change
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .context("a change in gopls's edit names no file; nothing was written")?;
            let list = change
                .get("edits")
                .and_then(|e| e.as_array())
                .cloned()
                .with_context(|| {
                    format!(
                        "gopls's change to {uri} has no list of edits: {change}; nothing was \
                         written"
                    )
                })?;
            raw.push((uri.to_string(), list));
        }
    } else if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, list) in changes {
            let list = list.as_array().cloned().with_context(|| {
                format!("gopls's edits for {uri} are not a list: {list}; nothing was written")
            })?;
            raw.push((uri.clone(), list));
        }
    } else {
        anyhow::bail!("gopls's answer is not a workspace edit: {edit}; nothing was written");
    }
    let mut out: BTreeMap<PathBuf, Vec<TextEdit>> = BTreeMap::new();
    for (uri, list) in raw {
        let path = PathBuf::from(crate::remote_fs::uri_to_path(&uri));
        ensure_inside(canonical_root, &path)?;
        // The same file under the spelling the references used, when it is one of them.
        let key = originals
            .keys()
            .find(|k| same_file(k, &path))
            .cloned()
            .unwrap_or_else(|| path.clone());
        if !originals.contains_key(&key) {
            let t = std::fs::read_to_string(&key)
                .with_context(|| format!("cannot read {}; nothing was written", key.display()))?;
            originals.insert(key.clone(), t);
        }
        let text = &originals[&key];
        let entry = out.entry(key.clone()).or_default();
        for e in list {
            // A position past `u32::MAX` is not a protocol position; truncating it would read
            // as a small one and splice another place in the file.
            let at = |p: &str| {
                let number = |field: &str| {
                    e.pointer(&format!("/range/{p}/{field}"))
                        .and_then(|v| v.as_u64())
                        .and_then(|v| u32::try_from(v).ok())
                };
                number("line")
                    .zip(number("character"))
                    .and_then(|(l, c)| offset_at(text, l, c))
            };
            let new_text = e.get("newText").and_then(|t| t.as_str());
            let (Some(s), Some(end), Some(new_text)) = (at("start"), at("end"), new_text) else {
                anyhow::bail!(
                    "an edit gopls proposed for {} is out of range or malformed: {e}; nothing was \
                     written",
                    key.display()
                );
            };
            anyhow::ensure!(s <= end, "gopls proposed an inverted range: {e}");
            entry.push((s, end, new_text.to_string()));
        }
        entry.sort_by_key(|(s, e, _)| (*s, *e));
        for pair in entry.windows(2) {
            anyhow::ensure!(
                pair[0].1 <= pair[1].0,
                "gopls proposed overlapping edits in {}; nothing was written",
                key.display()
            );
        }
    }
    Ok(out)
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

/// Refuses a path that is not an existing file under the checkout.
fn ensure_inside(canonical_root: &Path, path: &Path) -> Result<()> {
    let real = std::fs::canonicalize(path).with_context(|| {
        format!(
            "{} is not a file in the checkout; nothing was written",
            path.display()
        )
    })?;
    anyhow::ensure!(
        real.starts_with(canonical_root),
        "{} is outside the checkout {}; nothing was written",
        path.display(),
        canonical_root.display()
    );
    Ok(())
}

/// `text` with sorted, non-overlapping edits applied.
fn splice(text: &str, edits: &[TextEdit]) -> String {
    let mut out = text.to_string();
    for (s, e, t) in edits.iter().rev() {
        out.replace_range(*s..*e, t);
    }
    out
}

/// Where the byte at `at` of the old text is in the edited one; `None` when an edit replaces it.
fn map_offset(edits: &[TextEdit], at: usize) -> Option<usize> {
    let mut shifted = at as isize;
    for (s, e, t) in edits {
        if *e <= at {
            shifted += t.len() as isize - (*e - *s) as isize;
        } else if *s <= at {
            return None;
        }
    }
    usize::try_from(shifted).ok()
}

/// The byte offset of a 0-based line and UTF-16 column, the positions gopls speaks in.
fn offset_at(text: &str, line: u32, col: u32) -> Option<usize> {
    let mut start = 0usize;
    for _ in 0..line {
        start += text[start..].find('\n')? + 1;
    }
    let rest = &text[start..];
    let line_end = rest.find('\n').unwrap_or(rest.len());
    let end = line_end - usize::from(line_end < rest.len() && rest[..line_end].ends_with('\r'));
    let mut units = 0u32;
    for (i, ch) in rest[..end].char_indices() {
        if units >= col {
            return (units == col).then_some(start + i);
        }
        units += ch.len_utf16() as u32;
    }
    (units == col).then_some(start + end)
}

/// The 0-based line and UTF-16 column of a byte offset.
fn line_col_utf16(text: &str, offset: usize) -> (u32, u32) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() as u32;
    let col = before
        .rsplit('\n')
        .next()
        .map_or(0, |l| l.chars().map(|c| c.len_utf16() as u32).sum());
    (line, col)
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Where the string, rune or comment that starts at `i` ends; `None` when none starts there.
fn skip_opaque(s: &[u8], i: usize) -> Option<usize> {
    match s[i] {
        quote @ (b'"' | b'\'') => {
            let mut j = i + 1;
            while j < s.len() {
                match s[j] {
                    b'\\' => j += 2,
                    b'\n' => return Some(j),
                    c if c == quote => return Some(j + 1),
                    _ => j += 1,
                }
            }
            Some(s.len())
        }
        b'`' => Some(
            s[i + 1..]
                .iter()
                .position(|&c| c == b'`')
                .map_or(s.len(), |p| i + p + 2),
        ),
        b'/' if s.get(i + 1) == Some(&b'/') => Some(
            s[i..]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(s.len(), |p| i + p),
        ),
        b'/' if s.get(i + 1) == Some(&b'*') => Some(
            s[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(s.len(), |p| i + p + 4),
        ),
        _ => None,
    }
}

/// The offset of the bracket that closes the one at `open`.
fn closing(text: &str, open: usize) -> Option<usize> {
    let s = text.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                if depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The pieces of a list between its top-level commas, trimmed; a trailing comma adds none.
fn split_list(text: &str) -> Vec<String> {
    let s = text.as_bytes();
    let (mut depth, mut start, mut i) = (0i32, 0usize, 0usize);
    let mut out = Vec::new();
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(text[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let last = text[start..].trim();
    if !last.is_empty() {
        out.push(last.to_string());
    }
    out
}

/// `text` with every comment replaced by a space.
fn strip_comments(text: &str) -> String {
    let s = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut copied) = (0usize, 0usize);
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            if s[i] == b'/' {
                out.push_str(&text[copied..i]);
                out.push(' ');
                copied = end;
            }
            i = end;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

/// An expression or a type without comments, and with whitespace only where it separates two
/// words: what two spellings of the same code have in common.
fn canonical(text: &str) -> String {
    let text = strip_comments(text);
    let s = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut space) = (0usize, false);
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            out.push_str(&text[i..end]);
            i = end;
            space = false;
            continue;
        }
        if s[i].is_ascii_whitespace() {
            space = true;
            i += 1;
            continue;
        }
        let ch = text[i..].chars().next().unwrap_or(' ');
        if space && is_ident_byte(s[i]) && out.as_bytes().last().is_some_and(|b| is_ident_byte(*b))
        {
            out.push(' ');
        }
        space = false;
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// A parameter list on one line, for the report.
fn normalize(list: &str) -> String {
    strip_comments(list)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(',')
        .to_string()
}

fn list_text(params: &[GoParam]) -> String {
    params
        .iter()
        .map(|p| format!("{} {}", p.name, p.ty))
        .collect::<Vec<_>>()
        .join(", ")
}

fn suffix(results: &str) -> String {
    if results.is_empty() {
        String::new()
    } else {
        format!(" {results}")
    }
}

/// Skips whitespace and comments from `i`.
fn skip_space(text: &str, mut i: usize) -> usize {
    let s = text.as_bytes();
    while i < s.len() {
        if s[i].is_ascii_whitespace() {
            i += 1;
        } else if s[i] == b'/' && matches!(s.get(i + 1), Some(b'/' | b'*')) {
            i = skip_opaque(s, i).unwrap_or(s.len());
        } else {
            break;
        }
    }
    i
}

/// The declarations at the top level of a Go file, in order.
fn declarations(text: &str) -> Vec<Decl> {
    let s = text.as_bytes();
    let mut out = Vec::new();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'f' if depth == 0
                && text[i..].starts_with("func")
                && (i == 0 || !is_ident_byte(s[i - 1]))
                && !s.get(i + 4).is_some_and(|b| is_ident_byte(*b)) =>
            {
                if let Some(d) = header(text, i) {
                    i = d.close + 1;
                    out.push(d);
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The declaration whose `func` keyword is at `func_at`: `func (r T) Name[P any](…) results`.
/// `None` for a function literal or type, which has no name.
fn header(text: &str, func_at: usize) -> Option<Decl> {
    let s = text.as_bytes();
    if !text[func_at..].starts_with("func") {
        return None;
    }
    let mut i = skip_space(text, func_at + 4);
    let mut receiver = None;
    if s.get(i) == Some(&b'(') {
        let close = closing(text, i)?;
        receiver = Some(canonical(&text[i + 1..close]));
        i = skip_space(text, close + 1);
    }
    let name_at = i;
    while i < s.len() && is_ident_byte(s[i]) {
        i += 1;
    }
    if i == name_at || s[name_at].is_ascii_digit() {
        return None;
    }
    let name = text[name_at..i].to_string();
    i = skip_space(text, i);
    let mut generic = false;
    if s.get(i) == Some(&b'[') {
        generic = true;
        i = skip_space(text, closing(text, i)? + 1);
    }
    if s.get(i) != Some(&b'(') {
        return None;
    }
    let open = i;
    let close = closing(text, open)?;
    let end = body_open(text, close + 1).unwrap_or_else(|| {
        text[close + 1..]
            .find('\n')
            .map_or(text.len(), |n| close + 1 + n)
    });
    Some(Decl {
        func_at,
        name,
        name_at,
        receiver,
        generic,
        open,
        close,
        results: canonical(&text[close + 1..end]),
    })
}

/// The `{` that opens the body after a signature's parameters, skipping the braces of a
/// `struct{…}` or `interface{…}` result; `None` at the end of the line without one.
fn body_open(text: &str, from: usize) -> Option<usize> {
    let s = text.as_bytes();
    let mut i = from;
    while i < s.len() {
        if s[i] == b'/' && s.get(i + 1) == Some(&b'/') {
            return None;
        }
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'\n' | b';' => return None,
            b'{' => {
                let before = text[from..i].trim_end();
                if before.ends_with("struct") || before.ends_with("interface") {
                    i = closing(text, i)? + 1;
                    continue;
                }
                return Some(i);
            }
            b'(' | b'[' => {
                i = closing(text, i)? + 1;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The parameters of a declaration's list, flattened; an error for a list gopls cannot reorder
/// by name (unnamed or blank parameters).
fn parameters(list: &str) -> std::result::Result<Vec<GoParam>, String> {
    let mut pending: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut named = false;
    for piece in split_list(&strip_comments(list)) {
        let word_end = piece
            .bytes()
            .position(|b| !is_ident_byte(b))
            .unwrap_or(piece.len());
        let word = &piece[..word_end];
        let rest = piece[word_end..].trim();
        let keyword = matches!(word, "chan" | "func" | "map" | "struct" | "interface");
        let spaced = piece[word_end..].starts_with(|c: char| c.is_whitespace());
        if !word.is_empty() && !keyword && spaced && !rest.is_empty() {
            named = true;
            let ty = canonical(rest);
            for name in pending.drain(..) {
                out.push(GoParam {
                    name,
                    ty: ty.clone(),
                });
            }
            out.push(GoParam {
                name: word.to_string(),
                ty,
            });
        } else if !word.is_empty() && word_end == piece.len() && !keyword {
            pending.push(word.to_string());
        } else {
            return Err(format!("`{piece}` is an unnamed parameter"));
        }
    }
    if !pending.is_empty() {
        return Err(if named {
            format!("`{}` has no type", pending.join(", "))
        } else {
            "its parameters are unnamed".to_string()
        });
    }
    if out.iter().any(|p| p.name == "_") {
        return Err("it has a blank `_` parameter".to_string());
    }
    Ok(out)
}

/// The parentheses of the call whose callee's name starts at `at` (`f(…)`, `x.f(…)`,
/// `f[T](…)`); `None` when the name is not called there.
fn call_parens(text: &str, at: usize) -> Option<(usize, usize)> {
    let s = text.as_bytes();
    let inline = |mut i: usize| {
        while i < s.len() && matches!(s[i], b' ' | b'\t') {
            i += 1;
        }
        i
    };
    let mut i = at;
    while i < s.len() && is_ident_byte(s[i]) {
        i += 1;
    }
    i = inline(i);
    if s.get(i) == Some(&b'[') {
        i = inline(closing(text, i)? + 1);
    }
    if s.get(i) != Some(&b'(') {
        return None;
    }
    Some((i, closing(text, i)?))
}

/// Where each flattened parameter's name starts in the list between `open` and `close`, in the
/// order [`parameters`] gives them: the first word of every piece between top-level commas.
fn parameter_names_at(text: &str, open: usize, close: usize) -> Vec<usize> {
    let s = text.as_bytes();
    let mut starts = vec![open + 1];
    let (mut depth, mut i) = (0i32, open + 1);
    while i < close {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => starts.push(i + 1),
            _ => {}
        }
        i += 1;
    }
    starts
        .into_iter()
        .map(|start| skip_space(text, start))
        .filter(|&at| at < close && is_ident_byte(s[at]))
        .collect()
}

/// The whole identifier that starts at `at`, if one does.
fn ident_at(text: &str, at: usize) -> Option<&str> {
    let s = text.as_bytes();
    if at >= s.len() || !is_ident_byte(s[at]) || (at > 0 && is_ident_byte(s[at - 1])) {
        return None;
    }
    let end = s[at..]
        .iter()
        .position(|&b| !is_ident_byte(b))
        .map_or(s.len(), |n| at + n);
    Some(&text[at..end])
}

/// Where `name` is written as a word in `text[from..to]`, strings and comments aside, unless it
/// follows a dot (a field, a method or a package member, never a local). Anything else counts,
/// a struct literal's key or a label too: this is the proof that a name is unused, and it errs
/// towards a use.
fn identifier_uses(text: &str, from: usize, to: usize, name: &str) -> Vec<usize> {
    let s = text.as_bytes();
    let mut out = Vec::new();
    let mut i = from;
    while i < to {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        if let Some(word) = ident_at(text, i) {
            if word == name && !text[..i].trim_end().ends_with('.') {
                out.push(i);
            }
            i += word.len();
            continue;
        }
        i += 1;
    }
    out
}

/// `file:line:column`, 1-based, of a byte offset, as a message names a place.
fn position(root: &Path, file: &Path, text: &str, offset: usize) -> String {
    let (l, c) = line_col_utf16(text, offset);
    format!("{}:{}:{}", display(root, file), l + 1, c + 1)
}

/// The uses gopls knows of the parameter declared at `at`, as offsets in the body. The question
/// includes the declaration, so that an answer from a server that has not loaded the package
/// (nothing at all) is told apart from "no use": that one is asked again a few times, then
/// refused.
async fn parameter_uses(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    name: &str,
    at: usize,
    body: (usize, usize),
) -> Result<Vec<usize>> {
    let (line, character) = line_col_utf16(text, at);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character },
        "context": { "includeDeclaration": true },
    });
    let mut answer = serde_json::Value::Null;
    for attempt in 0..=crate::impact::COLD_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(crate::impact::COLD_WAIT).await;
        }
        answer = crate::tools::execute_lsp_query(
            remote,
            root,
            file,
            "textDocument/references",
            params.clone(),
        )
        .await
        .context("the references could not be listed")?;
        if answer.as_array().is_some_and(|a| !a.is_empty()) {
            break;
        }
    }
    parameter_evidence(&answer, file, text, name, at, body)
}

/// The uses in a `textDocument/references` answer for the parameter `name` declared at `at`,
/// which must be well-formed and complete to count: a list of locations in `file`, each on the
/// name as the file reads now, the declaration among them, and every other one inside the body.
/// Anything else is an error, never "unused": a stale position, a location elsewhere, a missing
/// declaration or an empty answer says the server was not answering about this parameter.
fn parameter_evidence(
    answer: &serde_json::Value,
    file: &Path,
    text: &str,
    name: &str,
    at: usize,
    body: (usize, usize),
) -> Result<Vec<usize>> {
    let entries = answer
        .as_array()
        .filter(|a| !a.is_empty())
        .with_context(|| {
            format!("the answer lists no location, not even the declaration: {answer}")
        })?;
    let mut declared = false;
    let mut uses = Vec::new();
    for entry in entries {
        let number = |end: &str, key: &str| {
            entry
                .pointer(&format!("/range/{end}/{key}"))
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v < u32::MAX)
        };
        let (Some(uri), Some(l), Some(c), Some(el), Some(ec)) = (
            entry.get("uri").and_then(|u| u.as_str()),
            number("start", "line"),
            number("start", "character"),
            number("end", "line"),
            number("end", "character"),
        ) else {
            anyhow::bail!("a location is malformed: {entry}");
        };
        let uri = url::Url::parse(uri).context("a parameter location has an invalid URI")?;
        anyhow::ensure!(
            uri.scheme() == "file" && uri.query().is_none() && uri.fragment().is_none(),
            "a parameter location is not a plain file URI: {uri}"
        );
        let path = uri
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("a parameter location is not a local file URI: {uri}"))?;
        anyhow::ensure!(
            same_file(&path, file),
            "a use in {} is outside the function; the answer is not about this parameter",
            path.display()
        );
        let offset = offset_at(text, l, c)
            .filter(|&o| ident_at(text, o) == Some(name))
            .with_context(|| {
                format!(
                    "the location {}:{} is not on `{name}`; the file may have changed since it \
                     was read",
                    l + 1,
                    c + 1
                )
            })?;
        anyhow::ensure!(
            offset_at(text, el, ec) == offset.checked_add(name.len()),
            "the location {}:{} does not span exactly `{name}`; its range is stale or malformed",
            l + 1,
            c + 1
        );
        if offset == at {
            declared = true;
        } else {
            anyhow::ensure!(
                body.0 < offset && offset < body.1,
                "the location {}:{} is outside the function's body",
                l + 1,
                c + 1
            );
            uses.push(offset);
        }
    }
    anyhow::ensure!(
        declared,
        "the answer does not include the parameter's own declaration, so it is not about it"
    );
    Ok(uses)
}

/// Every comment of a Go text, in order and as written.
fn comments(text: &str) -> Vec<&str> {
    let s = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            if s[i] == b'/' {
                out.push(&text[i..end]);
            }
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(list: &str) -> Vec<(String, String)> {
        parameters(list)
            .unwrap()
            .into_iter()
            .map(|p| (p.name, p.ty))
            .collect()
    }

    #[test]
    fn grouped_parameters_are_flattened_with_their_types() {
        assert_eq!(
            params(
                "a, b int, label string, fn func(x, y int) (int, error), xs ...[]map[string]int"
            ),
            vec![
                ("a".into(), "int".into()),
                ("b".into(), "int".into()),
                ("label".into(), "string".into()),
                ("fn".into(), "func(x,y int)(int,error)".into()),
                ("xs".into(), "...[]map[string]int".into()),
            ]
        );
        assert_eq!(
            params("ch <-chan int, /* note */ c chan int,\n\tp *pkg.T,\n"),
            vec![
                ("ch".into(), "<-chan int".into()),
                ("c".into(), "chan int".into()),
                ("p".into(), "*pkg.T".into()),
            ]
        );
        assert!(parameters("int, string").unwrap_err().contains("unnamed"));
        assert!(parameters("chan int").unwrap_err().contains("unnamed"));
        assert!(parameters("_ int, b string").unwrap_err().contains("blank"));
        assert!(parameters("pkg.T").is_err());
        assert!(parameters("").unwrap().is_empty());
    }

    #[test]
    fn declarations_are_found_and_literals_are_not() {
        let text = "package p\n\n// func Fake(a int)\nvar f = func(a, b int) int { return a }\n\
                    type F func(a int) int\n\
                    func (s *S) M(x int, y string) (n int, err error) {\n\treturn\n}\n\
                    func G[T any, U any](t T, u U) struct{ a int } { return struct{ a int }{} }\n\
                    func Asm(x, y int) int\n\
                    func main() { go func(a int) {}(1); s := \"func X(\" ; _ = s }\n";
        let decls = declarations(text);
        let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["M", "G", "Asm", "main"]);
        let m = &decls[0];
        assert_eq!(m.receiver.as_deref(), Some("s*S"));
        assert_eq!(m.results, "(n int,err error)");
        assert_eq!(&text[m.open + 1..m.close], "x int, y string");
        assert!(decls[1].generic);
        assert_eq!(decls[1].results, "struct{a int}");
        assert_eq!(decls[2].results, "int");
        assert!(!decls[3].generic && decls[3].results.is_empty());
    }

    #[test]
    fn package_type_shadowing_is_found_across_comments_and_groups() {
        let source = r#"// build comment
package p

// type fake string
const text = "type quoted int"
type Direct = string
type /* before group */ (
    // before name
    int64 /* after name */ = interface{}
    byte = struct {
        field int
    }
)
func local() { type string = interface{} }
"#;
        assert_eq!(package_name(source), Some("p"));
        assert_eq!(
            package_type_names(source),
            vec![
                "Direct".to_string(),
                "int64".to_string(),
                "byte".to_string()
            ]
        );

        let directory = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(directory.path()).unwrap();
        let file = root.join("a.go");
        std::fs::write(&file, "package p\nfunc F() int { return 1 }\n").unwrap();
        std::fs::write(root.join("shadow.go"), source).unwrap();
        std::fs::write(
            root.join("external_test.go"),
            "package p_test\ntype string = interface{}\n",
        )
        .unwrap();

        ensure_predeclared_types_unshadowed(&root, &file, &["string"]).unwrap();
        for shadowed in ["int64", "byte"] {
            let error = ensure_predeclared_types_unshadowed(&root, &file, &[shadowed])
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("declared as a package type") && error.contains("shadow.go"),
                "{shadowed}: {error}"
            );
        }
    }

    #[test]
    fn only_named_value_or_pointer_receivers_can_be_extended() {
        assert_eq!(
            ordinary_receiver("meter Meter").unwrap(),
            Receiver {
                binding: "meter".into(),
                ty: "Meter".into()
            }
        );
        assert_eq!(
            ordinary_receiver("meter *Meter").unwrap(),
            Receiver {
                binding: "meter".into(),
                ty: "*Meter".into()
            }
        );
        for receiver in [
            "_ Meter",
            "meter Meter[T]",
            "meter pkg.Meter",
            "meter *pkg.Meter",
            "left, right Meter",
        ] {
            assert!(ordinary_receiver(receiver).is_err(), "{receiver}");
        }
        let direct = "meter.Add(1)";
        receiver_selector_call(direct, direct.find("Add").unwrap(), "Meter").unwrap();
        for source in ["Add(1)", "Meter.Add(1)", "(*Meter).Add(1)"] {
            assert!(
                receiver_selector_call(source, source.find("Add").unwrap(), "Meter").is_err(),
                "{source}"
            );
        }
        assert!(declares_interface_method(
            "type I interface { Add(x int) }",
            "Add"
        ));
        assert!(!declares_interface_method(
            "type I interface { Other(x int) }",
            "Add"
        ));
    }

    #[test]
    fn arguments_are_split_and_classified() {
        let text = "x := f(a, g(b, c), \"s,)\", `r,`, '(', // c,\n\tt.u, &v, -1.5e-3, func(a int) {}, h(),\n)";
        let at = text.find("f(").unwrap();
        let (open, close) = call_parens(text, at).unwrap();
        assert_eq!(close, text.len() - 1);
        let args = split_list(&strip_comments(&text[open + 1..close]));
        assert_eq!(
            args,
            vec![
                "a",
                "g(b, c)",
                "\"s,)\"",
                "`r,`",
                "'('",
                "t.u",
                "&v",
                "-1.5e-3",
                "func(a int) {}",
                "h()"
            ]
        );
        let kinds: Vec<ArgKind> = args.iter().map(|a| classify(a)).collect();
        use ArgKind::*;
        assert_eq!(
            kinds,
            vec![
                Place, Effectful, Literal, Literal, Literal, Place, Place, Literal, Literal,
                Effectful
            ]
        );
        assert_eq!(classify("(x)"), Place);
        assert_eq!(classify("<-ch"), Effectful);
        assert_eq!(classify("a[i]"), Effectful);
        assert_eq!(classify("func() {}()"), Effectful);
        assert!(call_parens("f := Sub\n", 5).is_none());
        assert!(call_parens("Pair[int, string](1, \"s\")", 0).is_some());
    }

    fn call(args: &[&str]) -> Call {
        Call {
            path: PathBuf::from("m.go"),
            at: "m.go:1:1".into(),
            open: 0,
            close: 0,
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }

    fn declared(names: &[&str]) -> Vec<GoParam> {
        names
            .iter()
            .map(|n| GoParam {
                name: n.to_string(),
                ty: if n.starts_with("xs") { "...int" } else { "int" }.to_string(),
            })
            .collect()
    }

    #[test]
    fn a_reorder_that_changes_evaluation_order_is_a_hazard() {
        let d = declared(&["a", "b", "c"]);
        let order = [1, 0, 2];
        let safe = [
            call(&["x", "y", "g()"]),
            call(&["1", "g()", "h()"]),
            call(&["x.f", "&y", "2"]),
        ];
        assert!(effect_hazards("f", &d, &order, false, &safe).is_empty());
        let unsafe_ = [call(&["g()", "h()", "1"]), call(&["x", "g()", "1"])];
        let found = effect_hazards("f", &d, &order, false, &unsafe_);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("`g()` and `h()`"), "{found:?}");
        let short = effect_hazards("f", &d, &order, false, &[call(&["g()"])]);
        assert!(short[0].contains("cannot be checked"), "{short:?}");
        let v = declared(&["a", "b", "xs"]);
        assert!(effect_hazards("f", &v, &order, true, &[call(&["1", "2", "3", "4"])]).is_empty());
        assert!(effect_hazards("f", &v, &order, true, &[call(&["1", "2", "ys..."])]).is_empty());
        assert!(!effect_hazards("f", &v, &order, true, &[call(&["1", "ys..."])]).is_empty());
    }

    /// `true`, `false` and `nil` are predeclared identifiers that a scope can redeclare
    /// (`true := 1; f(true, bump(&true))`), so they are variables as far as their spelling goes,
    /// and a reorder against a call is a hazard. Literals that cannot be redeclared still are.
    #[test]
    fn predeclared_names_are_not_trusted_as_literals() {
        use ArgKind::*;
        for name in ["true", "false", "nil", "(true)", "&nil"] {
            assert_eq!(classify(name), Place, "{name}");
        }
        for literal in ["1", "0x1F", "1_000", "2.5e+3", "'x'", "\"true\"", "`nil`"] {
            assert_eq!(classify(literal), Literal, "{literal}");
        }
        let d = declared(&["a", "b"]);
        for name in ["true", "false", "nil"] {
            let bump = format!("bump(&{name})");
            let found = effect_hazards("f", &d, &[1, 0], false, &[call(&[name, &bump])]);
            assert_eq!(found.len(), 1, "{name}: {found:?}");
            assert!(
                found[0].contains(&format!("`{name}` and `{bump}`")),
                "{found:?}"
            );
        }
        // Two reads, or a read beside a real literal, stay independent.
        assert!(effect_hazards("f", &d, &[1, 0], false, &[call(&["true", "nil"])]).is_empty());
        assert_eq!(
            effect_hazards("f", &d, &[1, 0], false, &[call(&["false", "g()"])]).len(),
            1
        );
        assert!(effect_hazards("f", &d, &[1, 0], false, &[call(&["1", "g()"])]).is_empty());
    }

    #[test]
    fn permutations_keep_variadic_tails_in_place() {
        let args: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            permuted(&args[..3], &[2, 0, 1], 3, false),
            vec!["c", "a", "b"]
        );
        assert_eq!(
            permuted(&args, &[1, 0, 2], 3, true),
            vec!["b", "a", "c", "d"]
        );
        assert_eq!(permuted(&args[..2], &[1, 0, 2], 3, true), vec!["b", "a"]);
        assert_eq!(
            permuted(&args[..3], &[1, 0, 2], 3, true),
            vec!["b", "a", "c"]
        );
    }

    /// The arguments after a removal come from the declared arity, not from how many
    /// parameters are left: with three declared and one kept, the tail starts at the third
    /// argument, not the first.
    #[test]
    fn removals_keep_the_declared_arity_and_the_variadic_tail() {
        let args: Vec<String> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(permuted(&args[..3], &[2, 0], 3, false), vec!["c", "a"]);
        assert_eq!(permuted(&args[..3], &[], 3, false), Vec::<String>::new());
        // `f(a, b, xs ...int)` without `a`: the tail stays whole and last.
        assert_eq!(permuted(&args, &[1, 2], 3, true), vec!["b", "c", "d", "e"]);
        assert_eq!(permuted(&args[..2], &[1, 2], 3, true), vec!["b"]);
        let spread = vec!["a".to_string(), "b".to_string(), "ys...".to_string()];
        assert_eq!(permuted(&spread, &[1, 2], 3, true), vec!["b", "ys..."]);
        // Without `xs`: the whole tail goes, however long, spread or not.
        assert_eq!(permuted(&args, &[1, 0], 3, true), vec!["b", "a"]);
        assert_eq!(permuted(&spread, &[0], 3, true), vec!["a"]);
    }

    #[test]
    fn dropped_arguments_must_do_nothing_when_evaluated() {
        use ArgKind::*;
        for pure in [
            "1",
            "\"s\"",
            "'r'",
            "x",
            "&x",
            "(x)",
            "nil",
            "func() { g() }",
            "x /* c */",
        ] {
            assert!(droppable(pure), "{pure}");
        }
        // A selector is a `Place` for a reorder, but it can dereference nil and panic.
        assert_eq!(classify("p.n"), Place);
        for effect in [
            "p.n", "&p.n", "g()", "<-ch", "xs[i]", "i+1", "T(x)", "*p", "-x", "x.(T)", "[]int{1}",
        ] {
            assert!(!droppable(effect), "{effect}");
        }
        let d = declared(&["a", "b", "c"]);
        let safe = [call(&["g()", "1", "x"]), call(&["g()", "&y", "\"s\""])];
        assert!(effect_hazards("f", &d, &[0], false, &safe).is_empty());
        let found = effect_hazards(
            "f",
            &d,
            &[0],
            false,
            &[call(&["1", "h()", "p.n"]), call(&["x", "<-ch", "a[0]"])],
        );
        assert_eq!(found.len(), 4, "{found:?}");
        assert!(
            found[0].contains("`h()` is passed for the removed `b`")
                && found[1].contains("`p.n` is passed for the removed `c`")
                && found[2].contains("`<-ch`")
                && found[3].contains("`a[0]`"),
            "{found:?}"
        );
        // A reorder of what is kept is checked too.
        let both = effect_hazards("f", &d, &[2, 0], false, &[call(&["g()", "1", "h()"])]);
        assert_eq!(both.len(), 1, "{both:?}");
        assert!(both[0].contains("opposite order"), "{both:?}");
        // A pair passed as the arguments, and too few or too many: the arity cannot be matched.
        for args in [
            &["two()"][..],
            &["1", "2"],
            &["1", "2", "3", "4"],
            &["1", "2", "xs..."],
        ] {
            let odd = effect_hazards("f", &d, &[0], false, &[call(args)]);
            assert!(odd[0].contains("cannot be checked"), "{args:?}: {odd:?}");
        }
    }

    #[test]
    fn a_removed_variadic_parameter_takes_its_whole_tail() {
        let v = declared(&["a", "xs"]);
        let calls = [
            call(&["1"]),
            call(&["1", "2", "x"]),
            call(&["1", "ys..."]),
            call(&["g()", "x"]),
        ];
        assert!(effect_hazards("f", &v, &[0], true, &calls).is_empty());
        let found = effect_hazards(
            "f",
            &v,
            &[0],
            true,
            &[call(&["1", "2", "g()"]), call(&["1", "p.xs..."])],
        );
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found[0].contains("`g()` is passed for the removed `xs`")
                && found[1].contains("`p.xs...` is passed for the removed `xs`"),
            "{found:?}"
        );
        // Kept, the tail is not dropped, and a removed fixed parameter is checked alone.
        let kept = effect_hazards("f", &v, &[1], true, &[call(&["x", "g()", "h()"])]);
        assert!(kept.is_empty(), "{kept:?}");
        let fixed = effect_hazards("f", &v, &[1], true, &[call(&["g()", "1"])]);
        assert!(
            fixed[0].contains("`g()` is passed for the removed `a`"),
            "{fixed:?}"
        );
    }

    #[test]
    fn requests_that_are_not_permutations_are_refused_with_the_open_requirement() {
        let d = declared(&["a", "b"]);
        let keep = |n: &str| Param::Keep(n.to_string());
        assert_eq!(
            permutation(&d, &[keep("b"), keep("a")]).unwrap(),
            vec![1, 0]
        );
        // A parameter left out is to be removed, alone, with a reorder, or all of them.
        assert_eq!(permutation(&d, &[keep("b")]).unwrap(), vec![1]);
        assert_eq!(permutation(&d, &[keep("a")]).unwrap(), vec![0]);
        assert_eq!(permutation(&d, &[]).unwrap(), Vec::<usize>::new());
        let three = declared(&["a", "b", "c"]);
        assert_eq!(
            permutation(&three, &[keep("c"), keep("a")]).unwrap(),
            vec![2, 0]
        );
        assert!(is_subsequence(&[0, 2]) && !is_subsequence(&[2, 0]));
        assert_eq!(removed_names(&three, &[0, 2]), "`a`, `c`");
        let added = permutation(
            &d,
            &[
                keep("a"),
                keep("b"),
                Param::Add {
                    name: "c".into(),
                    ty: "int".into(),
                    value: "0".into(),
                },
            ],
        )
        .unwrap_err()
        .to_string();
        assert!(added.contains("adding the parameter `c`"), "{added}");
        assert!(permutation(&d, &[keep("a"), keep("b")]).is_err());
        assert!(permutation(&d, &[keep("b"), keep("b")]).is_err());
        assert!(permutation(&d, &[keep("z"), keep("a")]).is_err());
        for m in [
            Modifiers {
                visibility: Some("pub".into()),
                ..Default::default()
            },
            Modifiers {
                asyncness: Some(true),
                ..Default::default()
            },
        ] {
            let err = refuse_non_result_modifiers(&m).unwrap_err().to_string();
            assert!(err.contains(STILL_OPEN), "{err}");
        }
    }

    #[test]
    fn parameter_names_and_body_uses_are_found_by_word() {
        let text = "func F(a, /* b */ b int, c func(b int) int,\n\tdd ...string) (b2 int) {\n\
                    \tx := \"b\" + `b` // b\n\ty := s.b + T{b: 1}.b\n\treturn func() int { return b }()\n}\n";
        let open = text.find('(').unwrap();
        let close = closing(text, open).unwrap();
        let names: Vec<&str> = parameter_names_at(text, open, close)
            .into_iter()
            .map(|at| ident_at(text, at).unwrap())
            .collect();
        assert_eq!(names, vec!["a", "b", "c", "dd"]);
        assert_eq!(parameter_names_at("f()", 1, 2), Vec::<usize>::new());
        assert_eq!(parameter_names_at("f(a int,\n)", 1, 9).len(), 1);
        let body = body_open(text, close + 1).unwrap();
        let end = closing(text, body).unwrap();
        // The struct literal's key counts, the field reads after a dot and the texts do not.
        let uses: Vec<usize> = identifier_uses(text, body, end, "b");
        let lines: Vec<u32> = uses.iter().map(|&o| line_col_utf16(text, o).0).collect();
        assert_eq!(lines, vec![3, 4], "{uses:?}");
        assert!(identifier_uses(text, body, end, "a").is_empty());
        assert!(identifier_uses(text, body, end, "b2").is_empty());
        assert_eq!(identifier_uses(text, body, end, "x").len(), 1);
        assert_eq!(ident_at(text, text.find("dd").unwrap() + 1), None);
        assert_eq!(ident_at(text, text.len()), None);
        assert_eq!(
            position(Path::new("/r"), Path::new("/r/a.go"), text, body),
            "a.go:2:25"
        );
    }

    /// The parameter's references count as proof only when they are well-formed, current and
    /// complete: anything else is an error, and no answer at all is not "unused".
    #[test]
    fn parameter_references_prove_nothing_unless_complete_and_current() {
        let ws = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
        let root = std::fs::canonicalize(ws.path()).unwrap();
        let file = root.join("a.go");
        let other = root.join("b.go");
        let text = "package a\n\nfunc F(a, b int) int {\n\treturn a + b\n}\n\nvar b = 1\n";
        std::fs::write(&file, text).unwrap();
        std::fs::write(&other, "package a\n").unwrap();
        let open = text.find("F(").unwrap() + 1;
        let close = closing(text, open).unwrap();
        let at = parameter_names_at(text, open, close)[1];
        let body_at = body_open(text, close + 1).unwrap();
        let body = (body_at, closing(text, body_at).unwrap());
        let loc = |path: &Path, line: u64, character: u64| {
            serde_json::json!({ "uri": format!("file://{}", path.display()),
                "range": { "start": { "line": line, "character": character },
                           "end": { "line": line, "character": character + 1 } } })
        };
        let evidence =
            |answer: serde_json::Value| parameter_evidence(&answer, &file, text, "b", at, body);
        // Unused: the declaration alone. Used: the declaration and the read in the body.
        assert_eq!(
            evidence(serde_json::json!([loc(&file, 2, 10)])).unwrap(),
            Vec::<usize>::new()
        );
        let used = evidence(serde_json::json!([loc(&file, 2, 10), loc(&file, 3, 12)])).unwrap();
        assert_eq!(used, vec![text.find("+ b").unwrap() + 2]);
        let refused = |answer: serde_json::Value| {
            evidence(answer.clone())
                .map(|ok| format!("accepted {answer} as {ok:?}"))
                .unwrap_err()
                .to_string()
        };
        for (answer, said) in [
            (serde_json::Value::Null, "no location"),
            (serde_json::json!([]), "no location"),
            (serde_json::json!({ "uri": "x" }), "no location"),
            (serde_json::json!([{ "uri": 7 }]), "malformed"),
            (
                serde_json::json!([{ "uri": "file:///a.go", "range": { "start": { "line": -1, "character": 0 } } }]),
                "malformed",
            ),
            (
                serde_json::json!([loc(&other, 0, 0)]),
                "outside the function",
            ),
            (serde_json::json!([loc(&file, 2, 11)]), "not on `b`"),
            (serde_json::json!([loc(&file, 40, 0)]), "not on `b`"),
            (
                serde_json::json!([loc(&file, 2, 10), loc(&file, 6, 4)]),
                "outside the function's body",
            ),
            (serde_json::json!([loc(&file, 3, 12)]), "own declaration"),
        ] {
            let err = refused(answer.clone());
            assert!(err.contains(said), "{answer}: {err}");
        }
    }

    #[test]
    fn comments_are_listed_as_written_and_strings_are_not_comments() {
        let text = "f(a /* x */, \"// no\", '/') // tail\n/* multi\nline */ g(`/*`)\n";
        assert_eq!(
            comments(text),
            vec!["/* x */", "// tail", "/* multi\nline */"]
        );
        assert!(comments("f(a, b)").is_empty());
    }

    #[test]
    fn positions_are_utf16_and_edits_map_through() {
        let text = "a := \"é𝄞\"; f(x, y)\n";
        let at = text.find("f(").unwrap();
        let (l, c) = line_col_utf16(text, at);
        assert_eq!((l, c), (0, 12));
        assert_eq!(offset_at(text, l, c), Some(at));
        assert_eq!(offset_at(text, 0, 8), None, "inside a surrogate pair");
        assert_eq!(offset_at(text, 3, 0), None);
        let crlf = "ab\r\ncd\n";
        assert_eq!(offset_at(crlf, 0, 2), Some(2), "line end precedes CR");
        assert_eq!(offset_at(crlf, 0, 3), None, "CR is not a position");
        assert_eq!(offset_at(crlf, 1, 0), Some(4));
        assert_eq!(offset_at(crlf, 2, 0), Some(7), "empty final line");
        assert_eq!(line_col_utf16(crlf, 2), (0, 2));
        assert_eq!(line_col_utf16(crlf, 4), (1, 0));
        let emoji = "😀\r\n";
        assert_eq!(offset_at(emoji, 0, 0), Some(0));
        assert_eq!(offset_at(emoji, 0, 1), None, "inside a surrogate pair");
        assert_eq!(
            offset_at(emoji, 0, 2),
            Some(4),
            "emoji occupies two UTF-16 units"
        );
        assert_eq!(offset_at(emoji, 0, 3), None, "CR is not a position");
        let open = at + 1;
        let edits = vec![
            (open + 1, open + 2, "yy".to_string()),
            (0, 1, "bb".to_string()),
        ];
        let mut sorted = edits.clone();
        sorted.sort_by_key(|(s, e, _)| (*s, *e));
        assert_eq!(map_offset(&sorted, open), Some(open + 1));
        assert_eq!(map_offset(&sorted, open + 1), None);
        assert_eq!(splice(text, &sorted), "bb := \"é𝄞\"; f(yy, y)\n");
    }

    #[test]
    fn edits_outside_the_checkout_or_moving_files_are_refused() {
        let ws = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
        let root = std::fs::canonicalize(ws.path()).unwrap();
        std::fs::write(root.join("a.go"), "package a\n").unwrap();
        let outside = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
        let other = std::fs::canonicalize(outside.path()).unwrap().join("b.go");
        std::fs::write(&other, "package b\n").unwrap();
        let edit = |uri: String| {
            serde_json::json!({ "documentChanges": [ {
                "textDocument": { "uri": uri, "version": 1 },
                "edits": [ { "range": { "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 7 } }, "newText": "package" } ]
            } ] })
        };
        let mut originals = BTreeMap::new();
        let err = edits_by_file(
            &root,
            &edit(format!("file://{}", other.display())),
            &mut originals,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("outside the checkout"), "{err}");
        let inside = edits_by_file(
            &root,
            &edit(format!("file://{}", root.join("a.go").display())),
            &mut originals,
        )
        .unwrap();
        assert_eq!(
            inside[&root.join("a.go")],
            vec![(0, 7, "package".to_string())]
        );
        let moves = serde_json::json!({ "documentChanges": [ { "kind": "create", "uri": "file:///x.go" } ] });
        assert!(edits_by_file(&root, &moves, &mut originals).is_err());
        let overlapping = serde_json::json!({ "changes": {
            format!("file://{}", root.join("a.go").display()): [
                { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 5 } }, "newText": "x" },
                { "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 7 } }, "newText": "y" }
            ] } });
        assert!(
            edits_by_file(&root, &overlapping, &mut originals)
                .unwrap_err()
                .to_string()
                .contains("overlapping")
        );
        assert!(edits_by_file(&root, &serde_json::json!([]), &mut originals).is_err());
    }

    /// A malformed answer is an error, never "no edits" for a file and never another position:
    /// a missing or non-list `edits`, a non-list `documentChanges` or `changes` entry, and a
    /// line or column past `u32::MAX`, which a cast would have wrapped to a small number.
    #[test]
    fn malformed_or_oversized_edits_are_refused_not_defaulted() {
        let ws = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
        let root = std::fs::canonicalize(ws.path()).unwrap();
        let file = root.join("a.go");
        std::fs::write(&file, "package a\n\nfunc F(x, y int) {}\n").unwrap();
        let uri = format!("file://{}", file.display());
        let mut originals = BTreeMap::new();
        let refused = |edit: serde_json::Value, originals: &mut BTreeMap<PathBuf, String>| {
            edits_by_file(&root, &edit, originals)
                .map(|ok| format!("accepted as {ok:?}"))
                .unwrap_err()
                .to_string()
        };
        let shapes = [
            serde_json::json!({ "documentChanges": [ { "textDocument": { "uri": uri } } ] }),
            serde_json::json!({ "documentChanges": [ { "textDocument": { "uri": uri }, "edits": null } ] }),
            serde_json::json!({ "documentChanges": [ { "textDocument": { "uri": uri }, "edits": { "range": {} } } ] }),
            serde_json::json!({ "changes": { uri.clone(): "func F(y, x int) {}" } }),
            serde_json::json!({ "changes": { uri.clone(): null } }),
        ];
        for shape in shapes {
            let err = refused(shape.clone(), &mut originals);
            assert!(
                err.contains("list") && err.contains("nothing was written"),
                "{shape}: {err}"
            );
        }
        let not_a_list = refused(
            serde_json::json!({ "documentChanges": { "textDocument": { "uri": uri } } }),
            &mut originals,
        );
        assert!(
            not_a_list.contains("`documentChanges` is not a list"),
            "{not_a_list}"
        );
        // `u32::MAX + 1 + n` truncates to `n`: line 2, column 7 is `F`'s parameter list.
        let wrap = 1u64 << 32;
        let oversized = |line: u64, character: u64| {
            serde_json::json!({ "changes": { uri.clone(): [ {
                "range": { "start": { "line": line, "character": character },
                           "end": { "line": 2, "character": 11 } },
                "newText": "y, x"
            } ] } })
        };
        assert!(edits_by_file(&root, &oversized(2, 7), &mut originals).is_ok());
        for (line, character) in [(wrap + 2, 7), (2, wrap + 7), (u64::MAX, 7)] {
            let err = refused(oversized(line, character), &mut originals);
            assert!(
                err.contains("out of range or malformed"),
                "{line}:{character}: {err}"
            );
        }
        let negative = serde_json::json!({ "changes": { uri.clone(): [ {
            "range": { "start": { "line": -1, "character": 7 }, "end": { "line": 2, "character": 11 } },
            "newText": "y, x"
        } ] } });
        assert!(refused(negative, &mut originals).contains("out of range or malformed"));
        // An explicitly empty list is an answer, and stays one.
        let empty = edits_by_file(
            &root,
            &serde_json::json!({ "documentChanges": [ { "textDocument": { "uri": uri }, "edits": [] } ] }),
            &mut originals,
        )
        .unwrap();
        assert_eq!(empty[&file], Vec::<TextEdit>::new());
    }

    #[test]
    fn native_edits_inside_crlf_or_surrogates_are_refused_without_mutation() {
        let ws = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
        let root = std::fs::canonicalize(ws.path()).unwrap();
        let file = root.join("strict.go");
        std::fs::write(&file, "ab\r\n😀\r\n").unwrap();
        let uri = format!("file://{}", file.display());
        let before = std::fs::read(&file).unwrap();
        let edit = |line: u32, character: u32| {
            serde_json::json!({ "changes": { uri.clone(): [ {
                "range": {
                    "start": { "line": line, "character": character },
                    "end": { "line": 2, "character": 0 }
                },
                "newText": "x"
            } ] } })
        };
        for (line, character) in [(0, 3), (1, 1), (1, 3), (2, 1), (3, 0)] {
            let mut originals = BTreeMap::new();
            let err = edits_by_file(&root, &edit(line, character), &mut originals)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("out of range or malformed") && err.contains("nothing was written"),
                "{line}:{character}: {err}"
            );
            assert_eq!(
                std::fs::read(&file).unwrap(),
                before,
                "{line}:{character} wrote"
            );
        }
    }

    #[tokio::test]
    async fn public_positions_do_not_saturate_zero_to_one() {
        let ws = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
        let root = std::fs::canonicalize(ws.path()).unwrap();
        let file = root.join("a.go");
        std::fs::write(&file, "package a\nfunc F(a int) {}\n").unwrap();
        let remote = "127.0.0.1:1".parse().unwrap();
        for (line, col) in [(0, 1), (1, 0), (0, 0)] {
            let err = change_with(
                remote,
                &root,
                &file,
                line,
                col,
                &[],
                &Modifiers::default(),
                false,
                false,
            )
            .await
            .unwrap_err()
            .to_string();
            assert!(err.contains("not in the file"), "{line}:{col}: {err}");
        }
    }
}

#[cfg(test)]
mod removal_evidence_validation_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn malformed_parameter_locations_cannot_prove_a_removal_safe() {
        let file = Path::new("/tmp/parameter-proof.go");
        let text = "func F(discard int) {}";
        let at = text.find("discard").unwrap();
        let body = (text.find('{').unwrap(), text.find('}').unwrap());
        let good = json!({"uri":url::Url::from_file_path(file).unwrap().to_string(),
            "range":{"start":{"line":0,"character":at},"end":{"line":0,"character":at+7}}});
        assert!(
            parameter_evidence(&json!([good.clone()]), file, text, "discard", at, body)
                .unwrap()
                .is_empty()
        );
        let mut no_end = good.clone();
        no_end["range"].as_object_mut().unwrap().remove("end");
        let mut wrong_end = good.clone();
        wrong_end["range"]["end"]["character"] = json!(at);
        let mut raw_path = good.clone();
        raw_path["uri"] = json!(file.to_str().unwrap());
        let mut overflow = good;
        overflow["range"]["start"]["line"] = json!(u32::MAX);
        overflow["range"]["end"]["line"] = json!(u32::MAX);
        let mut failures = Vec::new();
        for entry in [no_end, wrong_end, raw_path, overflow] {
            let result = std::panic::catch_unwind(|| {
                parameter_evidence(&json!([entry.clone()]), file, text, "discard", at, body)
            });
            if !matches!(result, Ok(Err(_))) {
                failures.push(format!("accepted or panicked: {entry}: {result:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

#[cfg(test)]
mod literal_expression_regression {
    #[test]
    fn hexadecimal_digits_do_not_introduce_decimal_exponent_signs() {
        for literal in [
            "0x1e", "-0X1E", "0x1p+2", "0X1P-2", "1e+2", "-1E-2", "0x1p+2i",
        ] {
            assert!(super::is_literal(literal), "{literal}");
        }
        for expression in ["0x1e+2", "0x1E-2", "0x1e+counter", "0X1E-value"] {
            assert!(!super::is_literal(expression), "{expression}");
            assert!(!super::scalar_literal(expression), "{expression}");
        }
    }
}

#[cfg(test)]
mod interface_name_boundary_primary_probe {
    use super::*;
    #[test]
    fn an_unrelated_interface_member_does_not_block_receiver_addition() {
        assert!(
            !declares_interface_method("type Unrelated interface { NotAdd(x int) string }", "Add"),
            "NotAdd is not Add"
        );
        assert!(!declares_interface_method(
            "type Unrelated interface { Other(Add (int)) }",
            "Add"
        ));
        assert!(
            !declares_interface_method("const note = `interface { Add(int) string }`", "Add"),
            "source text in a string is not an interface obligation"
        );
        assert!(
            declares_interface_method(
                "type Related interface { Add /* comment */ (int) string }",
                "Add"
            ),
            "whitespace/comments are allowed before the parameter list"
        );
    }
}
