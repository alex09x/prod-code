/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The call side of a borrow conversion: every reference the analyzer reports is a call whose
//! argument for each borrowed parameter gains one `&`, or the change is refused.

use anyhow::{Context, Result};
use ra_ap_syntax::{AstNode, Edition, SourceFile, ast};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::declaration::Insertion;
use crate::signature::call_sites::{blank_comments, call_open, split_argument_ranges};
use crate::signature::parse::offset_of;
use crate::signature::plan::in_use_or_comment;
use crate::signature::types::Reference;
use crate::signature::util::{display, read_caller};

/// What the calls look like to the planner: the declaring file and where its parameter list's
/// inside starts, the function's name, whether it has a receiver, how many parameters it
/// declares besides it, and which of them are borrowed.
pub struct Callee<'a> {
    pub file: &'a Path,
    pub open: usize,
    pub name: &'a str,
    pub receiver: bool,
    pub arity: usize,
    pub borrowed: &'a [usize],
}

/// Adds to `edits` the insertions that borrow each call's arguments, and refuses, before anything
/// is written, a reference that is neither such a call nor an import or a comment that names the
/// function as it did: a function value, a callback, an aliased import, an unreadable call.
pub fn call_edits(
    root: &Path,
    callee: &Callee<'_>,
    refs: &[Reference],
    edits: &mut BTreeMap<PathBuf, Vec<Insertion>>,
) -> Result<()> {
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let name = callee.name;
    for (path, l, c) in refs {
        if !texts.contains_key(path) {
            texts.insert(path.clone(), read_caller(path)?);
        }
        let text = &texts[path];
        let at = format!("{}:{l}:{c}", display(root, path));
        let offset = offset_of(text, *l, *c)
            .filter(|o| text[*o..].starts_with(|ch: char| ch.is_alphanumeric() || ch == '_'))
            .with_context(|| {
                format!(
                    "the analyzer's reference {at} does not point at a name; the file may have \
                     changed since it was read. Nothing was written"
                )
            })?;
        let code = blank_comments(text);
        if in_use_or_comment(text, code.as_deref(), offset) {
            let rest = text[offset + name.len()..].trim_start();
            anyhow::ensure!(
                !(rest.starts_with("as") && rest[2..].starts_with(char::is_whitespace)),
                "`{name}` is imported under another name at {at}; calls through an alias are \
                 not rewritten to borrow. Nothing was written"
            );
            continue;
        }
        let open = call_open(text, offset)
            .with_context(|| format!("the call at {at} could not be read; nothing was written"))?
            .with_context(|| {
                format!(
                    "`{name}` is used as a value at {at}, not called: a function pointer, a \
                     callback or a stored path changes type with the declaration and cannot be \
                     rewritten to borrow. Nothing was written"
                )
            })?;
        if path.as_path() == callee.file && open + 1 == callee.open {
            continue; // the declaration itself
        }
        let ranges = split_argument_ranges(text, open).with_context(|| {
            format!("the arguments of the call at {at} could not be read; nothing was written")
        })?;
        let before = text[..offset].trim_end();
        let method = before.ends_with('.') && !before.ends_with("..");
        anyhow::ensure!(
            callee.receiver || !method,
            "the call at {at} is a method call of a function without a receiver; nothing was \
             written"
        );
        // `Type::method(receiver, …)` passes the receiver as its first argument.
        let shift = usize::from(callee.receiver && !method);
        anyhow::ensure!(
            ranges.len() == callee.arity + shift,
            "the call at {at} passes {} argument(s) where `{name}` takes {}; nothing was written",
            ranges.len(),
            callee.arity + shift
        );
        let file_edits = edits.entry(path.clone()).or_default();
        for &i in callee.borrowed {
            let range = ranges[i + shift].clone();
            anyhow::ensure!(
                !range.is_empty(),
                "the call at {at} has an empty argument; nothing was written"
            );
            file_edits.extend(borrow_insertions(text, range.start, range.end));
        }
    }
    Ok(())
}

/// The insertions that borrow the argument at `start..end` of `text` exactly once: `&` in front
/// of an expression that binds tighter than a borrow (a path, a call, a field, a literal), and
/// `&(` … `)` around anything else (`a + b`, `x as T`, a closure), with a trailing comment left
/// outside. The argument's own text is not moved or repeated, so it is evaluated once, where it
/// was, in the same order as the other arguments.
pub fn borrow_insertions(text: &str, start: usize, end: usize) -> Vec<Insertion> {
    let arg = &text[start..end];
    let code_len = blank_comments(arg).map_or(arg.len(), |c| c.trim_end().len());
    let code = &arg[..code_len];
    if binds_tighter_than_borrow(code) {
        vec![(start, "&".to_string())]
    } else {
        vec![
            (start, "&(".to_string()),
            (start + code_len, ")".to_string()),
        ]
    }
}

/// `arg` with one shared borrow in front, as [`borrow_insertions`] writes it.
pub fn borrow_argument(arg: &str) -> String {
    apply_insertions(arg, &borrow_insertions(arg, 0, arg.len()))
}

/// Whether `&` in front of `code` borrows all of it: it parses as one expression of a kind no
/// prefix operator splits.
fn binds_tighter_than_borrow(code: &str) -> bool {
    let prefix = "fn __prod_code_borrow_probe() { let _ = ";
    let wrapped = format!("{prefix}{code}; }}");
    let parsed = SourceFile::parse(&wrapped, Edition::Edition2024);
    if !parsed.errors().is_empty() {
        return false;
    }
    let Some(expr) = parsed
        .syntax_node()
        .descendants()
        .find_map(ast::LetStmt::cast)
        .and_then(|stmt| stmt.initializer())
    else {
        return false;
    };
    let range = expr.syntax().text_range();
    if usize::from(range.start()) != prefix.len()
        || usize::from(range.end()) != prefix.len() + code.len()
    {
        return false;
    }
    matches!(
        expr,
        ast::Expr::PathExpr(_)
            | ast::Expr::CallExpr(_)
            | ast::Expr::MethodCallExpr(_)
            | ast::Expr::FieldExpr(_)
            | ast::Expr::IndexExpr(_)
            | ast::Expr::ParenExpr(_)
            | ast::Expr::Literal(_)
            | ast::Expr::MacroExpr(_)
            | ast::Expr::TryExpr(_)
            | ast::Expr::AwaitExpr(_)
            | ast::Expr::TupleExpr(_)
            | ast::Expr::ArrayExpr(_)
            | ast::Expr::RecordExpr(_)
            | ast::Expr::RefExpr(_)
            | ast::Expr::PrefixExpr(_)
    )
}

/// `text` with every insertion made, each at its offset in the original text. Insertions at the
/// same offset keep the order they were listed in.
pub fn apply_insertions(text: &str, insertions: &[Insertion]) -> String {
    let mut sorted: Vec<&Insertion> = insertions.iter().collect();
    sorted.sort_by_key(|(at, _)| *at);
    let mut out = String::with_capacity(text.len() + insertions.len() * 2);
    let mut from = 0;
    for (at, piece) in sorted {
        out.push_str(&text[from..*at]);
        out.push_str(piece);
        from = *at;
    }
    out.push_str(&text[from..]);
    out
}
