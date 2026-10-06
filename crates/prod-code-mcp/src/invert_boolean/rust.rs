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

use super::syntax::{call_start, is_ident, negated_body};
use super::types::{Inverted, display};

/// Inverts the predicate declared at `line`:`col` of `file` under `new_name`.
#[allow(clippy::too_many_arguments)]
pub async fn invert(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    new_name: &str,
    apply: bool,
    force: bool,
) -> Result<Inverted> {
    anyhow::ensure!(
        !new_name.is_empty() && new_name.chars().all(is_ident),
        "`{new_name}` is not an identifier"
    );
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
    if !text[..start].trim_end().ends_with("fn") {
        let name: String = text[start..].chars().take_while(|c| is_ident(*c)).collect();
        let kind = crate::invert_value::value_kind(&text, start, &name).context(
            "the position is not the name of a function returning `bool`, a `bool` field or a \
             `let` binding",
        )?;
        return crate::invert_value::invert_value(
            remote, root, file, &text, start, kind, new_name, apply, force,
        )
        .await;
    }
    let (name, _, close) =
        crate::signature::param_span(&text, start).context("the function has no parameter list")?;
    anyhow::ensure!(name != new_name, "the new name is the old one");
    let returns = crate::wrap_return::declared_return(&text, close)
        .map(|(s, e)| text[s..e].to_string())
        .unwrap_or_default();
    anyhow::ensure!(
        returns == "bool",
        "`{name}` returns `{}`, not `bool`",
        if returns.is_empty() { "()" } else { &returns }
    );
    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{name}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the body does not close")?;

    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let own = edits.entry(file.to_path_buf()).or_default();
    own.push((start, name.len(), new_name.to_string()));
    own.push((
        body_open + 1,
        body_close - body_open - 1,
        negated_body(&text[body_open + 1..body_close]),
    ));

    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let (mut negated, mut cancelled) = (0usize, 0usize);
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
        if !body[at..].starts_with(name.as_str()) || body[at + name.len()..].starts_with(is_ident) {
            unmatched.push(format!(
                "{site} (the analyzer places `{name}` here, but the file says otherwise)"
            ));
            continue;
        }
        let same_file = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()) == canonical;
        anyhow::ensure!(
            !(same_file && body_open < at && at < body_close),
            "`{name}` calls itself; invert a recursive predicate by hand"
        );
        let Some((_, args_end)) = crate::parameter_object::call_args_span(&body, at + name.len())
        else {
            unmatched.push(format!("{site} `{name}` used as a value"));
            continue;
        };
        let begin = call_start(&body, at);
        let after = body[args_end + 1..].trim_start();
        let continues = after.starts_with('.') || after.starts_with('?') || after.starts_with('[');
        let lead = body[..begin].trim_end();
        let spot = edits.entry(path.clone()).or_default();
        spot.push((at, name.len(), new_name.to_string()));
        if !continues && lead.ends_with('!') && !lead.ends_with("!=") {
            // `!is_valid(x)` becomes `is_invalid(x)`: the two negations cancel.
            spot.push((lead.len() - 1, 1, String::new()));
            cancelled += 1;
        } else if continues {
            spot.push((begin, 0, "(!".to_string()));
            spot.push((args_end + 1, 0, ")".to_string()));
            negated += 1;
        } else {
            spot.push((begin, 0, "!".to_string()));
            negated += 1;
        }
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        // Insertions at the same offset as a replacement go before it: sort by offset, then put
        // zero-length edits first.
        file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
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
        // A function used as a value keeps its old meaning under the new name, and still
        // compiles; `force` overrides the analyzer, not a use this did not negate (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{name}` were not negated and would mean the opposite; nothing \
             was written:\n  {}",
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

    Ok(Inverted {
        was: name,
        now: new_name.to_string(),
        root: root.to_path_buf(),
        file: display(root, file),
        kind: "function".to_string(),
        negated,
        cancelled,
        writes: 0,
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
