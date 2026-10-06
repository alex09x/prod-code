/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod rewrite;

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::parameter_object::Language;
use crate::wrap_return::types::{WrappedReturn, Wrapper};
use crate::wrap_return::utils::{declared_return, display, enclosing_return_type, propagates};
use rewrite::rewrite_rust_body;

/// Wraps the return type of the function declared at `line`:`col` (or `symbol`) of `file` for Rust.
#[allow(clippy::too_many_arguments)]
pub async fn wrap_rust(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: u32,
    col: u32,
    wrapper: Wrapper,
    error: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<WrappedReturn> {
    wrap_rust_ext(
        remote, root, file, symbol, line, col, wrapper, None, error, apply, force,
    )
    .await
}

/// Wraps the return type of a Rust function with optional custom constructor.
#[allow(clippy::too_many_arguments)]
pub async fn wrap_rust_ext(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: u32,
    col: u32,
    wrapper: Wrapper,
    constructor: Option<&str>,
    error: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<WrappedReturn> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = if line > 0 && col > 0 {
        crate::signature::offset_of(&text, line, col)
            .context("the position is not inside the file")?
    } else if let Some(sym) = symbol {
        let needle = format!("fn {sym}");
        let pos = text
            .find(&needle)
            .with_context(|| format!("function `{sym}` not found in {}", file.display()))?;
        pos + 3
    } else {
        anyhow::bail!("provide either line and character or symbol");
    };
    // The name the position is in, however far into it.
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(at, |(i, _)| i);
    anyhow::ensure!(
        text[..start].trim_end().ends_with("fn"),
        "the position is not the name of a function declaration"
    );
    let (name, _, close) =
        crate::signature::param_span(&text, start).context("the function has no parameter list")?;
    let (ret_start, ret_end) = declared_return(&text, close).with_context(|| {
        format!("`{name}` returns `()` implicitly; declare `-> ()` first if that is what is meant")
    })?;
    let was = text[ret_start..ret_end].to_string();
    anyhow::ensure!(
        !propagates(&was, &wrapper),
        "`{name}` already returns a `{}`",
        wrapper.name()
    );
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());

    let (new_decl, now) = match &wrapper {
        Wrapper::Custom(custom_name) => {
            let base_name = custom_name
                .split(['<', '['])
                .next()
                .unwrap_or(custom_name)
                .trim();
            let base_name = base_name.rsplit("::").next().unwrap_or(base_name).trim();
            let now = if custom_name.contains('<') {
                custom_name
                    .replace("<T>", &format!("<{was}>"))
                    .replace("<>", &format!("<{was}>"))
            } else {
                format!("{custom_name}<{was}>")
            };
            let body_open = text[close..]
                .find('{')
                .map(|i| close + i)
                .context("function declaration has no body")?;
            let body_close = crate::parameter_object::matching_bracket(&text, body_open)
                .context("unmatched bracket in function body")?;
            let body_text = &text[body_open + 1..body_close];
            let rewritten_body = rewrite_rust_body(body_text, constructor, base_name, &was);
            let mut new_text = text.clone();
            new_text.replace_range(body_open + 1..body_close, &rewritten_body);
            new_text.replace_range(ret_start..ret_end, &now);
            (new_text, now)
        }
        Wrapper::Option | Wrapper::Result => {
            let error = match wrapper {
                Wrapper::Result => Some(error.map(str::trim).filter(|e| !e.is_empty()).context(
                    "pass `error`: the type a `Result` fails with, such as `anyhow::Error`",
                )?),
                _ => None,
            };

            let (rl, rc) = crate::signature::position_at(&text, ret_start)?;
            let uri = url::Url::from_file_path(file)
                .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
                .to_string();
            let edit = crate::tools::execute_lsp_query(
                remote,
                root,
                file,
                "prodCode/applyAssist",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": rl - 1, "character": rc - 1 },
                        "end": { "line": rl - 1, "character": rc - 1 }
                    },
                    "id": wrapper.assist_id(),
                }),
            )
            .await
            .with_context(|| {
                format!("rust-analyzer does not wrap the return type of `{name}` here")
            })?;
            let (planned, _) = crate::refactor::planned_texts(root, &edit)?;
            let mut decl_text = planned
                .into_iter()
                .find(|(p, _)| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == canonical)
                .map(|(_, t)| t)
                .context("the assist did not rewrite the declaring file")?;
            let now = match error {
                Some(error) => {
                    let sig_at = decl_text.find(&format!("fn {name}")).unwrap_or(0);
                    let body_at = decl_text[sig_at..]
                        .find('{')
                        .map_or(decl_text.len(), |i| sig_at + i);
                    let hole = decl_text[sig_at..body_at]
                        .rfind(", _>")
                        .map(|i| sig_at + i)
                        .context("the wrapped signature has no `_` error type to fill in")?;
                    decl_text.replace_range(hole..hole + ", _>".len(), &format!(", {error}>"));
                    format!("Result<{was}, {error}>")
                }
                None => format!("Option<{was}>"),
            };
            (decl_text, now)
        }
        _ => {
            anyhow::bail!("Rust wrap_return supports `option`, `result`, or a custom envelope type")
        }
    };

    // Where the declaring file did not change, a position means the same thing before and after.
    let prefix = text
        .bytes()
        .zip(new_decl.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = text
        .bytes()
        .rev()
        .zip(new_decl.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(text.len() - prefix);
    let delta = new_decl.len() as isize - text.len() as isize;

    // The function's own span in the file as it is: a call inside it is a recursive call, whose
    // text the assist itself rewrote.
    let own_end = text[close..]
        .find('{')
        .and_then(|i| crate::parameter_object::matching_bracket(&text, close + i))
        .unwrap_or(close);
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut edits: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    let mut propagated = 0usize;
    let mut blocked = Vec::new();
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
        if !body[at..].starts_with(name.as_str()) {
            unmatched.push(format!(
                "{site} (the analyzer places `{name}` here, but the file says otherwise)"
            ));
            continue;
        }
        if crate::inline_parameter::is_in_comment(&body, at, Language::Rust) {
            continue;
        }
        if crate::inline_parameter::is_import_or_export_context(&body, at, Language::Rust) {
            continue;
        }
        let Some((_, args_end)) = crate::parameter_object::call_args_span(&body, at + name.len())
        else {
            unmatched.push(format!("{site} (not a call: a function used as a value)"));
            continue;
        };
        let same_file = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()) == canonical;
        if same_file && start < at && at < own_end {
            unmatched.push(format!(
                "{site} (a call inside `{name}` itself: add wrapper there by hand)"
            ));
            continue;
        }
        let caller = enclosing_return_type(&body, at).unwrap_or_else(|| "()".to_string());
        if !propagates(&caller, &wrapper) {
            let line_text = body[body[..at].rfind('\n').map_or(0, |i| i + 1)..]
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            blocked.push(format!(
                "{site} the caller returns `{caller}`: `{line_text}`"
            ));
            continue;
        }
        if matches!(wrapper, Wrapper::Option | Wrapper::Result) {
            let insert_at = args_end + 1;
            // Elsewhere, and before the part the assist rewrote, a position is unchanged.
            let mapped = if !same_file || insert_at <= prefix {
                insert_at
            } else if insert_at >= text.len() - suffix {
                (insert_at as isize + delta) as usize
            } else {
                unmatched.push(format!(
                    "{site} (a call inside `{name}` itself: add `?` there by hand)"
                ));
                continue;
            };
            edits.entry(path.clone()).or_default().push(mapped);
        }
        propagated += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    rewritten.insert(file.to_path_buf(), new_decl);
    for (path, mut spots) in edits {
        let same_file = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()) == canonical;
        let key = if same_file {
            file.to_path_buf()
        } else {
            path.clone()
        };
        let mut body = rewritten
            .get(&key)
            .cloned()
            .unwrap_or_else(|| texts.get(&path).cloned().unwrap_or_default());
        spots.sort_unstable();
        for spot in spots.into_iter().rev() {
            body.insert(spot, '?');
        }
        rewritten.insert(key, body);
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
            "{} call site(s) cannot propagate; nothing was written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
        );
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{name}` were not rewritten; nothing was written:\n  {}",
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

    Ok(WrappedReturn {
        function: name,
        root: root.to_path_buf(),
        file: display(root, file),
        was,
        now,
        propagated,
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
