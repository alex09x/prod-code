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

use super::apply::{check_and_apply, display, references_with_declarations, unlisted};
use super::binding::{bind_arguments, call_args_in};
use super::c_cpp_decls::collect_c_declarations;
use super::container::{body_span, container_line, item_start_in};
use super::cpp_std::cpp_standard;
use super::effects::{reordered, reordered_arguments};
use super::params::entries;
use super::rewrite::rewritten_call_with;
use super::type_render::{aggregate_text, detected_indent, literal_text, parameter_in, type_text};
use super::types::{Field, Kind, Language, ParameterObject};

/// Bundling in C and C++, where the new type is a `struct` and every declaration of the
/// function changes together: the prototype a header gives callers, and the definition.
///
/// The declarations are the references clangd lists only when asked for declarations too. The
/// parameters are named as the definition names them — a prototype may name them differently,
/// or not at all — and matched to a prototype's by position. The type goes into the header
/// when there is one, above the prototype (above the class, for a method), where the
/// definition and every caller already see it.
///
/// A call passes a C99 compound literal in C, `(struct Opts){.a = x, .b = y}`. In C++ a braced
/// list converts to the parameter's type: `{.a = x, .b = y}` when the project builds as C++20
/// or later, whose designated initialisers these are, and `{x, y}` otherwise. GCC and clang
/// accept designators before C++20 as an extension, so the analyzer's check cannot tell the two
/// apart; the standard the build declares does (see [`cpp_standard`]).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn introduce_c(
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
    // A C struct is named in lower case as often as not, which is a choice, not a mistake.
    anyhow::ensure!(
        !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !name.as_bytes()[0].is_ascii_digit(),
        "`{name}` is not a type name"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset = crate::signature::offset_of(&text, line, col)
        .context("the declaration is not at the resolved position")?;
    let (callee, _, _) = crate::signature::param_span(&text, offset)
        .with_context(|| format!("no function declaration at {}:{line}:{col}", file.display()))?;

    let calls = crate::signature::references(remote, root, file, line, col)
        .await
        .with_context(|| unlisted(&callee, root, file, line, col))?;
    let everything = references_with_declarations(remote, root, file, line, col)
        .await
        .with_context(|| unlisted(&callee, root, file, line, col))?;
    let mut spots = vec![(file.to_path_buf(), line, col)];
    spots.extend(everything.into_iter().filter(|r| !calls.contains(r)));

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let (decls, language) = collect_c_declarations(&spots, &calls, &callee, &mut texts, language);
    let main = decls.iter().position(|d| d.body).unwrap_or(0);
    let count = decls[main].params.len();
    for d in &decls {
        anyhow::ensure!(
            d.params.len() == count,
            "{} declares `{callee}` with {} parameter(s) and {} with {count}; bundling needs \
             every declaration to list the same ones",
            display(root, &d.path),
            d.params.len(),
            display(root, &decls[main].path)
        );
    }
    let mut declared = decls[main].params.clone();
    // C++ gives a default on one declaration only, the header's as a rule.
    for (i, p) in declared.iter_mut().enumerate() {
        if p.default.is_none() {
            p.default = decls.iter().find_map(|d| d.params[i].default.clone());
        }
    }

    for p in params {
        anyhow::ensure!(
            declared.iter().any(|d| &d.name == p),
            "`{p}` is not a parameter of `{callee}`; its definition declares ({})",
            declared
                .iter()
                .filter(|d| !d.name.is_empty())
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let bundled: Vec<usize> = declared
        .iter()
        .enumerate()
        .filter(|(_, d)| !d.name.is_empty() && params.iter().any(|p| p == &d.name))
        .map(|(i, _)| i)
        .collect();
    anyhow::ensure!(bundled.len() == params.len(), "a parameter was named twice");
    for i in &bundled {
        anyhow::ensure!(
            declared[*i].kind == Kind::Plain,
            "`{}` takes a variable number of arguments, and a field holds one value",
            declared[*i].name
        );
    }
    let fields: Vec<Field> = bundled
        .iter()
        .map(|i| {
            let p = &declared[*i];
            Field {
                name: p.name.clone(),
                ty: p.ty.clone(),
                default: p.default.clone().filter(|_| language == Language::Cpp),
                optional: false,
            }
        })
        .collect();

    let home = decls
        .iter()
        .position(|d| crate::lang::is_header(&d.path))
        .unwrap_or(main);
    let home_path = decls[home].path.clone();
    let top = container_line(
        remote,
        root,
        &home_path,
        &decls[home].text,
        decls[home].name_at,
    )
    .await;
    let item_start = item_start_in(&decls[home].text, top, language);
    let indent = detected_indent(&decls[home].text)
        .or_else(|| detected_indent(&decls[main].text))
        .unwrap_or_else(|| "    ".to_string());
    let type_decl = type_text(language, name, &callee, &fields, &indent, false);

    // The uses in the definition's body, at the positions the analyzer reports.
    let def_path = decls[main].path.clone();
    let mut uses: Vec<(usize, usize, String)> = Vec::new();
    if decls[main].body {
        let def = &decls[main];
        let body = body_span(&def.text, def.name_at, def.close, language);
        for i in &bundled {
            let p = &declared[*i];
            let (l, c) = crate::signature::position_at(&def.text, def.open + p.name_at)?;
            let refs = crate::signature::references(remote, root, &def.path, l, c)
                .await
                .with_context(|| unlisted(&p.name, root, &def.path, l, c))?;
            for (path, rl, rc) in refs {
                if path != def.path {
                    continue;
                }
                let Some(o) = crate::signature::offset_of(&def.text, rl, rc) else {
                    continue;
                };
                if o <= body.0 || o >= body.1 || !def.text[o..].starts_with(&p.name) {
                    continue;
                }
                uses.push((o, p.name.len(), format!("{binding}.{}", p.name)));
            }
        }
    }
    uses.sort();
    uses.dedup();
    let body_uses = uses.len();

    let designated =
        language == Language::C || cpp_standard(root, &def_path).is_some_and(|year| year >= 2020);
    let literal = |pairs: &[(String, String)]| {
        if designated {
            literal_text(language, name, pairs)
        } else {
            aggregate_text(pairs)
        }
    };
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    let mut call_sites = 0usize;
    let mut consumed = vec![false; uses.len()];
    for (path, rl, rc) in calls {
        if !texts.contains_key(&path) {
            let t = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}; nothing was written", path.display()))?;
            texts.insert(path.clone(), t);
        }
        let source = &texts[&path];
        let Some(at) = crate::signature::offset_of(source, rl, rc) else {
            unmatched.push(format!(
                "{}:{rl}:{rc} (no such position in the file)",
                display(root, &path)
            ));
            continue;
        };
        // As in Rust (#75): the position is trusted only when the name is there.
        if !source[at..].starts_with(callee.as_str()) {
            unmatched.push(format!(
                "{}:{rl}:{rc} (the analyzer places `{callee}` here, but the file says otherwise)",
                display(root, &path)
            ));
            continue;
        }
        let Some((args_start, args_end)) = call_args_in(source, at + callee.len(), language) else {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        };
        // A recursive call passes uses of the bundled parameters; see `introduce_in`.
        let mut inner = source[args_start..args_end].to_string();
        let mut inside = Vec::new();
        if path == def_path {
            for (n, (o, len, replacement)) in uses.iter().enumerate().rev() {
                if *o >= args_start && o + len <= args_end {
                    inner.replace_range(o - args_start..o - args_start + len, replacement);
                    inside.push(n);
                }
            }
        }
        let args: Vec<String> = entries(&inner, language)
            .into_iter()
            .map(|(_, a)| a.to_string())
            .collect();
        let Some(bound) = bind_arguments(&args, &declared, language) else {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        };
        if let Some((moved, passed)) =
            reordered_arguments(&args, &bound, &bundled, &declared, language)
        {
            let place = format!("{}:{rl}:{rc}", display(root, &path));
            return Err(reordered(&callee, &place, moved, passed, language));
        }
        let new_args = rewritten_call_with(
            &args, &bound, &bundled, &declared, language, binding, literal,
        );
        for n in inside {
            consumed[n] = true;
        }
        edits
            .entry(path)
            .or_default()
            .push((args_start, args_end - args_start, new_args));
        call_sites += 1;
    }
    for (n, used) in uses.into_iter().enumerate() {
        if !consumed[n] {
            edits.entry(def_path.clone()).or_default().push(used);
        }
    }

    // Every declaration, each keeping the parameters it wrote as it wrote them. A C++ default
    // stays on the declaration that gave it; the new parameter gets one, `{}`, which the
    // fields' own defaults fill in, where every bundled parameter had one.
    let first = bundled.first().copied().unwrap_or(0);
    let mut now = String::new();
    for (k, d) in decls.iter().enumerate() {
        let defaulted =
            language == Language::Cpp && bundled.iter().all(|i| d.params[*i].default.is_some());
        let mut out: Vec<String> = Vec::new();
        for (i, p) in d.params.iter().enumerate() {
            if i == first {
                let mut parameter = parameter_in(language, binding, name);
                if defaulted {
                    parameter.push_str(" = {}");
                }
                out.push(parameter);
            } else if !bundled.contains(&i) {
                out.push(p.raw.clone());
            }
        }
        let list = out.join(", ");
        if k == main {
            now = list.clone();
        }
        edits
            .entry(d.path.clone())
            .or_default()
            .push((d.open, d.close - d.open, list));
    }
    edits
        .entry(home_path)
        .or_default()
        .push((item_start, 0, format!("{type_decl}\n")));

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut source = texts.get(&path).cloned().unwrap_or_default();
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
    let was = &decls[main].text[decls[main].open..decls[main].close];
    Ok(ParameterObject {
        symbol: callee,
        root: root.to_path_buf(),
        file: display(root, file),
        struct_text: type_decl,
        was: was.split_whitespace().collect::<Vec<_>>().join(" "),
        now,
        call_sites,
        imports: Vec::new(),
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
