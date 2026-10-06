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

use super::apply::{check_and_apply, display, read_referenced};
use super::container::{dataclass_import, item_start_in, python_import_edit, top_level_line};
use super::effects::js_constant;
use super::params::parse_params;
use super::polyglot_callers::{collect_body_uses, rewrite_polyglot_callers};
use super::rewrite::ident_uses;
use super::syntax::is_ident_byte;
use super::type_render::{hover_type, indent_unit, parameter_in, type_text};
use super::types::{Field, Kind, Language, ParameterObject};

/// Bundling in a TypeScript, JavaScript, Python, Go or Swift file: the same three edits as in
/// Rust, with the text each language writes them in.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn introduce_in(
    language: Language,
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    params: &[String],
    name: &str,
    binding: &str,
    apply: bool,
    force: bool,
) -> Result<ParameterObject> {
    anyhow::ensure!(
        !name.is_empty() && name.bytes().all(is_ident_byte) && !name.as_bytes()[0].is_ascii_digit(),
        "`{name}` is not a type name"
    );
    // A Go type in lower case is an unexported one, which is a choice, not a mistake. A
    // JavaScript name only names the shape; nothing is declared under it.
    anyhow::ensure!(
        matches!(language, Language::Go | Language::JavaScript)
            || name.starts_with(|c: char| c.is_ascii_uppercase()),
        "`{name}` is not a type name; {} types are UpperCamelCase",
        language.label()
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset = crate::signature::offset_of(&text, line, col)
        .context("the declaration is not at the resolved position")?;
    let (callee, open, close) = crate::signature::param_span(&text, offset)
        .with_context(|| format!("no function declaration at {}:{line}:{col}", file.display()))?;
    let old_inner = text[open..close].to_string();
    let (receiver, declared) = parse_params(&old_inner, language);

    for p in params {
        // A name a destructuring pattern binds is not a parameter a call passes by position.
        if language == Language::JavaScript
            && let Some(pattern) = declared
                .iter()
                .find(|d| d.name.is_empty() && !ident_uses(&d.raw, 0, d.raw.len(), p).is_empty())
        {
            anyhow::bail!(
                "`{p}` is bound by the destructuring pattern `{}` of `{callee}`, not a parameter \
                 of its own; the call passes the pattern one value, so it is not bundled",
                pattern.raw
            );
        }
        anyhow::ensure!(
            declared
                .iter()
                .any(|d| &d.name == p && d.kind != Kind::Marker),
            "`{p}` is not a parameter of `{callee}`; it declares ({})",
            declared
                .iter()
                .filter(|d| d.kind != Kind::Marker)
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let bundled: Vec<usize> = declared
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind != Kind::Marker && params.iter().any(|p| p == &d.name))
        .map(|(i, _)| i)
        .collect();
    anyhow::ensure!(bundled.len() == params.len(), "a parameter was named twice");
    for i in &bundled {
        anyhow::ensure!(
            declared[*i].kind == Kind::Plain,
            "`{}` takes a variable number of arguments, and a field holds one value",
            declared[*i].name
        );
        // What the body writes to an `inout` parameter reaches the caller; written to a field
        // of a copy, it would not.
        anyhow::ensure!(
            !(language == Language::Swift
                && declared[*i]
                    .ty
                    .as_deref()
                    .is_some_and(|t| t.starts_with("inout "))),
            "`{}` is `inout`, and a field of the new type would be a copy of it",
            declared[*i].name
        );
        // The callers write a JavaScript or TypeScript default into the object now. A constant
        // means the same there; anything else runs in the function's scope, on every call,
        // after the parameters before it are bound.
        if matches!(language, Language::JavaScript | Language::TypeScript)
            && let Some(default) = &declared[*i].default
        {
            anyhow::ensure!(
                js_constant(default),
                "`{}` defaults to `{default}`, which is evaluated in `{callee}` on every call; \
                 written at the callers it could mean something else, so only a constant \
                 default is bundled",
                declared[*i].name
            );
        }
    }

    let mut fields = Vec::with_capacity(bundled.len());
    for i in &bundled {
        let p = &declared[*i];
        let ty = match &p.ty {
            Some(ty) => Some(ty.clone()),
            // No type is written in JavaScript, so none is asked for.
            None if language == Language::JavaScript => None,
            None => hover_type(remote, root, file, &text, open + p.name_at, &p.name).await,
        };
        fields.push(Field {
            name: p.name.clone(),
            ty,
            // A TypeScript interface has no defaults; the field keeps the parameter's type.
            default: p.default.clone().filter(|_| {
                matches!(
                    language,
                    Language::Python | Language::Swift | Language::JavaScript
                )
            }),
            optional: p.optional,
        });
    }

    let decl_line = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let top = top_level_line(&text, offset);
    let item_start = item_start_in(&text, top, language);
    let export = match language {
        Language::TypeScript => text[top..].starts_with("export "),
        Language::Swift => {
            let modifiers = &text[decl_line..offset];
            modifiers.contains("public ") || modifiers.contains("open ")
        }
        _ => false,
    };
    let is_method = match language {
        Language::Go => text[decl_line..offset].trim_start().starts_with("func ("),
        _ => receiver.is_some() || top != decl_line,
    };
    let type_decl = type_text(
        language,
        name,
        &callee,
        &fields,
        &indent_unit(&text, language),
        export,
    );

    let uses = collect_body_uses(
        remote, root, file, &text, offset, open, close, &declared, &bundled, binding, language,
        &callee,
    )
    .await?;
    let body_uses = uses.len();

    let texts = |path: &Path| -> Result<String> { read_referenced(path, file, &text) };
    let callers = rewrite_polyglot_callers(
        remote, root, file, line, col, language, &callee, name, binding, is_method, &declared,
        &bundled, &uses, &texts,
    )
    .await?;

    let mut edits = callers.edits;
    let unmatched = callers.unmatched;
    let mut imports = Vec::new();
    let call_sites = callers.call_sites;
    let consumed = callers.consumed;
    let bare_callers = callers.bare_callers;

    for (n, used) in uses.into_iter().enumerate() {
        if !consumed[n] {
            edits.entry(file.to_path_buf()).or_default().push(used);
        }
    }

    // The declaration itself. A Swift parameter keeps the first bundled one's lack of a label,
    // and a default when every bundled one had a default, so that a call that passed none of
    // them still compiles unchanged.
    let mut parameter = parameter_in(language, binding, name);
    if language == Language::Swift {
        if bundled
            .first()
            .is_some_and(|p| declared[*p].label.is_none())
        {
            parameter = format!("_ {parameter}");
        }
        if bundled.iter().all(|p| declared[*p].default.is_some()) {
            parameter.push_str(&format!(" = {name}()"));
        }
    }
    let now = {
        let mut out: Vec<String> = Vec::new();
        if let Some(r) = &receiver {
            out.push(r.trim().to_string());
        }
        let first = bundled.first().copied().unwrap_or(0);
        for (i, p) in declared.iter().enumerate() {
            if i == first {
                out.push(parameter.clone());
            } else if bundled.contains(&i) {
                continue;
            } else if p.shares_type && bundled.contains(&(i + 1)) {
                // The name the type was written after is leaving the group.
                out.push(format!(
                    "{} {}",
                    p.name,
                    p.ty.as_deref().unwrap_or_default()
                ));
            } else {
                out.push(p.raw.clone());
            }
        }
        out.join(", ")
    };
    let declaring = edits.entry(file.to_path_buf()).or_default();
    declaring.push((open, close - open, now.clone()));
    // The import is pushed before the type: at the same offset, the one pushed first ends up
    // first in the file.
    if type_decl.starts_with("@dataclass")
        && let Some((at, line)) = dataclass_import(&text)
    {
        declaring.push((at, 0, line));
        imports.push(format!(
            "{}: added `from dataclasses import dataclass`",
            display(root, file)
        ));
    }
    // Python separates top-level definitions with two blank lines, the others with one.
    let gap = if language == Language::Python {
        "\n\n"
    } else {
        "\n"
    };
    // A JavaScript object has no type to declare; the text is only the report's.
    if language != Language::JavaScript {
        declaring.push((item_start, 0, format!("{type_decl}{gap}")));
    }

    // A Python caller in another module names the type bare, so it has to import it, from the
    // module it already imports the function (or the class) from.
    let stem = match file.file_stem().and_then(|s| s.to_str()) {
        Some("__init__") => file
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or(""),
        Some(stem) => stem,
        None => "",
    };
    for path in bare_callers {
        let source = texts(&path)?;
        match python_import_edit(&source, stem, name) {
            Some((_, insert)) if insert.is_empty() => {}
            Some((at, insert)) => {
                edits.entry(path.clone()).or_default().push((at, 0, insert));
                imports.push(format!(
                    "{}: added `{name}` to the import from `{stem}`",
                    display(root, &path)
                ));
            }
            None => imports.push(format!(
                "{}: needs `{name}` imported; it has no `from … import` of `{stem}` to add it to",
                display(root, &path)
            )),
        }
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut source = texts(&path)?;
        file_edits.sort_by_key(|(at, _, _)| *at);
        for (at, len, replacement) in file_edits.into_iter().rev() {
            source.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, source);
    }

    // A caller the analyzer did not report was not rewritten; checked with the rest, it shows
    // up as an error instead of breaking unseen (#294).
    let checked: Vec<PathBuf> = rewritten.keys().cloned().collect();
    let unreported = crate::signature::unreported_callers(root, file, &callee, &checked);
    let (diagnostics, applied) = check_and_apply(
        remote,
        root,
        &rewritten,
        &unreported,
        &unmatched,
        apply,
        force,
    )
    .await?;

    Ok(ParameterObject {
        symbol: callee,
        root: root.to_path_buf(),
        file: display(root, file),
        struct_text: type_decl,
        was: old_inner.split_whitespace().collect::<Vec<_>>().join(" "),
        now,
        call_sites,
        imports,
        body_uses,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        unmatched,
        unreported: unreported.iter().map(|p| display(root, p)).collect(),
        diagnostics,
        applied,
        language: language.fence(),
    })
}
