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

use super::apply::{check_and_apply, display, read_referenced, unlisted};
use super::binding::call_args_span;
use super::drop::{
    declared_async, drop_glue, drop_order, dropped_differently, name_in, rust_edition, spelled,
};
use super::effects::{reordered, reordered_arguments};
use super::rewrite::rewritten_args;
use super::rust_types::{parameter_text, struct_text, type_of};
use super::syntax::{matching_bracket, split_args};
use super::types::{Language, ParameterObject, Spelled};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn introduce_rust(
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
    let language = Language::Rust;
    anyhow::ensure!(
        name.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
        "`{name}` is not a type name; Rust types are UpperCamelCase"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset = crate::signature::offset_of(&text, line, col)
        .context("the declaration is not at the resolved position")?;
    let (callee, open, close) = crate::signature::param_span(&text, offset)
        .with_context(|| format!("no function declaration at {}:{line}:{col}", file.display()))?;
    let old_inner = text[open..close].to_string();
    let (receiver, declared) = crate::signature::parse_declared(&old_inner);

    for p in params {
        anyhow::ensure!(
            declared.iter().any(|d| &d.name == p),
            "`{p}` is not a parameter of `{callee}`; it declares ({})",
            declared
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // In declaration order rather than the order the request happens to name them: the
    // struct's field order is the reader's, not the caller's.
    let bundled: Vec<usize> = declared
        .iter()
        .enumerate()
        .filter(|(_, d)| params.iter().any(|p| p == &d.name))
        .map(|(i, _)| i)
        .collect();
    anyhow::ensure!(bundled.len() == params.len(), "a parameter was named twice");

    let fields: Vec<(String, String)> = bundled
        .iter()
        .map(|i| {
            let d = &declared[*i];
            (d.name.clone(), type_of(&d.raw).unwrap_or("()").to_string())
        })
        .collect();

    // A function drops its parameters last to first and a struct its fields first to last, so
    // with two or more that may have a destructor the fields are declared the other way round
    // (#441). The literal names them and is evaluated in the order it writes them.
    //
    // The spelling proves that a reference or a pointer has no destructor, but not that a
    // primitive's name means the builtin: the program may declare or import a `struct bool` with
    // `Drop`. For those the analyzer resolves the parameter's type, and anything short of its
    // word that a resolved type has no drop glue — a failed query included — counts as a type
    // that may have a destructor.
    let mut owned = Vec::with_capacity(declared.len());
    let mut unsure = Vec::new();
    let mut from = open;
    for d in &declared {
        let at = text[from..close].find(&d.raw).map(|o| from + o);
        if let Some(at) = at {
            from = at + d.raw.len();
        }
        let ty = type_of(&d.raw);
        owned.push(match ty.map_or(Spelled::MayDrop, spelled) {
            Spelled::Inert => false,
            Spelled::MayDrop => true,
            Spelled::Primitive => {
                let glue = match at.and_then(|at| Some(at + name_in(&d.raw, &d.name)?)) {
                    Some(at) => drop_glue(remote, root, file, &text, at, &d.name).await,
                    None => Err("its name was not found in the declaration".to_string()),
                };
                if let Err(why) = &glue {
                    unsure.push(format!(
                        "`{}` is spelled `{}`, but {why}",
                        d.name,
                        ty.unwrap_or_default()
                    ));
                }
                glue.is_err()
            }
        });
    }
    let owned_bundled: Vec<&str> = bundled
        .iter()
        .filter(|i| owned[**i])
        .map(|i| declared[*i].name.as_str())
        .collect();
    let reverse = owned_bundled.len() >= 2;
    let order: Vec<usize> = if reverse {
        bundled.iter().rev().copied().collect()
    } else {
        bundled.clone()
    };
    // Where the qualifiers cannot be read, the function is taken to be `async`: that only
    // refuses more.
    let is_async = declared_async(&text, offset).unwrap_or(true);
    let item_line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let quoted = |list: &[usize]| -> String {
        list.iter()
            .map(|i| format!("`{}`", declared[*i].name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for forward in [false, true] {
        if forward && !is_async {
            continue;
        }
        let (was, now) = drop_order(declared.len(), &bundled, &order, &owned, forward);
        if was != now {
            return Err(dropped_differently(
                &callee,
                name,
                &quoted(&bundled),
                &quoted(&was),
                &quoted(&now),
                forward,
                &unsure,
            ));
        }
    }
    // Before edition 2021 a closure or an `async` block that uses `binding.a` captures all of
    // `binding`, and keeps every field alive for as long as it lives.
    let body = text[close..]
        .find(['{', ';'])
        .filter(|b| text.as_bytes()[close + b] == b'{')
        .and_then(|b| matching_bracket(&text, close + b))
        .map_or("", |end| &text[close..end]);
    if !owned_bundled.is_empty() && (body.contains('|') || body.contains("async")) {
        let kept = owned_bundled
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ");
        match rust_edition(file) {
            Ok(edition) => anyhow::ensure!(
                edition >= 2021,
                "`{callee}` is in a crate of edition {edition}, where a closure or an `async` \
                 block that uses a field of `{binding}` captures all of it, and would keep {kept} \
                 (whose types may have a destructor) until it is dropped rather than drop them \
                 when `{callee}` returns (#441). Bundle them in edition 2021 or later, or from a \
                 body without closures; nothing was rewritten"
            ),
            Err(why) => anyhow::bail!(
                "cannot tell which edition the crate of `{callee}` is in ({why}); before edition \
                 2021 a closure or an `async` block that uses a field of `{binding}` captures all \
                 of it, and would keep {kept} (whose types may have a destructor) until it is \
                 dropped rather than drop them when `{callee}` returns (#441). Give the crate's \
                 Cargo.toml an `edition` Cargo accepts, or bundle from a body without closures; \
                 nothing was rewritten"
            ),
        }
    }

    let laid_out: Vec<(String, String)> = if reverse {
        fields.iter().rev().cloned().collect()
    } else {
        fields.clone()
    };
    let doc = if reverse {
        format!(
            "The parameters `{callee}` takes together, last to first: a struct drops its fields \
             first to last, and `{callee}` dropped them last to first."
        )
    } else {
        format!("The parameters `{callee}` takes together.")
    };
    let struct_text = struct_text(name, &laid_out, &doc);

    // Every edit is computed against the file as it is now and applied from the last offset
    // backwards, so no edit has to know what the ones before it did to the offsets.
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    let mut imports = Vec::new();
    let mut call_sites = 0usize;
    let home = crate::move_item::module_of(file).ok().map(|(_, m)| m);

    // How the new type is named in a given file: bare where the file is a module that can
    // import it, the full path where it is not — a file under `tests/` is a crate of its own
    // and `crate::` there means something else.
    let spelling_in = |path: &Path| -> (String, Option<String>) {
        let Some(home) = home.as_ref() else {
            return (name.to_string(), None);
        };
        if path == file {
            return (name.to_string(), None);
        }
        match crate::move_item::module_of(path) {
            Ok((_, theirs)) if theirs == *home => (name.to_string(), None),
            Ok((_, theirs)) => (
                name.to_string(),
                Some(format!("use {}::{name};", home.spelled_from(&theirs.krate))),
            ),
            Err(_) => (format!("{}::{name}", home.absolute()), None),
        }
    };

    let texts = |path: &Path| -> Result<String> { read_referenced(path, file, &text) };

    let callers = crate::signature::references(remote, root, file, line, col)
        .await
        .with_context(|| unlisted(&callee, root, file, line, col))?;
    for (path, rl, rc) in callers {
        let body = texts(&path)?;
        let Some(at) = crate::signature::offset_of(&body, rl, rc) else {
            unmatched.push(format!(
                "{}:{rl}:{rc} (no such position in the file)",
                display(root, &path)
            ));
            continue;
        };
        // The analyzer's position is trusted only when the name is actually there. If the file
        // changed since it was analysed, the position points at something else, and appending
        // an argument to whatever call follows it is the one mistake this must never make (#75).
        if !body[at..].starts_with(callee.as_str()) {
            unmatched.push(format!(
                "{}:{rl}:{rc} (the analyzer places `{callee}` here, but the file says otherwise)",
                display(root, &path)
            ));
            continue;
        }
        // An import or a comment names the function after the change as it did before.
        let code = crate::signature::blank_comments(&body);
        if crate::signature::in_use_or_comment(&body, code.as_deref(), at) {
            continue;
        }
        let after_name = at + callee.len();
        let Some((args_start, args_end)) = call_args_span(&body, after_name) else {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        };
        let args = split_args(&body[args_start..args_end]);
        if args.len() != declared.len() {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        }
        let bound: Vec<Option<usize>> = (0..args.len()).map(Some).collect();
        // A Rust literal converts to nothing of the program's, so no parameter types are needed.
        if let Some((moved, passed)) = reordered_arguments(&args, &bound, &bundled, &[], language) {
            let place = format!("{}:{rl}:{rc}", display(root, &path));
            return Err(reordered(&callee, &place, moved, passed, language));
        }
        let (spelling, _) = spelling_in(&path);
        edits.entry(path).or_default().push((
            args_start,
            args_end - args_start,
            rewritten_args(&args, &bundled, &spelling, &fields),
        ));
        call_sites += 1;
    }

    // The body reaches the bundled parameters through one name now. The analyzer says where
    // each of them is used; a text search would also find them in a string and in a comment.
    let mut body_uses = 0usize;
    for i in &bundled {
        let d = &declared[*i];
        let Some(at) = text[open..close].find(&d.raw).map(|o| open + o) else {
            continue;
        };
        let (l, c) = crate::signature::position_at(&text, at)?;
        let refs = crate::signature::references(remote, root, file, l, c)
            .await
            .with_context(|| unlisted(&d.name, root, file, l, c))?;
        for (path, rl, rc) in refs {
            if path != file || rl == l {
                continue;
            }
            let Some(o) = crate::signature::offset_of(&text, rl, rc) else {
                continue;
            };
            if !text[o..].starts_with(&d.name) {
                continue;
            }
            edits.entry(file.to_path_buf()).or_default().push((
                o,
                d.name.len(),
                format!("{binding}.{}", d.name),
            ));
            body_uses += 1;
        }
    }

    // The declaration itself, and the type above it.
    let now = {
        let mut out: Vec<String> = Vec::new();
        if let Some(r) = &receiver {
            out.push(r.trim().to_string());
        }
        let first = bundled.first().copied().unwrap_or(0);
        for (i, d) in declared.iter().enumerate() {
            if i == first {
                out.push(parameter_text(binding, name, &fields));
            } else if !bundled.contains(&i) {
                out.push(d.raw.trim().to_string());
            }
        }
        out.join(", ")
    };
    let declaring = edits.entry(file.to_path_buf()).or_default();
    declaring.push((open, close - open, now.clone()));
    let item_start = text[..item_line_start]
        .rfind("\n\n")
        .map(|i| i + 2)
        .unwrap_or(item_line_start);
    declaring.push((item_start, 0, format!("{struct_text}\n")));

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts(&path)?;
        file_edits.sort_by_key(|(at, _, _)| *at);
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }

    // A call site in another module of the same crate names the type bare, so it has to import
    // it. The analyzer does not report an unresolved struct literal, so this is not something
    // the type check would catch afterwards.
    let paths: Vec<PathBuf> = rewritten.keys().cloned().collect();
    for path in paths {
        let (_, use_line) = spelling_in(&path);
        let Some(use_line) = use_line else { continue };
        let body = rewritten.get(&path).cloned().unwrap_or_default();
        let with_import = crate::move_item::add_import(&body, &use_line);
        if with_import != body {
            imports.push(format!("{}: added `{use_line}`", display(root, &path)));
        }
        rewritten.insert(path, with_import);
    }

    let (diagnostics, applied) =
        check_and_apply(remote, root, &rewritten, &[], &unmatched, apply, force).await?;

    Ok(ParameterObject {
        symbol: callee,
        root: root.to_path_buf(),
        file: display(root, file),
        struct_text,
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
        unreported: Vec::new(),
        diagnostics,
        applied,
        language: language.fence(),
    })
}
