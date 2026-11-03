/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Borrow an existing Rust parameter in place (#1810): `request: Request` becomes
//! `request: &Request` and every call passes `&argument`.
//!
//! The request lists every declared parameter in its declared order, with `&name` for those to
//! borrow; the arity, the other parameters and the order of the arguments stay as they were, and
//! each borrowed argument is written once, where it was, with one `&` in front (or `&( … )`
//! around an expression a prefix `&` would split). The change is the ownership contract the
//! request asks for, not an equivalence: the caller now keeps and drops the value. Anything this
//! cannot rewrite exactly — a function value, an aliased import, a trait's method, a mutable or
//! patterned binding, an already-borrowed type, a result that borrows through lifetime elision,
//! another language — is refused before anything is written, and so is a proposal the analyzer
//! does not type-check. `force` overrides none of it.
//!
//! This module only previews. The analyzer's overlay check has no borrow checker: once `T` is
//! `&T`, a body that moves out of the parameter (`fn take(request: Request) -> String {
//! request.0 }`) is E0507 to the compiler and clean to the analyzer. So `apply` is refused here,
//! before the analyzer is asked anything; the write goes through `code_change_signature` with
//! `verify: "compile"`, which plans this preview and writes it only when `cargo check` of the
//! affected packages accepts it.

mod calls;
mod declaration;

pub use calls::{apply_insertions, borrow_argument, borrow_insertions};
pub use declaration::{Insertion, borrowed_indices, declaration_edits};

/// Why an unverified borrow conversion is not written, and how to write it.
pub const NEEDS_COMPILER: &str = "a borrow conversion is written only after the compiler accepts \
     it: the analyzer's check that the preview reports has no borrow checker, so a body that moves \
     out of the borrowed parameter (E0507) looks clean to it. Nothing was written. Preview with \
     `apply: false`, then write with `code_change_signature` and `apply: true, verify: \
     \"compile\"`, which runs `cargo check` on the affected packages in a shadow of the workspace \
     and writes only if it passes; `force` does not skip that check";

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::signature::parse::{offset_of, param_span, parse_declared};
use crate::signature::references::references;
use crate::signature::types::{Modifiers, Param, SignatureChange};
use crate::signature::util::{display, error_lines, normalize, read_caller};
use calls::{Callee, call_edits};

/// Whether the request borrows a parameter (`&name`).
pub fn requested(request: &[Param]) -> bool {
    request.iter().any(|p| matches!(p, Param::Borrow(_)))
}

/// Refuses a borrow request for a language other than Rust, before anything is read or written.
pub fn refuse_outside_rust(request: &[Param], language: &str) -> Result<()> {
    if let Some(Param::Borrow(name)) = request.iter().find(|p| matches!(p, Param::Borrow(_))) {
        anyhow::bail!(
            "`&{name}` borrows an existing parameter, which is supported for Rust only, not \
             {language}; nothing was written"
        );
    }
    Ok(())
}

/// Previews borrowing the parameters `request` marks with `&` in the Rust function at
/// `file:line:col`, with every call the analyzer reports; refuses a change that is not complete.
/// `apply` is refused before any analyzer request or write ([`NEEDS_COMPILER`]): only the
/// compile gate of `code_change_signature` writes this preview.
pub async fn change(
    remote: SocketAddr,
    root: &Path,
    (file, line, col): (&Path, u32, u32),
    request: &[Param],
    modifiers: &Modifiers,
    apply: bool,
) -> Result<SignatureChange> {
    if file.extension().is_none_or(|e| e != "rs") {
        let language = file.extension().map_or("this file".into(), |e| {
            format!("`.{}` files", e.to_string_lossy())
        });
        refuse_outside_rust(request, &language)?;
    }
    anyhow::ensure!(
        modifiers.returns.is_none()
            && modifiers.visibility.is_none()
            && modifiers.asyncness.is_none(),
        "borrow a parameter and change the return type, the visibility or `async` in separate \
         requests; nothing was written"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset =
        offset_of(&text, line, col).context("the declaration is not at the resolved position")?;
    let (name, open, close) = param_span(&text, offset)
        .with_context(|| format!("no function declaration at {}:{line}:{col}", file.display()))?;
    let old_inner = text[open..close].to_string();
    let (receiver, declared) = parse_declared(&old_inner);
    let borrowed = borrowed_indices(&declared, request)?;
    let declaration = declaration_edits(&text, open, &name, &borrowed)?;
    let inside: Vec<Insertion> = declaration
        .iter()
        .map(|(at, piece)| (at - open, piece.clone()))
        .collect();
    let new_inner = apply_insertions(&old_inner, &inside);
    // No write without the compiler's word, whatever `force` says; this path has none (#1810).
    anyhow::ensure!(!apply, "{NEEDS_COMPILER}");

    // Every reference, asked once before anything is rewritten; a failed question stops it.
    let refs = references(remote, root, file, line, col)
        .await
        .with_context(|| format!("cannot list the references to `{name}`; nothing was written"))?;
    let mut edits: BTreeMap<PathBuf, Vec<Insertion>> = BTreeMap::new();
    edits.insert(file.to_path_buf(), declaration);
    let callee = Callee {
        file,
        open,
        name: &name,
        receiver: receiver.is_some(),
        arity: declared.len(),
        borrowed: &borrowed,
    };
    call_edits(root, &callee, &refs, &mut edits)?;
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, insertions) in &edits {
        let base = if path == file {
            text.clone()
        } else {
            read_caller(path)?
        };
        rewritten.insert(path.clone(), apply_insertions(&base, insertions));
    }

    // The declaration and every rewritten call judged together, with the callers left as they
    // were (an import) checked against the new declaration.
    let overlay: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let mut callers: Vec<PathBuf> = refs
        .iter()
        .map(|(p, _, _)| p.clone())
        .filter(|p| !rewritten.contains_key(p))
        .collect();
    callers.sort();
    callers.dedup();
    let reports = crate::diagnostics::validate_texts(remote, root, &overlay, &callers).await?;
    let names: Vec<String> = borrowed
        .iter()
        .map(|&i| format!("`{}`", declared[i].name))
        .collect();
    Ok(SignatureChange {
        symbol: name,
        root: root.to_path_buf(),
        file: display(root, file),
        old_signature: normalize(&old_inner),
        new_signature: normalize(&new_inner),
        rule: format!(
            "borrow {}: every call passes `&argument`, evaluated once where it was",
            names.join(", ")
        ),
        rewritten: rewritten
            .iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t.clone()))
            .collect(),
        unmatched: Vec::new(),
        unexpected: Vec::new(),
        diagnostics: error_lines(&reports),
        applied: false,
        returns: None,
        visibility: None,
        asyncness: None,
        not_async: Vec::new(),
    })
}
