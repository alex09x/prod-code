//! Turning an associated function into a method: the other direction of `make_static`.
//!
//! `fn bump(c: &mut Counter, by: u32)` in `impl Counter` becomes `fn bump(&mut self, by: u32)`, the
//! parameter's uses in the body become `self`, and `Counter::bump(&mut c, 2)` becomes `c.bump(2)`.
//! Nothing a caller evaluates is dropped or reordered: the first argument becomes the receiver, and
//! a receiver is evaluated first, as the first argument was. A use of the function as a value,
//! `Counter::bump`, is left alone — a method is still reachable by its path — and so is a call
//! inside the function itself, which stays valid as `Counter::bump(self, …)`.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MadeMethod {
    pub owner: String,
    pub method: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    /// The first parameter as it was declared: `c: &mut Counter`.
    pub parameter: String,
    /// The receiver it became: `&mut self`.
    pub receiver: String,
    /// Uses of the parameter in the body, now `self`.
    pub renamed_uses: usize,
    pub rewritten_calls: usize,
    /// References left as they are, and why: still valid, but not rewritten.
    pub unchanged: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl MadeMethod {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}::{}` ({})\n\n- the parameter `{}` becomes the receiver `{}`; {} use(s) of it in \
             the body are now `self`\n- {} call site(s) now call it as a method\n\n",
            self.owner,
            self.method,
            self.file,
            self.parameter,
            self.receiver,
            self.renamed_uses,
            self.rewritten_calls
        );
        let mut body = String::new();
        let mut changed_lines = 0usize;
        for (path, new_text) in &self.rewritten {
            let old_text = crate::refactor::text_before_apply(Path::new(path));
            let rel = display(&self.root, Path::new(path));
            let diff = similar::TextDiff::from_lines(&old_text, new_text);
            changed_lines += diff
                .iter_all_changes()
                .filter(|c| c.tag() != similar::ChangeTag::Equal)
                .count();
            body.push_str(
                &diff
                    .unified_diff()
                    .context_radius(2)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        out.push_str(&format!(
            "{} changed line(s) in {} file(s)\n\n",
            changed_lines,
            self.rewritten.len()
        ));
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }
        if !self.unchanged.is_empty() {
            out.push_str(&format!(
                "\nleft as they are, and still valid ({}):\n",
                self.unchanged.len()
            ));
            for r in &self.unchanged {
                out.push_str(&format!("  {r}\n"));
            }
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        if self.applied {
            out.push_str(&format!(
                "\n[applied to {} file(s)]\n",
                self.rewritten.len()
            ));
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make these edits\n");
        }
        out
    }
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The base name of a type: `Wrapper` for `Wrapper<T>` and for `crate::m::Wrapper`.
fn base_name(ty: &str) -> &str {
    let ty = ty.trim();
    let ty = ty.split('<').next().unwrap_or(ty);
    ty.rsplit("::").next().unwrap_or(ty).trim()
}

/// The receiver a first parameter becomes, when its type is the `impl`'s own type: `c: &mut
/// Counter` → `&mut self`, `c: &'a Self` → `&'a self`, `mut c: Counter` → `mut self`. The binding's
/// name comes back with it.
pub fn receiver_for(param: &str, owner: &str) -> Option<(String, String)> {
    let (pattern, ty) = param.split_once(':')?;
    let pattern = pattern.trim();
    let (binding_mut, name) = match pattern.strip_prefix("mut ") {
        Some(rest) => (true, rest.trim()),
        None => (false, pattern),
    };
    if name.is_empty() || !name.chars().all(is_ident) || name == "self" {
        return None;
    }
    let ty = ty.trim();
    let (reference, rest) = match ty.strip_prefix('&') {
        None => (String::new(), ty),
        Some(rest) => {
            let rest = rest.trim_start();
            let (lifetime, rest) = if rest.starts_with('\'') {
                let end = rest.find(char::is_whitespace)?;
                (format!("{} ", &rest[..end]), rest[end..].trim_start())
            } else {
                (String::new(), rest)
            };
            match rest.strip_prefix("mut ") {
                Some(after) => (format!("&{lifetime}mut "), after.trim_start()),
                None => (format!("&{lifetime}"), rest),
            }
        }
    };
    let is_own = rest == "Self" || base_name(rest) == base_name(owner);
    if !is_own {
        return None;
    }
    let receiver = if reference.is_empty() && binding_mut {
        "mut self".to_string()
    } else {
        format!("{reference}self")
    };
    Some((receiver, name.to_string()))
}

/// The receiver a first argument becomes: `&mut c` and `&c` lose the borrow, which method-call
/// syntax takes by itself, and anything but a path or a call chain is parenthesized.
pub fn receiver_of(argument: &str) -> String {
    let a = argument.trim();
    let a = a
        .strip_prefix("&mut ")
        .or_else(|| a.strip_prefix('&'))
        .map(str::trim_start)
        .unwrap_or(a);
    let mut depth = 0i32;
    let mut simple = !a.is_empty() && a.chars().next().is_some_and(is_ident);
    for c in a.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if depth > 0 => {}
            c if is_ident(c) || c == '.' || c == ':' => {}
            _ => simple = false,
        }
    }
    if simple {
        a.to_string()
    } else {
        format!("({a})")
    }
}

/// Where the path in front of a call's name begins: `Counter::` or `crate::m::Counter::` or `Self::`.
fn path_start(text: &str, name_at: usize) -> usize {
    let mut at = name_at;
    loop {
        let before = &text[..at];
        let Some(stripped) = before.strip_suffix("::") else {
            return at;
        };
        let segment = stripped
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_ident(*c))
            .last()
            .map_or(stripped.len(), |(i, _)| i);
        if segment == stripped.len() {
            return at;
        }
        at = segment;
    }
}

/// Makes the associated function declared at `line`:`col` of `file` a method of its type.
pub async fn convert_to_method(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    apply: bool,
    force: bool,
) -> Result<MadeMethod> {
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
    anyhow::ensure!(
        text[..start].trim_end().ends_with("fn"),
        "the position is not the name of a function declaration"
    );
    let (name, open, close) =
        crate::signature::param_span(&text, start).context("the function has no parameter list")?;
    anyhow::ensure!(
        crate::make_static::split_receiver(&text[open..close]).is_none(),
        "`{name}` already takes `self`"
    );
    let (owner, impl_at, impl_open, _) = crate::extract_field::impl_blocks(&text)
        .into_iter()
        .filter(|(_, _, o, c)| *o < start && start < *c)
        .min_by_key(|(_, _, o, c)| c - o)
        .with_context(|| {
            format!(
                "`{name}` is not inside an `impl` block; a free function has to be moved into one \
                 before it can become a method"
            )
        })?;
    anyhow::ensure!(
        !text[impl_at..impl_open].contains(" for "),
        "`{name}` implements a trait function, and the trait decides whether it takes `self`; \
         change the trait instead"
    );
    let params = crate::signature::split_params(&text[open..close]);
    let first = params
        .first()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .with_context(|| format!("`{name}` takes no parameters, so nothing can become `self`"))?;
    let (receiver, binding) = receiver_for(&first, &owner).with_context(|| {
        format!(
            "the first parameter of `{name}`, `{first}`, is not `{owner}`, `&{owner}` or `&mut \
             {owner}`, so it cannot become the receiver"
        )
    })?;
    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{name}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the function's body does not close")?;

    // The declaration: the first parameter becomes the receiver.
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let rest: Vec<String> = params[1..].iter().map(|p| p.trim().to_string()).collect();
    let new_params = std::iter::once(receiver.clone())
        .chain(rest)
        .collect::<Vec<_>>()
        .join(", ");
    edits
        .entry(file.to_path_buf())
        .or_default()
        .push((open, close - open, new_params));

    // The body: every use of the parameter, as the analyzer resolves it, becomes `self`.
    let first_at = open + text[open..close].find(first.as_str()).unwrap_or(0);
    let binding_at = first_at
        + first
            .find(binding.as_str())
            .context("the parameter's name is not in its declaration")?;
    let (bl, bc) = crate::signature::line_col_at(&text, binding_at);
    let mut renamed_uses = 0usize;
    for (path, l, c) in crate::signature::references(remote, root, file, bl, bc)
        .await
        .unwrap_or_default()
    {
        if path != file {
            continue;
        }
        let Some(use_at) = crate::signature::offset_of(&text, l, c) else {
            continue;
        };
        if !(body_open < use_at && use_at < body_close) || !text[use_at..].starts_with(&binding) {
            continue;
        }
        let after = &text[use_at + binding.len()..];
        if after.chars().next().is_some_and(is_ident) {
            continue;
        }
        edits.entry(file.to_path_buf()).or_default().push((
            use_at,
            binding.len(),
            "self".to_string(),
        ));
        renamed_uses += 1;
    }

    // The calls: `Owner::name(first, rest)` → `first.name(rest)`.
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let mut rewritten_calls = 0usize;
    let mut unchanged = Vec::new();
    let (nl, nc) = crate::signature::line_col_at(&text, start);
    for (path, l, c) in crate::signature::references(remote, root, file, nl, nc)
        .await
        .unwrap_or_default()
    {
        let body = texts
            .entry(path.clone())
            .or_insert_with(|| std::fs::read_to_string(&path).unwrap_or_default())
            .clone();
        let site = format!("{}:{l}:{c}", display(root, &path));
        let Some(at) = crate::signature::offset_of(&body, l, c) else {
            unchanged.push(format!("{site} (the position is not in the file)"));
            continue;
        };
        if !body[at..].starts_with(name.as_str()) {
            unchanged.push(format!(
                "{site} (the analyzer places `{name}` here, but the file says otherwise)"
            ));
            continue;
        }
        if path == file && body_open < at && at < body_close {
            unchanged.push(format!(
                "{site} (a call inside `{name}` itself: `{owner}::{name}(self, …)` stays valid)"
            ));
            continue;
        }
        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(&body, at + name.len())
        else {
            unchanged.push(format!(
                "{site} (the function used as a value: a method is still reachable by its path)"
            ));
            continue;
        };
        let from = path_start(&body, at);
        if from == at {
            unchanged.push(format!("{site} (a call without a path in front)"));
            continue;
        }
        let args = crate::parameter_object::split_args(&body[args_start..args_end]);
        let Some(first_arg) = args.first().filter(|a| !a.trim().is_empty()) else {
            unchanged.push(format!("{site} (a call with no first argument)"));
            continue;
        };
        let remaining: Vec<&str> = args[1..].iter().map(|a| a.trim()).collect();
        edits.entry(path.clone()).or_default().push((
            from,
            args_end + 1 - from,
            format!(
                "{}.{name}({})",
                receiver_of(first_arg),
                remaining.join(", ")
            ),
        ));
        rewritten_calls += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts
            .get(&path)
            .cloned()
            .unwrap_or_else(|| std::fs::read_to_string(&path).unwrap_or_default());
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

    Ok(MadeMethod {
        owner,
        method: name,
        root: root.to_path_buf(),
        file: display(root, file),
        parameter: first,
        receiver,
        renamed_uses,
        rewritten_calls,
        unchanged,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_receiver_follows_the_parameters_type() {
        let r = |p: &str| receiver_for(p, "Counter");
        assert_eq!(r("c: &mut Counter"), Some(("&mut self".into(), "c".into())));
        assert_eq!(r("c: &Counter"), Some(("&self".into(), "c".into())));
        assert_eq!(r("c: Counter"), Some(("self".into(), "c".into())));
        assert_eq!(r("mut c: Counter"), Some(("mut self".into(), "c".into())));
        assert_eq!(r("c: &'a Self"), Some(("&'a self".into(), "c".into())));
        assert_eq!(
            receiver_for("w: &Wrapper<T>", "Wrapper<T>"),
            Some(("&self".into(), "w".into()))
        );
        assert_eq!(r("c: &Other"), None);
        assert_eq!(r("(a, b): (Counter, u32)"), None);
    }

    #[test]
    fn the_first_argument_loses_its_borrow_and_keeps_its_shape() {
        assert_eq!(receiver_of("&mut c"), "c");
        assert_eq!(receiver_of("&c"), "c");
        assert_eq!(receiver_of("self.inner"), "self.inner");
        assert_eq!(receiver_of("make()"), "make()");
        assert_eq!(receiver_of("&items[0]"), "items[0]");
        assert_eq!(receiver_of("*boxed"), "(*boxed)");
        assert_eq!(receiver_of("a + b"), "(a + b)");
    }

    #[test]
    fn the_path_in_front_of_a_call_is_found_whole() {
        let t = "x = crate::m::Counter::bump(&mut c, 1);";
        let at = t.find("bump").unwrap();
        assert_eq!(&t[path_start(t, at)..at], "crate::m::Counter::");
        let t = "Self::peek(c)";
        assert_eq!(path_start(t, 6), 0);
        let t = "peek(c)";
        assert_eq!(path_start(t, 0), 0);
    }

    #[test]
    fn the_report_says_what_became_the_receiver_and_what_stayed() {
        let done = MadeMethod {
            owner: "Counter".into(),
            method: "bump".into(),
            root: "/nonexistent".into(),
            file: "src/lib.rs".into(),
            parameter: "c: &mut Counter".into(),
            receiver: "&mut self".into(),
            renamed_uses: 2,
            rewritten_calls: 1,
            unchanged: vec!["src/lib.rs:9:5 (the function used as a value)".into()],
            rewritten: vec![],
            diagnostics: vec![],
            applied: false,
        };
        let text = done.render(1000);
        assert!(
            text.contains("`c: &mut Counter` becomes the receiver `&mut self`"),
            "{text}"
        );
        assert!(text.contains("2 use(s) of it in the body"), "{text}");
        assert!(
            text.contains("left as they are, and still valid (1)"),
            "{text}"
        );
        assert!(text.contains("nothing was written"), "{text}");
    }
}
