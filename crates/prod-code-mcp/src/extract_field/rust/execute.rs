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

use crate::extract_field::helpers::{display, is_ident, mentions, source_line};
use crate::extract_field::rust::insertion::{field_insertion, literal_insertion};
use crate::extract_field::rust::locate::{
    braces_kind, constructor_brace, impl_blocks, method_at, self_literals, struct_braces,
};
use crate::extract_field::types::{Braces, ExtractedField};

/// Promotes the expression selected in `file` into a field of the type its method belongs to.
#[allow(clippy::too_many_arguments)]
pub async fn extract(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    start: (u32, u32),
    end: (u32, u32),
    name: &str,
    ty: Option<&str>,
    init: Option<&str>,
    replace_all: bool,
    apply: bool,
    force: bool,
) -> Result<ExtractedField> {
    anyhow::ensure!(
        !name.is_empty() && name.chars().all(is_ident),
        "`{name}` is not an identifier"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let from = crate::signature::offset_of(&text, start.0, start.1)
        .context("the selection does not start inside the file")?;
    let to = crate::signature::offset_of(&text, end.0, end.1)
        .context("the selection does not end inside the file")?;
    anyhow::ensure!(to > from, "the selection is empty");
    let expression = text[from..to].trim().to_string();
    anyhow::ensure!(!expression.is_empty(), "the selection is only whitespace");

    // The method, the `impl` it is in, and the type that `impl` is for.
    let (owner, impl_at, impl_open, impl_close) = impl_blocks(&text)
        .into_iter()
        .filter(|(_, _, open, close)| *open < from && from < *close)
        .min_by_key(|(_, _, open, close)| close - open)
        .context("the selection is not inside an `impl` block")?;
    let (method, params, body_open, body_close) = method_at(&text, impl_open, impl_close, from)
        .context("the selection is not inside a method")?;
    anyhow::ensure!(
        mentions(&params, "self"),
        "`{method}` takes no `self`, so it has no field to read; extract a parameter instead"
    );
    anyhow::ensure!(
        to <= body_close,
        "the selection runs past the end of `{method}`"
    );
    let init = match init {
        Some(init) => init.trim().to_string(),
        None => {
            anyhow::ensure!(
                !mentions(&expression, "self"),
                "`{expression}` reads `self`, which does not exist yet where `{owner}` is built; \
                 pass `init` with what a new value should start as"
            );
            expression.clone()
        }
    };
    let ty = ty
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .context("pass the field's `type`: the analyzer gives no type for an arbitrary expression in a shape this can read")?;

    // Where the type is declared.
    let owner_in_header = text[impl_at..impl_open]
        .rfind(owner.as_str())
        .map(|i| impl_at + i)
        .context("the `impl` header does not name its type")?;
    let (hl, hc) = crate::signature::position_at(&text, owner_in_header)?;
    let definition = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/definition",
        serde_json::json!({
            "textDocument": { "uri": url::Url::from_file_path(file)
                .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?.to_string() },
            "position": { "line": hl - 1, "character": hc - 1 },
        }),
    )
    .await?;
    let location = match &definition {
        serde_json::Value::Array(items) => items.first().cloned(),
        other if other.is_object() => Some(other.clone()),
        _ => None,
    }
    .with_context(|| format!("the analyzer does not know where `{owner}` is declared"))?;
    let def_uri = location
        .get("uri")
        .or_else(|| location.get("targetUri"))
        .and_then(|u| u.as_str())
        .context("the definition has no file")?;
    let def_path = PathBuf::from(crate::remote_fs::uri_to_path(def_uri));
    let range = location
        .get("range")
        .or_else(|| location.get("targetSelectionRange"))
        .context("the definition has no position")?;
    let def_line = range
        .pointer("/start/line")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32
        + 1;
    let def_col = range
        .pointer("/start/character")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32
        + 1;

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let def_text = texts
        .entry(def_path.clone())
        .or_insert_with(|| std::fs::read_to_string(&def_path).unwrap_or_default())
        .clone();
    let def_at = crate::signature::offset_of(&def_text, def_line, def_col)
        .context("the declaration is not where the analyzer put it")?;
    anyhow::ensure!(
        def_text[def_at..].starts_with(owner.as_str()),
        "the analyzer places `{owner}` at {}:{def_line}:{def_col}, but the file says otherwise",
        display(root, &def_path)
    );
    let (struct_open, struct_close) = struct_braces(&def_text, def_at)
        .with_context(|| format!("`{owner}` is not a struct with named fields"))?;
    anyhow::ensure!(
        !def_text[struct_open + 1..struct_close].lines().any(|l| l
            .trim_start()
            .trim_start_matches("pub ")
            .starts_with(&format!("{name}:"))),
        "`{owner}` already has a field `{name}`"
    );

    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    // The method reads the field where it used to compute the value.
    let mut replaced = 0usize;
    if replace_all {
        let mut at = body_open;
        while let Some(i) = text[at..body_close].find(&expression) {
            let hit = at + i;
            edits.entry(file.to_path_buf()).or_default().push((
                hit,
                expression.len(),
                format!("self.{name}"),
            ));
            replaced += 1;
            at = hit + expression.len();
        }
    } else {
        let lead = text[from..to].len() - text[from..to].trim_start().len();
        edits.entry(file.to_path_buf()).or_default().push((
            from + lead,
            expression.len(),
            format!("self.{name}"),
        ));
        replaced = 1;
    }
    // The struct declares it.
    edits
        .entry(def_path.clone())
        .or_default()
        .push(field_insertion(
            &def_text,
            struct_open,
            struct_close,
            &format!("{name}: {ty}"),
        ));

    // Every construction site initialises it.
    let field_init = format!("{name}: {init}");
    let mut constructors = 0usize;
    let mut blocked = Vec::new();
    let mut unmatched = Vec::new();
    let mut impl_files: Vec<PathBuf> = vec![def_path.clone(), file.to_path_buf()];
    let mut braces: Vec<(PathBuf, usize)> = Vec::new();
    let refs = crate::signature::references(remote, root, &def_path, def_line, def_col)
        .await
        .with_context(|| {
            format!("cannot find the construction sites of `{owner}`; nothing was planned")
        })?;
    for (path, rl, rc) in refs {
        let body = crate::refactor::referenced_text(&mut texts, &path)?;
        let at_site = format!("{}:{rl}:{rc}", display(root, &path));
        let Some(at) = crate::signature::offset_of(body, rl, rc) else {
            unmatched.push(format!("{at_site} (the position is not in the file)"));
            continue;
        };
        // Inside its own `impl`, the analyzer reports `Self` as a reference to the type too.
        let spelled = [owner.as_str(), "Self"].into_iter().find(|spelling| {
            body[at..].starts_with(spelling) && !body[at + spelling.len()..].starts_with(is_ident)
        });
        let Some(spelled) = spelled else {
            unmatched.push(format!(
                "{at_site} (the analyzer places `{owner}` here, but the file says otherwise)"
            ));
            continue;
        };
        if !impl_files.contains(&path) {
            impl_files.push(path.clone());
        }
        if let Some(open) = constructor_brace(body, at, spelled) {
            braces.push((path.clone(), open));
        }
    }
    for path in &impl_files {
        let body = crate::refactor::referenced_text(&mut texts, path)?.clone();
        for (ty_name, _, open, close) in impl_blocks(&body) {
            if ty_name == owner {
                braces.extend(
                    self_literals(&body, open, close)
                        .into_iter()
                        .map(|b| (path.clone(), b)),
                );
            }
        }
    }
    braces.sort();
    braces.dedup();
    for (path, open) in braces {
        let body = &texts[&path];
        let (line, col) = crate::signature::position_at(body, open)?;
        let Some(close) = crate::parameter_object::matching_bracket(body, open) else {
            unmatched.push(format!(
                "{}:{line}:{col} (a construction whose braces do not close)",
                display(root, &path)
            ));
            continue;
        };
        match braces_kind(body, open, close) {
            Braces::Literal => {
                edits
                    .entry(path.clone())
                    .or_default()
                    .push(literal_insertion(body, open, &field_init));
                constructors += 1;
            }
            Braces::Pattern { rest: true } => {}
            Braces::Pattern { rest: false } => blocked.push(format!(
                "{}:{line}:{col} a pattern that lists every field no longer matches: `{}`",
                display(root, &path),
                source_line(body, open)
            )),
        }
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, _, _)| *at);
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
            "{} use(s) of `{owner}` stop compiling with one more field; nothing was written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
        );
        // A construction site left as it was is in a file nothing here checks; `force`
        // overrides the analyzer, not a site this did not read (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{owner}` were not read, so a construction there may lack the \
             field; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `init`, or \
             `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(ExtractedField {
        owner,
        method,
        root: root.to_path_buf(),
        file: display(root, file),
        name: name.to_string(),
        ty,
        init,
        replaced,
        constructors,
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
