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

use super::enclosing::{
    declarations, enclosing_declaration, name_offset, parameter_list, with_argument,
};
use super::hover::{hover_type, type_from_hover};
use super::syntax::{Syntax, swift_labeled};
use super::types::{ExtractedParameter, display};

/// Promotes the expression selected in `file` into a parameter of the function that contains it.
#[allow(clippy::too_many_arguments)]
pub async fn extract(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    start: (u32, u32),
    end: (u32, u32),
    name: &str,
    ty: Option<&str>,
    replace_all: bool,
    apply: bool,
    force: bool,
) -> Result<ExtractedParameter> {
    anyhow::ensure!(
        !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'),
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
    let syntax = Syntax::of(file).with_context(|| {
        format!(
            "{} is not in a language this can extract a parameter in (Rust, TypeScript, \
             JavaScript, Python, Go, C, C++, Swift)",
            display(root, file)
        )
    })?;

    // The function the selection is inside, and its parameter list.
    let symbols = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": url::Url::from_file_path(file)
            .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?.to_string() } }),
    )
    .await?;
    let declaration = enclosing_declaration(&symbols, start.0)
        .context("the selection is not inside a function")?;
    let (callee, fn_start, fn_end) = (declaration.name, declaration.start, declaration.end);
    // What a call site spells: gopls names a method `(*Store).Limit`, its callers write `Limit`.
    let bare = syntax.bare_name(&callee).to_string();
    let (fn_offset, open, close) = if syntax == Syntax::Rust {
        // A byte offset in the line, not a column: text before the name can be wider in bytes
        // than in UTF-16 units (#456).
        let fn_offset = {
            let line_start = crate::signature::offset_of(&text, fn_start, 1)
                .context("the declaration's first line is not in the file")?;
            let head = text[line_start..].lines().next().unwrap_or_default();
            head.find(&format!("fn {callee}"))
                .map(|i| line_start + i + 3)
                .with_context(|| format!("`{callee}` is not a function"))?
        };
        let (_, open, close) = crate::signature::param_span(&text, fn_offset)
            .with_context(|| format!("`{callee}` has no parameter list"))?;
        (fn_offset, open, close)
    } else {
        let fn_offset = name_offset(&text, &bare, fn_start, declaration.name_at)
            .with_context(|| format!("`{callee}` is not declared where the analyzer put it"))?;
        let (open, close) = parameter_list(&text, fn_offset + bare.len())
            .with_context(|| format!("`{callee}` has no parameter list"))?;
        (fn_offset, open, close)
    };
    anyhow::ensure!(
        from > close,
        "the selection is in the signature, not in the body"
    );
    if let Some(rest) = syntax.catch_all(&text[open..close]) {
        anyhow::bail!(
            "`{callee}` takes `{rest}`, which collects whatever arguments are left: a parameter \
             after it would not receive the one every call site passes, so the callers would \
             change behaviour"
        );
    }

    // The type: the caller's, the literal's, or the one hover gives when it gives a shape this
    // can read. JavaScript has no annotations, so it needs none of them.
    let ty = match ty {
        _ if syntax == Syntax::JavaScript => String::new(),
        Some(ty) => ty.to_string(),
        None if syntax != Syntax::Rust => match syntax.literal_type(&expression) {
            Some(ty) => ty.to_string(),
            None => hover_type(remote, root, file, &text, from, to, syntax)
                .await
                .unwrap_or_default(),
        },
        None => {
            let hover = crate::tools::execute_lsp_query(
                remote,
                root,
                file,
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": url::Url::from_file_path(file)
                        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?.to_string() },
                    "position": { "line": start.0.saturating_sub(1), "character": start.1.saturating_sub(1) },
                }),
            )
            .await
            .ok()
            .and_then(|h| {
                h.pointer("/contents/value")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
            type_from_hover(&hover).context(
                "the analyzer does not give a type for this selection in a shape this can read; \
                 pass the type explicitly",
            )?
        }
    };
    let parameter = syntax
        .parameter(name, Some(ty.as_str()).filter(|t| !t.is_empty()))
        .context(
            "the analyzer does not give a type for this selection in a shape this can read; \
             pass the type explicitly",
        )?;
    // A Swift caller writes each argument's label. When every parameter the function already
    // has is labeled (or it has none), the new one is too, as the API guidelines would spell
    // it, and callers append `name: expression`. Once one of them is positional (`_ text:`),
    // the function has chosen positional arguments, and the new parameter is `_ name: T` with
    // the expression appended bare, which is also the only spelling that cannot collide with a
    // label the callers already write.
    let labeled = syntax == Syntax::Swift
        && crate::signature::split_params(&text[open..close])
            .iter()
            .all(|p| swift_labeled(p));
    let parameter = if syntax == Syntax::Swift && !labeled {
        format!("_ {parameter}")
    } else {
        parameter
    };
    let argument = if labeled {
        format!("{name}: {expression}")
    } else {
        expression.clone()
    };

    // Every edit against the file as it is, applied from the last offset backwards.
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    // A Rust function ends on a line of its own, `}`. A Python one ends with its last
    // statement, which is still body, so elsewhere the range ends exactly where the analyzer
    // says.
    let body_end = if syntax == Syntax::Rust {
        crate::signature::offset_of(&text, fn_end, 1)
    } else {
        crate::signature::offset_of(&text, fn_end, declaration.end_col)
    };
    let body_range = close..body_end.unwrap_or(text.len());
    let mut replaced = 0usize;
    if replace_all {
        let mut at = body_range.start;
        while let Some(i) = text[at..body_range.end.min(text.len())].find(&expression) {
            let hit = at + i;
            edits.entry(file.to_path_buf()).or_default().push((
                hit,
                expression.len(),
                name.to_string(),
            ));
            replaced += 1;
            at = hit + expression.len();
        }
    } else {
        edits
            .entry(file.to_path_buf())
            .or_default()
            .push((from, to - from, name.to_string()));
        replaced = 1;
    }
    edits.entry(file.to_path_buf()).or_default().push((
        open,
        close - open,
        syntax.with_parameter(&text[open..close], &parameter),
    ));

    let (fn_line, fn_col) = crate::signature::position_at(&text, fn_offset)?;
    let mut unmatched = Vec::new();
    // A C or C++ function is usually declared in a header and defined in a source file, a
    // method in its class and defined outside it. The declaration must take the parameter too
    // or the definition no longer matches it, and clangd leaves declarations out of
    // `references`, so it is asked for them.
    let declarations = if syntax.is_c_family() {
        declarations(remote, root, file, fn_line, fn_col)
            .await
            .with_context(|| {
                format!("cannot find the declarations of `{callee}`; nothing was planned")
            })?
    } else {
        Vec::new()
    };
    let mut declared: Vec<(PathBuf, usize)> = Vec::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    for (path, dl, dc) in declarations {
        let body = crate::refactor::referenced_text(&mut texts, &path)?.clone();
        let Some(at) = crate::signature::offset_of(&body, dl, dc) else {
            unmatched.push(format!(
                "{}:{dl}:{dc} (a declaration of `{callee}` whose position is not in the file)",
                display(root, &path)
            ));
            continue;
        };
        // An inline definition is its own declaration, and it already has the parameter.
        if path == *file && at == fn_offset {
            continue;
        }
        let list = body[at..]
            .starts_with(bare.as_str())
            .then(|| parameter_list(&body, at + bare.len()))
            .flatten();
        let Some((d_open, d_close)) = list else {
            unmatched.push(format!(
                "{}:{dl}:{dc} (a declaration of `{callee}` whose parameter list is not there)",
                display(root, &path)
            ));
            continue;
        };
        edits.entry(path.clone()).or_default().push((
            d_open,
            d_close - d_open,
            syntax.with_parameter(&body[d_open..d_close], &parameter),
        ));
        declared.push((path, at));
    }

    // Every call site passes what the body used to say.
    let mut call_sites = 0usize;
    let refs = crate::signature::references(remote, root, file, fn_line, fn_col)
        .await
        .with_context(|| format!("cannot find the calls to `{callee}`; nothing was planned"))?;
    for (path, rl, rc) in refs {
        let body = crate::refactor::referenced_text(&mut texts, &path)?.clone();
        let Some(at) = crate::signature::offset_of(&body, rl, rc) else {
            unmatched.push(format!(
                "{}:{rl}:{rc} (the position is not in the file)",
                display(root, &path)
            ));
            continue;
        };
        // The declaration's own name is not a call, whatever the answer includes, and an import
        // names the function without calling it.
        let line_start = body[..at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = body[at..].find('\n').map_or(body.len(), |i| at + i);
        if (path == *file && at == fn_offset)
            || syntax.is_import(&body[line_start..line_end])
            || declared.iter().any(|(p, d)| *p == path && *d == at)
        {
            continue;
        }
        // The analyzer's position is trusted only when the name is actually there. If the file
        // changed since it was analysed, the position points at something else, and appending
        // an argument to whatever call follows it is the one mistake this must never make (#75).
        if !body[at..].starts_with(bare.as_str()) {
            unmatched.push(format!(
                "{}:{rl}:{rc} (the analyzer places `{callee}` here, but the file says otherwise)",
                display(root, &path)
            ));
            continue;
        }
        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(&body, at + bare.len())
        else {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        };
        edits.entry(path).or_default().push((
            args_start,
            args_end - args_start,
            with_argument(&body[args_start..args_end], &argument),
        ));
        call_sites += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = crate::refactor::referenced_text(&mut texts, &path)?.clone();
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
    // A caller the analyzer did not report was not rewritten; checked with the rest, it shows up
    // as an error instead of breaking unseen (#294).
    let unreported = if syntax == Syntax::Rust {
        Vec::new()
    } else {
        let checked: Vec<PathBuf> = rewritten.keys().cloned().collect();
        crate::signature::unreported_callers(root, file, &bare, &checked)
    };
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, &unreported).await?;
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
        // A call left without the argument may be in a file nothing here checks; `force`
        // overrides the analyzer, not a call this did not rewrite (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{callee}` were not given the argument; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Extract something \
             the callers can see, or pass `force: true`:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(ExtractedParameter {
        symbol: callee,
        root: root.to_path_buf(),
        file: display(root, file),
        name: name.to_string(),
        ty,
        parameter,
        expression,
        replaced,
        call_sites,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        unmatched,
        unreported: unreported.iter().map(|p| display(root, p)).collect(),
        diagnostics,
        applied,
    })
}
