/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::lsp::{display, local_type};
use super::syntax::{
    braces_are_pattern, derives_above, enclosing_open_brace, expression_end, inside_string,
    is_struct_brace, negation_of,
};
use super::types::{ValueKind, is_ident};
use crate::invert_boolean::Inverted;

/// Inverts the boolean field or local declared at `start` (its name) of `file`.
#[allow(clippy::too_many_arguments)]
pub async fn invert_value(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    start: usize,
    kind: ValueKind,
    new_name: &str,
    apply: bool,
    force: bool,
) -> Result<Inverted> {
    let name: String = text[start..].chars().take_while(|c| is_ident(*c)).collect();
    anyhow::ensure!(name != new_name, "the new name is the old one");
    let mut blocked = Vec::new();
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.to_string());
    let (l0, c0) = crate::signature::position_at(text, start)?;

    // The declaration.
    let own = edits.entry(file.to_path_buf()).or_default();
    own.push((start, name.len(), new_name.to_string()));
    let mut writes = 0usize;
    match &kind {
        ValueKind::Field { header } => {
            for derive in derives_above(text, *header) {
                match derive.as_str() {
                    "Default" => blocked.push(format!(
                        "{}:{l0}:{c0} the struct derives `Default`: a default `{name}` of `false` \
                         would be a default `{new_name}` of `false`, the opposite; write the \
                         `Default` impl by hand first",
                        display(root, file)
                    )),
                    "Serialize" | "Deserialize" => blocked.push(format!(
                        "{}:{l0}:{c0} the struct derives `{derive}`: the serialised field would \
                         change its name and its meaning; keep the name with `#[serde(rename)]` \
                         and invert the value in a custom (de)serializer, or do it by hand",
                        display(root, file)
                    )),
                    _ => {}
                }
            }
        }
        ValueKind::Local { annotated } => {
            if !annotated {
                let hover = local_type(remote, root, file, l0, c0).await;
                anyhow::ensure!(
                    hover.as_deref() == Some("bool"),
                    "`{name}` is {}, not `bool`: only a boolean can be inverted",
                    hover.map_or(
                        "of a type the analyzer does not say".to_string(),
                        |t| format!("`{t}`")
                    )
                );
            }
            // `let name = value;` stores the negation.
            let rest = &text[start + name.len()..];
            if let Some(eq) = rest
                .find('=')
                .filter(|&i| !rest[..i].contains(';') && !rest[i..].starts_with("=="))
            {
                let value_start = start + name.len() + eq + 1;
                let value_end = expression_end(text, value_start);
                own.push((
                    value_start,
                    value_end - value_start,
                    format!(" {}", negation_of(&text[value_start..value_end])),
                ));
                writes += 1;
            }
        }
    }

    let (mut negated, mut cancelled) = (0usize, 0usize);
    let mut unmatched = Vec::new();
    let refs = crate::signature::references(remote, root, file, l0, c0)
        .await
        .with_context(|| format!("cannot find the uses of `{name}`; nothing was planned"))?;
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
        if inside_string(&body, at) {
            blocked.push(format!("{site} `{name}` is used inside a format string"));
            continue;
        }
        let end = at + name.len();
        let after = body[end..].trim_start();
        let before = body[..at].trim_end();
        let spot = edits.entry(path.clone()).or_default();

        // In struct braces: `name: value`, or the shorthand `name`.
        let struct_open = enclosing_open_brace(&body, at).filter(|&o| is_struct_brace(&body, o));
        if let Some(open) = struct_open
            && (before.ends_with('{') || before.ends_with(','))
        {
            let is_init = after.starts_with(':') && !after.starts_with("::");
            let is_shorthand = after.starts_with(',') || after.starts_with('}');
            if is_init || is_shorthand {
                if braces_are_pattern(&body, open) {
                    blocked.push(format!(
                        "{site} a pattern binds `{name}`: the binding would hold the opposite"
                    ));
                    continue;
                }
                let field_side = matches!(kind, ValueKind::Field { .. });
                if is_init && field_side {
                    let colon = end + body[end..].find(':').unwrap_or(0);
                    let value_end = expression_end(&body, colon + 1);
                    spot.push((at, name.len(), new_name.to_string()));
                    spot.push((
                        colon + 1,
                        value_end - colon - 1,
                        format!(" {}", negation_of(&body[colon + 1..value_end])),
                    ));
                    writes += 1;
                } else if is_shorthand && field_side {
                    // `S { enabled }` with a local `enabled`: `S { disabled: !enabled }`.
                    spot.push((at, name.len(), format!("{new_name}: !{name}")));
                    writes += 1;
                } else if is_shorthand {
                    // A local used as a field's shorthand: `S { flag }` → `S { flag: !not_flag }`.
                    spot.push((at, name.len(), format!("{name}: !{new_name}")));
                    negated += 1;
                } else {
                    unmatched.push(format!("{site} (neither a read nor a write)"));
                }
                continue;
            }
        }

        // Where the use begins: the receiver chain of a field, or the name of a local.
        let (begin, is_field_access) = match before.strip_suffix('.') {
            Some(rest) => (
                crate::encapsulate_field::chain_start(&body, rest.len()),
                true,
            ),
            None => (at, false),
        };
        let lead = body[..begin].trim_end();
        if lead.ends_with('&') || lead.ends_with("&mut") {
            blocked.push(format!(
                "{site} `{name}` is borrowed: the reference would read the opposite"
            ));
            continue;
        }
        let compound = ["|=", "&=", "^="];
        if compound.iter().any(|op| after.starts_with(op)) {
            blocked.push(format!(
                "{site} a compound assignment to `{name}` is not the negation of the same one"
            ));
            continue;
        }
        if after.starts_with('=') && !after.starts_with("==") && !after.starts_with("=>") {
            let eq = end + body[end..].find('=').unwrap_or(0);
            let value_end = expression_end(&body, eq + 1);
            spot.push((at, name.len(), new_name.to_string()));
            spot.push((
                eq + 1,
                value_end - eq - 1,
                format!(" {}", negation_of(&body[eq + 1..value_end])),
            ));
            writes += 1;
            continue;
        }
        if !is_field_access && matches!(kind, ValueKind::Field { .. }) {
            unmatched.push(format!("{site} (a field named without a receiver)"));
            continue;
        }
        let continues = after.starts_with('.') || after.starts_with('?') || after.starts_with('[');
        spot.push((at, name.len(), new_name.to_string()));
        if !continues && lead.ends_with('!') && !lead.ends_with("!=") {
            spot.push((lead.len() - 1, 1, String::new()));
            cancelled += 1;
        } else if continues {
            spot.push((begin, 0, "(!".to_string()));
            spot.push((end, 0, ")".to_string()));
            negated += 1;
        } else {
            spot.push((begin, 0, "!".to_string()));
            negated += 1;
        }
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
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
        anyhow::ensure!(
            blocked.is_empty() || force,
            "{} use(s) of `{name}` cannot keep their meaning under the inversion; nothing was \
             written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
        );
        // A use left as it was reads the opposite of what it did, and still compiles; `force`
        // overrides the analyzer, not a use this did not negate (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} use(s) of `{name}` were not rewritten and would read the opposite; nothing was \
             written:\n  {}",
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
        kind: match kind {
            ValueKind::Field { .. } => "field",
            ValueKind::Local { .. } => "variable",
        }
        .to_string(),
        negated,
        cancelled,
        writes,
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
