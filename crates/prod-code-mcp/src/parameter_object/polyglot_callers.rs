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

use super::apply::{display, unlisted};
use super::binding::{bind_arguments, call_args_in};
use super::container::{body_span, in_import, qualifier_before};
use super::effects::{js_constant, reordered, reordered_arguments};
use super::params::entries;
use super::rewrite::{called_name, ident_uses, object_shorthand, rewritten_call};
use super::type_render::js_key;
use super::types::{Language, Param};

pub(crate) struct PolyglotCallers {
    pub edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>>,
    pub unmatched: Vec<String>,
    pub call_sites: usize,
    pub consumed: Vec<bool>,
    pub bare_callers: Vec<PathBuf>,
}

/// Collects references to the bundled parameters inside the function's own body.
pub(crate) async fn collect_body_uses(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    offset: usize,
    open: usize,
    close: usize,
    declared: &[Param],
    bundled: &[usize],
    binding: &str,
    language: Language,
    callee: &str,
) -> Result<Vec<(usize, usize, String)>> {
    let body = body_span(text, offset, close, language);
    let mut uses: Vec<(usize, usize, String)> = Vec::new();
    for i in bundled {
        let p = &declared[*i];
        let (l, c) = crate::signature::position_at(text, open + p.name_at)?;
        let refs = crate::signature::references(remote, root, file, l, c)
            .await
            .with_context(|| unlisted(&p.name, root, file, l, c))?;
        for (path, rl, rc) in refs {
            if path != file {
                continue;
            }
            // A use the analyzer places where the file has something else was not rewritten,
            // and in JavaScript no check afterwards would say so: the body would read a name
            // that is gone.
            let o = crate::signature::offset_of(text, rl, rc)
                .filter(|o| text[*o..].starts_with(&p.name));
            anyhow::ensure!(
                o.is_some() || language != Language::JavaScript,
                "the analyzer places a use of `{}` at {}:{rl}:{rc}, but the file says otherwise; \
                 it changed since the analyzer read it, so nothing was rewritten",
                p.name,
                display(root, file)
            );
            let Some(o) = o else {
                continue;
            };
            if o <= body.0 || o >= body.1 {
                // Another parameter's default reads this one, and would lose it. TypeScript's
                // checker says so after the rewrite; in JavaScript nothing would.
                anyhow::ensure!(
                    language != Language::JavaScript || o == open + p.name_at,
                    "`{}` is read at {}:{rl}:{rc}, outside the body of `{callee}` (another \
                     parameter's default); it would not be in scope as a field",
                    p.name,
                    display(root, file)
                );
                continue;
            }
            // basedpyright counts the name of a keyword argument (`height=…`) as a reference;
            // in a call in the body that is the callee's parameter, not a use of this one.
            let after = text[o + p.name.len()..].trim_start();
            if language == Language::Python && after.starts_with('=') && !after.starts_with("==") {
                continue;
            }
            let field = if language == Language::Java {
                format!("{binding}.{}()", p.name)
            } else {
                format!("{binding}.{}", p.name)
            };
            let replacement = if language == Language::JavaScript
                && object_shorthand(text, body.0, o, p.name.len())
            {
                format!("{}: {field}", js_key(&p.name))
            } else {
                field
            };
            uses.push((o, p.name.len(), replacement));
        }
    }
    uses.sort();
    uses.dedup();

    if language == Language::JavaScript {
        let line_of = |at: usize| crate::signature::position_at(text, at).map(|(line, _)| line);
        // `arguments` still counts and orders the arguments the call passed, which bundling
        // changes. A nested function has its own, but it is refused too rather than told apart.
        if let Some(at) = ident_uses(text, body.0, body.1, "arguments").first() {
            anyhow::bail!(
                "`{callee}` reads `arguments` ({}:{}), whose length and order bundling changes; \
                 it is not bundled",
                display(root, file),
                line_of(*at)?
            );
        }
        // A name the function already has would shadow the object, or be shadowed by it; the
        // analyzer's references are to the parameters, not to the binding that replaces them.
        let taken = ident_uses(text, open, body.1, binding)
            .into_iter()
            .find(|at| {
                !uses.iter().any(|(o, _, _)| o == at)
                    && !bundled.iter().any(|i| open + declared[*i].name_at == *at)
            });
        if let Some(at) = taken {
            anyhow::bail!(
                "`{binding}` is already a name in `{callee}` ({}:{}); the object would shadow \
                 it or be shadowed by it. Pass another `binding`",
                display(root, file),
                line_of(at)?
            );
        }
    }
    Ok(uses)
}

/// Rewrites call sites of the function across all referencing files.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn rewrite_polyglot_callers(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    language: Language,
    callee: &str,
    name: &str,
    binding: &str,
    is_method: bool,
    declared: &[Param],
    bundled: &[usize],
    uses: &[(usize, usize, String)],
    texts: &impl Fn(&Path) -> Result<String>,
) -> Result<PolyglotCallers> {
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    let mut call_sites = 0usize;
    let mut consumed = vec![false; uses.len()];
    let mut bare_callers: Vec<PathBuf> = Vec::new();
    let callers = crate::signature::references(remote, root, file, line, col)
        .await
        .with_context(|| unlisted(callee, root, file, line, col))?;

    for (path, rl, rc) in callers {
        let source = texts(&path)?;
        let place = format!("{}:{rl}:{rc}", display(root, &path));
        // As in Rust (#75): the position is trusted only when the name is there. A JavaScript
        // call left as it was would pass the old arguments to the new parameter, and only a
        // type checker would see that, so there it stops the whole change.
        let called = crate::signature::offset_of(&source, rl, rc)
            .and_then(|at| Some((at, called_name(&source, at, callee, language)?)));
        anyhow::ensure!(
            called.is_some() || language != Language::JavaScript,
            "the analyzer places `{callee}` at {place}, but the file says otherwise; it changed \
             since the analyzer read it, so nothing was rewritten"
        );
        let Some((at, called)) = called else {
            unmatched.push(if crate::signature::offset_of(&source, rl, rc).is_some() {
                format!(
                    "{place} (the analyzer places `{callee}` here, but the file says otherwise)"
                )
            } else {
                format!("{place} (no such position in the file)")
            });
            continue;
        };
        // `build.call(receiver, …)` passes the receiver first and the arguments after it;
        // `apply` passes them in an array that only the running call can take apart.
        let mut after_name = at + called;
        let mut through_call = false;
        if language == Language::JavaScript {
            let rest = &source[after_name..];
            let method = |m: &str| {
                rest.starts_with(m)
                    && !rest
                        .as_bytes()
                        .get(m.len())
                        .is_some_and(|b| super::syntax::is_ident_byte(*b))
            };
            anyhow::ensure!(
                !method(".apply"),
                "`{callee}` is called through `apply` at {place}, with its arguments in an array; \
                 which of them are bundled is known only when it runs"
            );
            if method(".call") {
                after_name += ".call".len();
                through_call = true;
            }
        }
        let Some((args_start, args_end)) = call_args_in(&source, after_name, language) else {
            if !in_import(&source, at, language) {
                unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            }
            continue;
        };
        // A call in the function's own body passes arguments that are uses of the bundled
        // parameters themselves. One edit cannot sit inside another, so those uses are
        // rewritten inside the argument text.
        let mut inner = source[args_start..args_end].to_string();
        let mut inside = Vec::new();
        if path == file {
            for (n, (o, len, replacement)) in uses.iter().enumerate().rev() {
                if *o >= args_start && o + len <= args_end {
                    inner.replace_range(o - args_start..o - args_start + len, replacement);
                    inside.push(n);
                }
            }
        }
        let mut args: Vec<String> = entries(&inner, language)
            .into_iter()
            .map(|(_, a)| a.to_string())
            .collect();
        let receiver_arg = if through_call {
            // Left as it was, the call would give the body no object to read fields of.
            anyhow::ensure!(
                !args.is_empty(),
                "`{callee}` is called through `call` with no arguments at {place}; there is no \
                 receiver to keep in front of the object, so nothing was rewritten"
            );
            Some(args.remove(0))
        } else {
            None
        };
        if language == Language::JavaScript
            && let Some(spread) = args.iter().find(|a| a.starts_with("..."))
        {
            anyhow::bail!(
                "`{callee}` is called with `{spread}` at {place}; which parameters a spread \
                 reaches is known only when the call runs, so it is not bundled"
            );
        }
        if matches!(language, Language::JavaScript | Language::TypeScript) {
            // The object carries a default where the call left the parameter out or passed
            // `undefined`, but a value that is `undefined` only at run time would get the
            // default before and not after.
            // A bundled parameter is never a rest one, so the argument at its position is its.
            for p in bundled {
                let (Some(default), Some(value)) = (&declared[*p].default, args.get(*p)) else {
                    continue;
                };
                anyhow::ensure!(
                    js_constant(value),
                    "`{callee}` at {place} passes `{value}` as `{}`, which defaults to \
                     `{default}`: if it is `undefined` when the call runs the default applies, \
                     and in the object it would not. Pass a constant there (`void 0` for the \
                     default), or leave `{}` out of the bundle",
                    declared[*p].name,
                    declared[*p].name
                );
            }
        }
        let Some(bound) = bind_arguments(&args, declared, language) else {
            unmatched.push(place);
            continue;
        };
        if let Some((moved, passed)) =
            reordered_arguments(&args, &bound, bundled, declared, language)
        {
            return Err(reordered(callee, &place, moved, passed, language));
        }
        // A method's qualifier is the object it is called on, not where the type lives, and a
        // TypeScript or JavaScript literal names no type at all.
        let qualifier =
            if is_method || matches!(language, Language::TypeScript | Language::JavaScript) {
                ""
            } else {
                qualifier_before(&source, at)
            };
        let spelling = format!("{qualifier}{name}");
        let mut new_args = rewritten_call(
            &args, &bound, bundled, declared, language, &spelling, binding,
        );
        if let Some(receiver) = receiver_arg {
            new_args = if new_args.is_empty() {
                receiver
            } else {
                format!("{receiver}, {new_args}")
            };
        }
        for n in inside {
            consumed[n] = true;
        }
        if language == Language::Python
            && qualifier.is_empty()
            && path != file
            && !bare_callers.contains(&path)
        {
            bare_callers.push(path.clone());
        }
        edits
            .entry(path)
            .or_default()
            .push((args_start, args_end - args_start, new_args));
        call_sites += 1;
    }

    Ok(PolyglotCallers {
        edits,
        unmatched,
        call_sites,
        consumed,
        bare_callers,
    })
}
