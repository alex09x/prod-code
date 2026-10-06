/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature_go::hazards::permuted;
use crate::signature_go::parse::{header, parameters};
use crate::signature_go::syntax::{canonical, list_text, normalize, suffix};
use crate::signature_go::text::{
    closing, comments, display, line_col_utf16, map_offset, splice, split_list, strip_comments,
};
use crate::signature_go::types::{Call, Decl, GoParam, TextEdit};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) struct GoplsEditsVerification {
    pub(crate) unexpected: Vec<String>,
    pub(crate) unmatched: Vec<String>,
    pub(crate) new_signature: String,
}

pub(crate) fn verify_gopls_edits(
    root: &Path,
    file: &Path,
    text: &str,
    decl: &Decl,
    new_params: &[GoParam],
    order: &[usize],
    arity: usize,
    variadic: bool,
    calls: &[Call],
    edits: &BTreeMap<PathBuf, Vec<TextEdit>>,
    originals: &BTreeMap<PathBuf, String>,
    rewritten: &mut BTreeMap<PathBuf, String>,
) -> GoplsEditsVerification {
    let mut unexpected = Vec::new();
    for (path, list) in edits {
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
    for call in calls {
        let expected = permuted(&call.args, order, arity, variadic);
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
                    list_text(new_params),
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
    GoplsEditsVerification {
        unexpected,
        unmatched,
        new_signature,
    }
}
