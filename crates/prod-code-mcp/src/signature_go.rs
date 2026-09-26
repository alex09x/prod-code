//! Go parameter reordering through gopls (#448), with the same arguments and report as
//! [`crate::signature::change_with`], which hands every `.go` file here; so do the MCP tool
//! and the CLI through it. Nothing here calls back into `change_with`: only its helpers
//! (references, unreported callers, the whole-file edit), so the dispatch cannot recurse.
//!
//! gopls changes a signature when a rename is asked at the `func` keyword of a declaration and
//! the new name is the new signature: `func(b, a int) error`. Its v0.23.0 implementation accepts
//! only a permutation of the declared parameters (adding one, changing a type or the results is
//! refused), and rewrites every call by inlining a wrapper. That inliner runs with effect
//! analysis switched off, so `f(mark(a), mark(b))` comes back as `f(mark(b), mark(a))`: the
//! program runs its calls in another order. gopls's own edits are therefore used as they are,
//! never re-spelled here, but they are not trusted blindly:
//!
//! - every reference is asked for first; a use that is not a call (a function value, a method
//!   value) and a reorder of two arguments where either can have an effect the other sees are
//!   refused before gopls is asked;
//! - gopls's edit is checked against the permutation that was asked for: every call's arguments
//!   must come back as exactly the old arguments in the new order, the declaration's parameters
//!   likewise, and nothing outside those lists may change, in no file outside the checkout;
//! - the whole proposal is type-checked in an overlay, and written in one transaction or not at
//!   all. `force` overrides none of this.
//!
//! A reference list, a rename or a validation that cannot be had stops the change.

use crate::signature::{Modifiers, Param, SignatureChange};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// The gopls release whose behaviour this adapter was written and tested against.
pub const GOPLS_VERSION: &str = "v0.23.0";

/// The part of the requirement that a refusal leaves open, so that it stays visible.
const STILL_OPEN: &str = "Go signature changes here are limited to reordering the named \
     parameters of a declared function or method through gopls; adding or removing parameters, \
     changing parameter or result types, changing results, and rewriting function values remain \
     open requirements";

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

/// Changes the parameter order of the Go function or method at `file:line:col` (1-based; the
/// position may be anywhere from its `func` keyword to the `)` closing its parameters).
///
/// `request` must name every declared parameter exactly once; `modifiers` must be empty.
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
    refuse_modifiers(modifiers)?;
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = offset_at(&text, line.saturating_sub(1), col.saturating_sub(1))
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
    let order = permutation(&declared, request)?;
    let variadic = declared.last().is_some_and(|p| p.ty.starts_with("..."));
    if variadic && order.last() != Some(&(declared.len() - 1)) {
        return Err(refusal(format!(
            "`{}` is variadic, and Go allows `...` only on the last parameter",
            declared[declared.len() - 1].name
        )));
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
             parameter order and cannot be rewritten",
            decl.name,
            values.join(", ")
        )));
    }
    let hazards = effect_hazards(&decl.name, &declared, &order, variadic, &calls);
    anyhow::ensure!(
        hazards.is_empty(),
        "the new parameter order would change what the program does, not only how the calls \
         are written; nothing was written, and `force` does not override this:\n  {}\nbind such \
         an argument to a local before the call and pass the local",
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
            "gopls refused to reorder `{}` as `{signature}`{generic}: {e:#}; nothing was \
             written. {STILL_OPEN}",
            decl.name
        )
    })?;
    anyhow::ensure!(
        !edit.is_null(),
        "gopls answered the reorder of `{}` with no edit; nothing was written. {STILL_OPEN}",
        decl.name
    );
    let edits = edits_by_file(&canonical_root, &edit, &mut originals)?;

    // What gopls wrote, against what was asked.
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut unexpected = Vec::new();
    for (path, list) in &edits {
        let old = &originals[path];
        rewritten.insert(path.clone(), splice(old, list));
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
        let expected = permuted(&call.args, &order, variadic);
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
                "{}: gopls wrote ({}) where the new order is ({})",
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
            "gopls's edit is not the reorder that was asked for; nothing was written:\n  {}",
            unmatched
                .iter()
                .chain(&unexpected)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty(),
            "the reordered program does not type-check ({} error(s)); nothing was written, and \
             `force` does not override this:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        for (path, old) in &originals {
            let now = std::fs::read_to_string(path).unwrap_or_default();
            anyhow::ensure!(
                now == *old,
                "{} changed while the reorder was planned; nothing was written",
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

/// An error that says why, that nothing was written, and what stays open.
fn refusal(why: String) -> anyhow::Error {
    anyhow::anyhow!("{why}; nothing was written. {STILL_OPEN}")
}

/// Results, visibility and `async` are not parameters, and gopls changes none of them.
fn refuse_modifiers(modifiers: &Modifiers) -> Result<()> {
    if modifiers.returns.is_some() {
        return Err(refusal(format!(
            "changing a Go function's results is not supported: gopls {GOPLS_VERSION} refuses \
             result changes in a signature rename"
        )));
    }
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

/// The new order as indices into `declared`: every declared parameter exactly once, reordered.
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
    let dropped: Vec<&str> = declared
        .iter()
        .enumerate()
        .filter(|(i, _)| !order.contains(i))
        .map(|(_, d)| d.name.as_str())
        .collect();
    if !dropped.is_empty() {
        return Err(refusal(format!(
            "removing {} is not supported for Go: gopls's rename drops a parameter without \
             checking that the body no longer uses it or that its argument has no effect, and \
             its unused-parameter fix is not wired here",
            dropped
                .iter()
                .map(|d| format!("`{d}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    anyhow::ensure!(
        order.iter().enumerate().any(|(i, o)| i != *o),
        "the requested order is the declared one; there is nothing to change"
    );
    Ok(order)
}

/// A call's arguments in the new order; for a variadic function, whose last parameter stays
/// last, the arguments past the fixed parameters stay where they are, at the end.
fn permuted(args: &[String], order: &[usize], variadic: bool) -> Vec<String> {
    let fixed = if variadic {
        order.len().saturating_sub(1)
    } else {
        order.len()
    };
    let mut out: Vec<String> = order[..fixed]
        .iter()
        .filter_map(|&i| args.get(i).cloned())
        .collect();
    out.extend(args.iter().skip(fixed).cloned());
    out
}

/// What the new order would change at run time, one line per place: two arguments evaluated
/// the other way round when either can have an effect the other sees.
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
    }
    out
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
                    && matches!(nb[i - 1], b'e' | b'E' | b'p' | b'P'))
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
    let end = rest.find('\n').unwrap_or(rest.len());
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
        assert_eq!(permuted(&args[..3], &[2, 0, 1], false), vec!["c", "a", "b"]);
        assert_eq!(permuted(&args, &[1, 0, 2], true), vec!["b", "a", "c", "d"]);
        assert_eq!(permuted(&args[..2], &[1, 0, 2], true), vec!["b", "a"]);
        assert_eq!(permuted(&args[..3], &[1, 0, 2], true), vec!["b", "a", "c"]);
    }

    #[test]
    fn requests_that_are_not_permutations_are_refused_with_the_open_requirement() {
        let d = declared(&["a", "b"]);
        let keep = |n: &str| Param::Keep(n.to_string());
        assert_eq!(
            permutation(&d, &[keep("b"), keep("a")]).unwrap(),
            vec![1, 0]
        );
        let dropped = permutation(&d, &[keep("b")]).unwrap_err().to_string();
        assert!(
            dropped.contains("removing `a`") && dropped.contains(STILL_OPEN),
            "{dropped}"
        );
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
                returns: Some("error".into()),
                ..Default::default()
            },
            Modifiers {
                visibility: Some("pub".into()),
                ..Default::default()
            },
            Modifiers {
                asyncness: Some(true),
                ..Default::default()
            },
        ] {
            let err = refuse_modifiers(&m).unwrap_err().to_string();
            assert!(err.contains(STILL_OPEN), "{err}");
        }
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
}
