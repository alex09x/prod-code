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
        // gopls prints the new lists afresh, and a comment inside one can be lost or left
        // beside another argument.
        let mut spans: Vec<(usize, usize)> = calls
            .iter()
            .filter(|c| &c.path == path)
            .map(|c| (c.open, c.close))
            .collect();
        if path == file {
            spans.push((decl.open, decl.close));
        }
        if !comments_follow(old, &new, list, &spans, order, arity, variadic) {
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

/// Whether every comment of `old` is where `new` should have it (#1410). Outside the argument
/// and parameter lists at `spans` (each `(` with its `)`) comments stay in order. Inside one, a
/// comment belongs to the argument or parameter it is written beside: it moves to that one's
/// new place, and goes when that one is removed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn comments_follow(
    old: &str,
    new: &str,
    edits: &[TextEdit],
    spans: &[(usize, usize)],
    order: &[usize],
    arity: usize,
    variadic: bool,
) -> bool {
    // Each element's comments, sorted: a nested call's own arguments are checked as its list.
    let by_element = |list: &str| -> Vec<String> {
        split_list(list)
            .iter()
            .map(|element| {
                let mut found = comments(element);
                found.sort_unstable();
                found.join("\n")
            })
            .collect()
    };
    let mut spans = spans.to_vec();
    spans.sort_unstable();
    let mut mapped = Vec::new();
    for (open, close) in spans {
        let Some(new_open) =
            map_offset(edits, open).filter(|&o| new.as_bytes().get(o) == Some(&b'('))
        else {
            return false;
        };
        let Some(new_close) = closing(new, new_open) else {
            return false;
        };
        let before = by_element(&old[open + 1..close]);
        let after = by_element(&new[new_open + 1..new_close]);
        // A list with no comment either side has none to lose; a wrong shape is reported apart.
        let uncommented = |list: &[String]| list.iter().all(String::is_empty);
        if !(uncommented(&before) && uncommented(&after))
            && permuted(&before, order, arity, variadic) != after
        {
            return false;
        }
        mapped.push(((open, close), (new_open, new_close)));
    }
    let outside = |text: &str, lists: Vec<(usize, usize)>| -> Vec<String> {
        let (mut kept, mut at) = (String::new(), 0);
        for (open, close) in lists {
            if open >= at {
                kept.push_str(&text[at..open]);
                kept.push(' ');
                at = close + 1;
            }
        }
        kept.push_str(&text[at..]);
        comments(&kept).into_iter().map(str::to_string).collect()
    };
    outside(old, mapped.iter().map(|m| m.0).collect())
        == outside(new, mapped.iter().map(|m| m.1).collect())
}

/// Callers that gopls did not report but that call `decl` all the same, one refusal each
/// (#1410). Such a file is hidden from gopls, by a build constraint for one, so its calls are
/// neither rewritten nor checked, and compiling it with the change proves nothing about what
/// its arguments now mean. A file of the declaring package counts when it writes the bare name,
/// another package's when it writes `pkg.Name(`, and for a method any file that writes `.name(`.
pub(crate) fn unreported_calls(
    root: &Path,
    file: &Path,
    text: &str,
    decl: &Decl,
    unreported: &[PathBuf],
) -> Vec<String> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let package_dir = file.parent().map(canonical);
    let package = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("package "))
        .map(str::trim);
    let name = decl.name.as_str();
    unreported
        .iter()
        .filter(|path| {
            let Ok(source) = std::fs::read_to_string(path) else {
                return false;
            };
            let code = strip_comments(&source);
            if decl.receiver.is_some() {
                return code.contains(&format!(".{name}("));
            }
            if path.parent().map(canonical) != package_dir {
                return package.is_some_and(|p| code.contains(&format!("{p}.{name}(")));
            }
            code.match_indices(&format!("{name}(")).any(|(at, _)| {
                !code[..at]
                    .bytes()
                    .next_back()
                    .is_some_and(|b| b == b'.' || b == b'_' || b.is_ascii_alphanumeric())
            })
        })
        .map(|path| {
            format!(
                "{}: calls `{name}`, but gopls did not report it (a build constraint or an \
                 unindexed file?), so its arguments are not rewritten",
                display(root, path)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::comments_follow;

    /// `old` with its one list `(…)` replaced by `inner`, as gopls would print it: whether the
    /// comments followed, for `order` of `arity` parameters.
    fn follows(old: &str, inner: &str, order: &[usize]) -> bool {
        let (open, close) = (old.find('(').unwrap(), old.rfind(')').unwrap());
        let edits = vec![(open + 1, close, inner.to_string())];
        let new = super::splice(old, &edits);
        comments_follow(old, &new, &edits, &[(open, close)], order, 2, false)
    }

    /// Controlled edits, not gopls's output: a real gopls drops these comments, and the
    /// public-operation test covers that.
    #[test]
    fn a_comment_belongs_to_the_argument_beside_it() {
        let removal = "F(a, b /* documents b */)\n";
        assert!(!follows(removal, "a /* documents b */", &[0]));
        assert!(follows(removal, "a", &[0]));
        let reorder = "F(a /* A */, b /* B */)\n";
        assert!(follows(reorder, "b /* B */, a /* A */", &[1, 0]));
        assert!(!follows(reorder, "b /* A */, a /* B */", &[1, 0]));
        assert!(!follows(reorder, "b, a", &[1, 0]));
        let outside = "x /* k */ + F(a, b) // end\n";
        assert!(follows(outside, "b, a", &[1, 0]));
        let new = "x + F(b, a) // end\n";
        let edits = vec![(
            0,
            outside.find(')').unwrap(),
            new[..new.find(')').unwrap()].to_string(),
        )];
        assert!(!comments_follow(
            outside,
            new,
            &edits,
            &[(13, 18)],
            &[1, 0],
            2,
            false
        ));
    }
}
