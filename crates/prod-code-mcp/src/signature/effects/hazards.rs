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
use std::path::Path;

use crate::signature::effects::classify::{SCALARS, classify_arg, is_path, is_pointer};
use crate::signature::parse::split_at_top_level;
use crate::signature::types::{ArgKind, CallSite, Declared, ParamFacts};
use crate::signature::util::line_col_at;

/// The names in `ty` that have to be the built-in types they are spelled as for a value of it to
/// be dropped without running code, with their offsets in `ty`; `None` when it may run code
/// whatever the names are (`String`, a generic `T`, a type of the crate's own). References, raw
/// and function pointers never do, and scalars, `()`, and tuples, arrays and options of those do
/// not — if the names are the built-in ones: `enum Option<T>` with a `Drop`, or a `struct u32`,
/// is spelled the same.
pub fn drop_free_names(ty: &str) -> Option<Vec<(usize, String)>> {
    let mut names = Vec::new();
    drop_free_at(ty, 0, &mut names).then_some(names)
}

pub fn drop_free_at(ty: &str, base: usize, names: &mut Vec<(usize, String)>) -> bool {
    let base = base + (ty.len() - ty.trim_start().len());
    let ty = ty.trim();
    if ty == "()" || is_pointer(ty) {
        return true;
    }
    if SCALARS.contains(&ty) {
        names.push((base, ty.to_string()));
        return true;
    }
    if let Some(inner) = ty.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        return split_at_top_level(inner, ';')
            .is_some_and(|(elem, _)| drop_free_at(elem, base + 1, names));
    }
    if let Some(inner) = ty.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
        let (mut rest, mut at) = (inner, base + 1);
        loop {
            let (elem, tail) = match split_at_top_level(rest, ',') {
                Some((elem, tail)) => (elem, Some(tail)),
                None => (rest, None),
            };
            // `(T,)` ends with an empty element; nothing else may be empty.
            let trailing = tail.is_none() && elem.trim().is_empty();
            if !trailing && !drop_free_at(elem, at, names) {
                return false;
            }
            match tail {
                Some(tail) => {
                    at += elem.len() + 1;
                    rest = tail;
                }
                None => return true,
            }
        }
    }
    if let Some(inner) = ty.strip_prefix("Option<").and_then(|t| t.strip_suffix('>')) {
        names.push((base, "Option".to_string()));
        return drop_free_at(inner, base + "Option<".len(), names);
    }
    false
}

/// Whether converting an argument to `ty` can run code, and if that turns on a name, which one.
/// `Some(None)`: it cannot, whatever the names (a raw or function pointer, a tuple, an array,
/// `()`: no `Deref` makes one). `Some(Some((offset, name)))`: it cannot if `name`, at `offset` in
/// `ty`, is a built-in scalar or a struct, enum or union rather than an alias that may stand for
/// a reference. `None`: it can — a reference is the target of `Deref` coercion, and `impl` or
/// `dyn` are not a type the analyzer can be asked about by name.
pub fn coercion_name(ty: &str) -> Option<Option<(usize, String)>> {
    let base = ty.len() - ty.trim_start().len();
    let ty = ty.trim();
    if ty.starts_with('&') {
        return None;
    }
    if ty == "()" || is_pointer(ty) || ty.starts_with('(') || ty.starts_with('[') {
        return Some(None);
    }
    let path = &ty[..ty.find('<').unwrap_or(ty.len())];
    if !is_path(path) || (path.len() < ty.len() && !ty.ends_with('>')) {
        return None;
    }
    let at = path.rfind("::").map_or(0, |i| i + 2);
    Some(Some((base + at, path[at..].to_string())))
}

/// The code blocks of a hover before its documentation, without their language tags.
pub fn hover_blocks(markdown: &str) -> Vec<&str> {
    let head = markdown.split("\n---").next().unwrap_or("");
    head.split("```")
        .skip(1)
        .step_by(2)
        .map(|block| block.split_once('\n').map_or("", |(_, body)| body).trim())
        .collect()
}

/// Whether a hover over `name` says it is the built-in scalar, or the standard library's
/// `Option`, that the name usually means. rust-analyzer describes a built-in type by its name
/// alone, and a declared one by the module it is in and then the declaration: a `struct u32` or
/// an `enum Option<T>` of the crate's own reads `crate_name` and `struct u32`.
pub fn hover_is_builtin(markdown: &str, name: &str) -> bool {
    let blocks = hover_blocks(markdown);
    if name == "Option" {
        return blocks.len() == 2
            && matches!(blocks[0], "core::option" | "std::option")
            && blocks[1].starts_with("pub enum Option<");
    }
    SCALARS.contains(&name) && blocks == [name]
}

/// Whether a hover over `name` says it is a struct, an enum or a union, as opposed to a type
/// alias or a generic parameter, either of which may stand for a reference.
pub fn hover_is_adt(markdown: &str, name: &str) -> bool {
    let blocks = hover_blocks(markdown);
    let [_, item] = blocks[..] else {
        return false;
    };
    let item = match item.strip_prefix("pub(") {
        Some(rest) => rest.split_once(')').map_or("", |(_, r)| r).trim_start(),
        None => item.strip_prefix("pub ").unwrap_or(item),
    };
    ["struct ", "enum ", "union "].iter().any(|kw| {
        item.strip_prefix(kw).is_some_and(|rest| {
            rest.strip_prefix(name)
                .is_some_and(|after| !after.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
        })
    })
}

/// The hover's markdown at byte `at` of `file`, asked once per place; `None` when it fails or
/// has none, which confirms nothing.
pub async fn hover_markdown(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    at: usize,
    asked: &mut BTreeMap<usize, Option<String>>,
) -> Option<String> {
    if let Some(known) = asked.get(&at) {
        return known.clone();
    }
    let (line, col) = line_col_at(text, at)?;
    let uri = url::Url::from_file_path(file).ok()?.to_string();
    let hover = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line - 1, "character": col - 1 },
        }),
    )
    .await
    .ok();
    let markdown = hover
        .as_ref()
        .and_then(|h| h.get("contents"))
        .and_then(|c| {
            c.as_str()
                .or_else(|| c.get("value").and_then(|v| v.as_str()))
        })
        .map(str::to_string);
    asked.insert(at, markdown.clone());
    markdown
}

/// What the analyzer confirms about the type of each declared parameter, asked by hovering the
/// names in the declaration's own text (its parameter list is `text[open..close]`). A name the
/// analyzer does not describe as expected is not taken on trust.
pub async fn param_facts(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    open: usize,
    close: usize,
    declared: &[Declared],
) -> Vec<ParamFacts> {
    let mut asked = BTreeMap::new();
    let mut out = Vec::with_capacity(declared.len());
    // In order, each after the one before: `a: u32` is also the end of `aa: u32`.
    let mut from = open;
    for d in declared {
        let mut facts = ParamFacts::default();
        let found = text[from..close].find(&d.raw).map(|i| from + i);
        let typed = found.and_then(|at| {
            from = at + d.raw.len();
            split_at_top_level(&d.raw, ':').map(|(head, ty)| (at + head.len() + 1, ty))
        });
        let Some((ty_at, ty)) = typed else {
            out.push(facts);
            continue;
        };
        facts.reference = ty.trim_start().starts_with('&');
        if let Some(names) = drop_free_names(ty) {
            facts.drop_free = true;
            for (offset, name) in names {
                let hover = hover_markdown(remote, root, file, text, ty_at + offset, &mut asked);
                if !hover.await.is_some_and(|h| hover_is_builtin(&h, &name)) {
                    facts.drop_free = false;
                    if !facts.unconfirmed.contains(&name) {
                        facts.unconfirmed.push(name);
                    }
                }
            }
        }
        facts.coercion_free = match coercion_name(ty) {
            None => false,
            Some(None) => true,
            Some(Some((offset, name))) => {
                hover_markdown(remote, root, file, text, ty_at + offset, &mut asked)
                    .await
                    .is_some_and(|h| hover_is_builtin(&h, &name) || hover_is_adt(&h, &name))
            }
        };
        out.push(facts);
    }
    out
}

/// What the new argument order `args` and the parameters it leaves out would change at run time,
/// one line per place: two arguments that would be evaluated the other way round when either
/// can have an effect the other sees, two owned parameters that would be dropped the other way
/// round, and a removed argument that does something or is an owned value the function drops.
/// `facts` is what the analyzer confirmed about each parameter's type; what it did not confirm
/// counts as able to run code. The receiver is evaluated first before and after, so it is not an
/// argument here; a call written `Type::f(recv, …)` passes it first and it is skipped.
pub fn effect_hazards(
    name: &str,
    declared: &[Declared],
    facts: &[ParamFacts],
    has_receiver: bool,
    args: &[Option<usize>],
    calls: &[CallSite],
) -> Vec<String> {
    let unknown = ParamFacts::default();
    let fact = |i: usize| facts.get(i).unwrap_or(&unknown);
    let kept: Vec<usize> = args.iter().flatten().copied().collect();
    // (i, j), declared i before j, that the new order passes j before i.
    let mut swapped = Vec::new();
    for (p, &later) in kept.iter().enumerate() {
        for &earlier in &kept[p + 1..] {
            if earlier < later {
                swapped.push((earlier, later));
            }
        }
    }
    let removed: Vec<usize> = (0..declared.len()).filter(|i| !kept.contains(i)).collect();
    let mut out = Vec::new();
    for &(i, j) in &swapped {
        let (a, b) = (&declared[i], &declared[j]);
        if !fact(i).drop_free && !fact(j).drop_free {
            let mut line = format!(
                "`{name}` drops `{}` before `{}` when it returns (parameters are dropped in \
                 reverse order of declaration); the new order drops `{}` first",
                b.raw.trim(),
                a.raw.trim(),
                a.name
            );
            let mut unconfirmed: Vec<&str> = Vec::new();
            for n in fact(i).unconfirmed.iter().chain(&fact(j).unconfirmed) {
                if !unconfirmed.contains(&n.as_str()) {
                    unconfirmed.push(n);
                }
            }
            if !unconfirmed.is_empty() {
                line.push_str(&format!(
                    " (the analyzer does not confirm that `{}` is the built-in or standard \
                     library type the name usually means, and a type of that name may have a \
                     `Drop`)",
                    unconfirmed.join("`, `")
                ));
            }
            out.push(line);
        }
    }
    for call in calls {
        let own = if has_receiver && call.args.len() == declared.len() + 1 {
            &call.args[1..]
        } else {
            &call.args[..]
        };
        if own.len() != declared.len() {
            out.push(format!(
                "{}: the call passes {} argument(s) and `{name}` declares {}, so what the \
                 rewrite does to it cannot be checked",
                call.at,
                own.len(),
                declared.len()
            ));
            continue;
        }
        let kinds: Vec<ArgKind> = own
            .iter()
            .enumerate()
            .map(|(i, a)| classify_arg(a, fact(i)))
            .collect();
        for &d in &removed {
            let param = &declared[d];
            match kinds[d] {
                ArgKind::Effectful => out.push(format!(
                    "{}: `{}` is evaluated for `{}`, and removing the parameter removes what it \
                     does",
                    call.at, own[d], param.name
                )),
                ArgKind::Unproven(why) => out.push(format!(
                    "{}: `{}` is evaluated for `{}`, and removing the parameter may remove what \
                     it does: {why}",
                    call.at, own[d], param.name
                )),
                _ if !fact(d).drop_free => out.push(format!(
                    "{}: `{}` is moved into `{}` and dropped when `{name}` returns; without the \
                     parameter it is dropped at another time, or not at all",
                    call.at,
                    own[d],
                    param.raw.trim()
                )),
                _ => {}
            }
        }
        for &(i, j) in &swapped {
            let independent = kinds[i] == ArgKind::Literal
                || kinds[j] == ArgKind::Literal
                || (kinds[i] == ArgKind::Place && kinds[j] == ArgKind::Place);
            if !independent {
                let mut line = format!(
                    "{}: `{}` and `{}` would be evaluated in the opposite order",
                    call.at, own[i], own[j]
                );
                let mut why: Vec<&str> = Vec::new();
                for kind in [kinds[i], kinds[j]] {
                    if let ArgKind::Unproven(reason) = kind
                        && !why.contains(&reason)
                    {
                        why.push(reason);
                    }
                }
                if !why.is_empty() {
                    line.push_str(&format!(" ({})", why.join("; ")));
                }
                out.push(line);
            }
        }
    }
    out
}
