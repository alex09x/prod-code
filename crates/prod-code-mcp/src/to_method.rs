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
    /// Positions the analyzer reported where the file does not name the function: what is there
    /// is unknown, so nothing is written while any remains, forced or not (#446).
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl MadeMethod {
    pub fn render(&self, diff_budget: usize) -> String {
        let sep = if self.file.ends_with(".ts")
            || self.file.ends_with(".tsx")
            || self.file.ends_with(".js")
            || self.file.ends_with(".jsx")
            || self.file.ends_with(".py")
            || self.file.ends_with(".swift")
        {
            "."
        } else if self.file.ends_with(".go") {
            if self.owner.is_empty() { "" } else { "." }
        } else {
            "::"
        };
        let call_target = if self.owner.is_empty() {
            self.method.clone()
        } else {
            format!("{}{sep}{}", self.owner, self.method)
        };
        let receiver_desc = if self.file.ends_with(".ts")
            || self.file.ends_with(".tsx")
            || self.file.ends_with(".js")
            || self.file.ends_with(".jsx")
            || self.file.ends_with(".cpp")
            || self.file.ends_with(".cc")
            || self.file.ends_with(".cxx")
            || self.file.ends_with(".h")
            || self.file.ends_with(".hpp")
        {
            "this"
        } else if self.file.ends_with(".go") {
            &self.receiver
        } else {
            "self"
        };
        let mut out = format!(
            "`{call_target}` ({})\n\n- the parameter `{}` becomes the receiver `{}`; {} use(s) of it in \
             the body are now `{receiver_desc}`\n- {} call site(s) now call it as a method\n\n",
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
        if !self.unmatched.is_empty() {
            out.push_str("\nnot rewritten; nothing is written while any remains, forced or not:\n");
            for u in &self.unmatched {
                out.push_str(&format!("  {u}\n"));
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
    let (bl, bc) = crate::signature::position_at(&text, binding_at)?;
    let mut renamed_uses = 0usize;
    let uses = crate::signature::references(remote, root, file, bl, bc)
        .await
        .with_context(|| {
            format!("cannot find the uses of `{binding}`, which become `self`; nothing was planned")
        })?;
    for (path, l, c) in uses {
        if path != file {
            continue;
        }
        // A use left behind names a parameter that no longer exists (#446).
        let use_at = crate::signature::offset_of(&text, l, c).with_context(|| {
            format!(
                "the analyzer places a use of `{binding}` at {}:{l}:{c}, which is not in the \
                 file; nothing was planned",
                display(root, file)
            )
        })?;
        if !(body_open < use_at && use_at < body_close) {
            continue;
        }
        anyhow::ensure!(
            text[use_at..].starts_with(&binding)
                && !text[use_at + binding.len()..].starts_with(is_ident),
            "the analyzer places a use of `{binding}` at {}:{l}:{c}, but the file says otherwise; \
             nothing was planned",
            display(root, file)
        );
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
        if !body[at..].starts_with(name.as_str()) || body[at + name.len()..].starts_with(is_ident) {
            unmatched.push(format!(
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
        // `force` overrides the analyzer, not a position this could not read (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{name}` could not be read; nothing was written:\n  {}",
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
        unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}

use crate::make_static::Language;

pub fn split_call_arguments(args_str: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth_p = 0i32;
    let mut depth_b = 0i32;
    let mut depth_c = 0i32;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;

    for c in args_str.chars() {
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                in_quote = None;
            }
            current.push(c);
            continue;
        }
        match c {
            '"' | '\'' | '`' => {
                in_quote = Some(c);
                current.push(c);
            }
            '(' => {
                depth_p += 1;
                current.push(c);
            }
            ')' => {
                depth_p -= 1;
                current.push(c);
            }
            '[' => {
                depth_b += 1;
                current.push(c);
            }
            ']' => {
                depth_b -= 1;
                current.push(c);
            }
            '{' => {
                depth_c += 1;
                current.push(c);
            }
            '}' => {
                depth_c -= 1;
                current.push(c);
            }
            ',' if depth_p == 0 && depth_b == 0 && depth_c == 0 => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        args.push(current.trim().to_string());
    }
    args
}

pub fn to_method_ts(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
) -> Result<(String, String, String, String, String, usize, usize)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ") || trimmed.starts_with("export class ") || trimmed.starts_with("export default class ") {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let mut name = "";
            for (w_idx, w) in words.iter().enumerate() {
                if *w == "class" && w_idx + 1 < words.len() {
                    name = words[w_idx + 1].trim_matches('{').trim();
                    break;
                }
            }
            if target_class.is_none() || target_class == Some(name) {
                class_start = Some(idx);
                class_name = name.to_string();
                brace_depth = 0;
            }
        }
        if class_start.is_some() && class_end.is_none() {
            brace_depth += line.chars().filter(|&c| c == '{').count() as i32;
            brace_depth -= line.chars().filter(|&c| c == '}').count() as i32;
            if brace_depth == 0 && line.contains('}') {
                class_end = Some(idx);
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class in TypeScript/JavaScript file")?;
    let c_end = class_end.context("Could not find closing brace of class")?;

    let mut method_line_idx = None;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        if let Some(paren_pos) = trimmed.find('(') {
            let before_paren = trimmed[..paren_pos].trim();
            let words: Vec<&str> = before_paren.split_whitespace().collect();
            if words.last() == Some(&target_method) {
                method_line_idx = Some(idx);
                break;
            }
        }
    }

    let m_idx = method_line_idx.with_context(|| {
        format!("Method `{target_method}` not found in class `{class_name}`")
    })?;

    let m_line = lines[m_idx];
    if !m_line.contains("static ") {
        anyhow::bail!("`{target_method}` is already an instance method");
    }

    let trimmed = m_line.trim_start();
    let indent = &m_line[..m_line.len() - trimmed.len()];
    let open_p = trimmed.find('(').context("Missing parameter list")?;
    let close_p = trimmed.find(')').context("Parameter list does not close on declaration line")?;
    let params_str = &trimmed[open_p + 1..close_p];
    let params = split_call_arguments(params_str);
    let first_param = params.first().context("Method takes no parameters; nothing can become `this`")?.clone();
    let param_name = first_param.split(':').next().unwrap_or(&first_param).trim().to_string();

    let remaining_params = if params.len() > 1 {
        params[1..].join(", ")
    } else {
        String::new()
    };

    let without_static = trimmed.replace("static ", "");
    let new_m_trimmed = if let (Some(op), Some(cp)) = (without_static.find('('), without_static.find(')')) {
        format!("{}{remaining_params}{}", &without_static[..op + 1], &without_static[cp..])
    } else {
        without_static
    };

    let mut m_body_end = m_idx;
    let mut m_depth = 0i32;
    let mut started = false;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(m_idx) {
        let opens = line.chars().filter(|&c| c == '{').count() as i32;
        let closes = line.chars().filter(|&c| c == '}').count() as i32;
        if opens > 0 {
            started = true;
        }
        m_depth += opens;
        m_depth -= closes;
        if started && m_depth == 0 {
            m_body_end = idx;
            break;
        }
    }

    let needle_param_dot = format!("{param_name}.");
    let mut renamed_uses = 0;
    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            new_lines.push(format!("{indent}{new_m_trimmed}"));
        } else if idx > m_idx && idx <= m_body_end {
            let mut cur = line.to_string();
            let mut search_idx = 0;
            while let Some(rel) = cur[search_idx..].find(&needle_param_dot) {
                let pos = search_idx + rel;
                if pos > 0 && is_ident(cur[..pos].chars().next_back().unwrap()) {
                    search_idx = pos + needle_param_dot.len();
                    continue;
                }
                cur.replace_range(pos..pos + needle_param_dot.len(), "this.");
                renamed_uses += 1;
                search_idx = pos + "this.".len();
            }
            new_lines.push(cur);
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let (final_code, rewritten_calls) = rewrite_static_calls_in_code(
        &intermediate,
        target_method,
        &class_name,
        Language::TypeScript,
    );

    Ok((class_name, target_method.to_string(), first_param, "this".to_string(), final_code, renamed_uses, rewritten_calls))
}

pub fn to_method_py(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
) -> Result<(String, String, String, String, String, usize, usize)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut class_indent = 0;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(after_class_raw) = trimmed.strip_prefix("class ") {
            let indent = line.len() - trimmed.len();
            let after_class = after_class_raw.trim_start();
            let name = after_class
                .split(['(', ':'])
                .next()
                .unwrap_or("")
                .trim();
            if target_class.is_none() || target_class == Some(name) {
                class_start = Some(idx);
                class_name = name.to_string();
                class_indent = indent;
            }
        }
        if class_start.is_some() && idx > class_start.unwrap() && class_end.is_none() {
            let indent = line.len() - trimmed.len();
            if !trimmed.is_empty() && !trimmed.starts_with('#') && indent <= class_indent {
                class_end = Some(idx);
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class in Python file")?;
    let c_end = class_end.unwrap_or(lines.len());

    let mut method_line_idx = None;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        let def_needle = format!("def {target_method}(");
        let async_def_needle = format!("async def {target_method}(");
        if trimmed.starts_with(&def_needle) || trimmed.starts_with(&async_def_needle) {
            method_line_idx = Some(idx);
            break;
        }
    }

    let m_idx = method_line_idx.with_context(|| {
        format!("Method `{target_method}` not found in class `{class_name}`")
    })?;

    let m_trimmed = lines[m_idx].trim_start();
    let open_p = m_trimmed.find('(').context("Missing parameter list")?;
    let close_p = m_trimmed.find(')').context("Parameter list does not close")?;
    let params_str = &m_trimmed[open_p + 1..close_p];
    let params = split_call_arguments(params_str);
    let first_param = params.first().context("Method takes no parameters; nothing can become `self`")?.clone();
    if first_param == "self" {
        anyhow::bail!("`{target_method}` already takes `self`");
    }
    let param_name = first_param.split(':').next().unwrap_or(&first_param).trim().to_string();

    let remaining_params = if params.len() > 1 {
        format!("self, {}", params[1..].join(", "))
    } else {
        "self".to_string()
    };

    let before_p = &m_trimmed[..open_p + 1];
    let after_p = &m_trimmed[close_p..];
    let indent_str = &lines[m_idx][..lines[m_idx].len() - m_trimmed.len()];
    let new_m_line = format!("{indent_str}{before_p}{remaining_params}{after_p}");

    let m_line = lines[m_idx];
    let m_indent = m_line.len() - m_line.trim_start().len();
    let mut m_body_end = m_idx;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(m_idx + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent <= m_indent {
            break;
        }
        m_body_end = idx;
    }

    let needle_param_dot = format!("{param_name}.");
    let mut renamed_uses = 0;
    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if m_idx > 0 && idx == m_idx - 1 && line.trim() == "@staticmethod" {
            continue;
        }
        if idx == m_idx {
            new_lines.push(new_m_line.clone());
        } else if idx > m_idx && idx <= m_body_end {
            let mut cur = line.to_string();
            let mut search_idx = 0;
            while let Some(rel) = cur[search_idx..].find(&needle_param_dot) {
                let pos = search_idx + rel;
                if pos > 0 && is_ident(cur[..pos].chars().next_back().unwrap()) {
                    search_idx = pos + needle_param_dot.len();
                    continue;
                }
                cur.replace_range(pos..pos + needle_param_dot.len(), "self.");
                renamed_uses += 1;
                search_idx = pos + "self.".len();
            }
            new_lines.push(cur);
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let (final_code, rewritten_calls) = rewrite_static_calls_in_code(
        &intermediate,
        target_method,
        &class_name,
        Language::Python,
    );

    Ok((class_name, target_method.to_string(), first_param, "self".to_string(), final_code, renamed_uses, rewritten_calls))
}

pub fn to_method_cpp(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
) -> Result<(String, String, String, String, String, usize, usize)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ") || trimmed.starts_with("struct ") {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let mut name = "";
            for (w_idx, w) in words.iter().enumerate() {
                if (*w == "class" || *w == "struct") && w_idx + 1 < words.len() {
                    name = words[w_idx + 1].trim_matches(|c| c == '{' || c == ':').trim();
                    break;
                }
            }
            if target_class.is_none() || target_class == Some(name) {
                class_start = Some(idx);
                class_name = name.to_string();
                brace_depth = 0;
            }
        }
        if class_start.is_some() && class_end.is_none() {
            brace_depth += line.chars().filter(|&c| c == '{').count() as i32;
            brace_depth -= line.chars().filter(|&c| c == '}').count() as i32;
            if brace_depth == 0 && line.contains('}') {
                class_end = Some(idx);
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class/struct in C++ file")?;
    let c_end = class_end.context("Could not find closing brace of C++ class")?;

    let mut method_line_idx = None;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        if let Some(paren_pos) = trimmed.find('(') {
            let before_paren = trimmed[..paren_pos].trim();
            let words: Vec<&str> = before_paren.split_whitespace().collect();
            if let Some(&last_word) = words.last() {
                let clean_name = last_word.trim_start_matches('*').trim_start_matches('&');
                if clean_name == target_method {
                    method_line_idx = Some(idx);
                    break;
                }
            }
        }
    }

    let m_idx = method_line_idx.with_context(|| {
        format!("Method `{target_method}` not found in class `{class_name}`")
    })?;

    let m_line = lines[m_idx];
    if !m_line.contains("static ") {
        anyhow::bail!("`{target_method}` is already an instance method");
    }

    let trimmed = m_line.trim_start();
    let indent = &m_line[..m_line.len() - trimmed.len()];
    let open_p = trimmed.find('(').context("Missing parameter list")?;
    let close_p = trimmed.find(')').context("Parameter list does not close")?;
    let params_str = &trimmed[open_p + 1..close_p];
    let params = split_call_arguments(params_str);
    let first_param = params.first().context("Method takes no parameters; nothing can become receiver")?.clone();
    let is_const = first_param.starts_with("const ");
    let param_words: Vec<&str> = first_param.split_whitespace().collect();
    let param_name = param_words.last().unwrap_or(&"").trim_matches(|c| c == '&' || c == '*');

    let remaining_params = if params.len() > 1 {
        params[1..].join(", ")
    } else {
        String::new()
    };

    let without_static = trimmed.replace("static ", "");
    let const_suffix = if is_const && !without_static.contains(") const") { " const" } else { "" };
    let new_m_trimmed = if let (Some(op), Some(cp)) = (without_static.find('('), without_static.find(')')) {
        let after_cp = &without_static[cp + 1..];
        format!("{}{remaining_params}){const_suffix}{after_cp}", &without_static[..op + 1])
    } else {
        without_static
    };

    let mut m_body_end = m_idx;
    let mut m_depth = 0i32;
    let mut started = false;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(m_idx) {
        let opens = line.chars().filter(|&c| c == '{').count() as i32;
        let closes = line.chars().filter(|&c| c == '}').count() as i32;
        if opens > 0 {
            started = true;
        }
        m_depth += opens;
        m_depth -= closes;
        if started && m_depth == 0 {
            m_body_end = idx;
            break;
        }
    }

    let needle_param_dot = format!("{param_name}.");
    let needle_param_arrow = format!("{param_name}->");
    let mut renamed_uses = 0;
    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            new_lines.push(format!("{indent}{new_m_trimmed}"));
        } else if idx > m_idx && idx <= m_body_end {
            let mut cur = line.to_string();
            let mut search_idx = 0;
            while let Some(rel) = cur[search_idx..].find(&needle_param_dot) {
                let pos = search_idx + rel;
                if pos > 0 && is_ident(cur[..pos].chars().next_back().unwrap()) {
                    search_idx = pos + needle_param_dot.len();
                    continue;
                }
                cur.replace_range(pos..pos + needle_param_dot.len(), "this->");
                renamed_uses += 1;
                search_idx = pos + "this->".len();
            }
            search_idx = 0;
            while let Some(rel) = cur[search_idx..].find(&needle_param_arrow) {
                let pos = search_idx + rel;
                if pos > 0 && is_ident(cur[..pos].chars().next_back().unwrap()) {
                    search_idx = pos + needle_param_arrow.len();
                    continue;
                }
                cur.replace_range(pos..pos + needle_param_arrow.len(), "this->");
                renamed_uses += 1;
                search_idx = pos + "this->".len();
            }
            new_lines.push(cur);
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let (final_code, rewritten_calls) = rewrite_static_calls_in_code(
        &intermediate,
        target_method,
        &class_name,
        Language::Cpp,
    );

    Ok((class_name, target_method.to_string(), first_param, "*this".to_string(), final_code, renamed_uses, rewritten_calls))
}

pub fn to_method_swift(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
) -> Result<(String, String, String, String, String, usize, usize)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ") || trimmed.starts_with("struct ") || trimmed.starts_with("actor ") {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let mut name = "";
            for (w_idx, w) in words.iter().enumerate() {
                if (*w == "class" || *w == "struct" || *w == "actor") && w_idx + 1 < words.len() {
                    name = words[w_idx + 1].trim_matches(|c| c == '{' || c == ':').trim();
                    break;
                }
            }
            if target_class.is_none() || target_class == Some(name) {
                class_start = Some(idx);
                class_name = name.to_string();
                brace_depth = 0;
            }
        }
        if class_start.is_some() && class_end.is_none() {
            brace_depth += line.chars().filter(|&c| c == '{').count() as i32;
            brace_depth -= line.chars().filter(|&c| c == '}').count() as i32;
            if brace_depth == 0 && line.contains('}') {
                class_end = Some(idx);
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class/struct in Swift file")?;
    let c_end = class_end.context("Could not find closing brace in Swift type")?;

    let mut method_line_idx = None;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        let needle = format!("func {target_method}(");
        if trimmed.contains(&needle) {
            method_line_idx = Some(idx);
            break;
        }
    }

    let m_idx = method_line_idx.with_context(|| {
        format!("Method `{target_method}` not found in `{class_name}`")
    })?;

    let m_line = lines[m_idx];
    if !m_line.contains("static func ") && !m_line.contains("class func ") {
        anyhow::bail!("`{target_method}` is already an instance method");
    }

    let trimmed = m_line.trim_start();
    let indent = &m_line[..m_line.len() - trimmed.len()];
    let open_p = trimmed.find('(').context("Missing parameter list")?;
    let close_p = trimmed.find(')').context("Parameter list does not close")?;
    let params_str = &trimmed[open_p + 1..close_p];
    let params = split_call_arguments(params_str);
    let first_param = params.first().context("Method takes no parameters; nothing can become `self`")?.clone();
    let param_name = first_param.split(':').next().unwrap_or(&first_param).trim().to_string();

    let remaining_params = if params.len() > 1 {
        params[1..].join(", ")
    } else {
        String::new()
    };

    let without_static = trimmed.replace("static func ", "func ")
        .replace("class func ", "func ");
    let new_m_trimmed = if let (Some(op), Some(cp)) = (without_static.find('('), without_static.find(')')) {
        format!("{}{remaining_params}{}", &without_static[..op + 1], &without_static[cp..])
    } else {
        without_static
    };

    let mut m_body_end = m_idx;
    let mut m_depth = 0i32;
    let mut started = false;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(m_idx) {
        let opens = line.chars().filter(|&c| c == '{').count() as i32;
        let closes = line.chars().filter(|&c| c == '}').count() as i32;
        if opens > 0 {
            started = true;
        }
        m_depth += opens;
        m_depth -= closes;
        if started && m_depth == 0 {
            m_body_end = idx;
            break;
        }
    }

    let needle_param_dot = format!("{param_name}.");
    let mut renamed_uses = 0;
    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            new_lines.push(format!("{indent}{new_m_trimmed}"));
        } else if idx > m_idx && idx <= m_body_end {
            let mut cur = line.to_string();
            let mut search_idx = 0;
            while let Some(rel) = cur[search_idx..].find(&needle_param_dot) {
                let pos = search_idx + rel;
                if pos > 0 && is_ident(cur[..pos].chars().next_back().unwrap()) {
                    search_idx = pos + needle_param_dot.len();
                    continue;
                }
                cur.replace_range(pos..pos + needle_param_dot.len(), "self.");
                renamed_uses += 1;
                search_idx = pos + "self.".len();
            }
            new_lines.push(cur);
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let (final_code, rewritten_calls) = rewrite_static_calls_in_code(
        &intermediate,
        target_method,
        &class_name,
        Language::Swift,
    );

    Ok((class_name, target_method.to_string(), first_param, "self".to_string(), final_code, renamed_uses, rewritten_calls))
}

pub fn to_method_go(
    code: &str,
    target_struct: Option<&str>,
    target_func: &str,
) -> Result<(String, String, String, String, String, usize, usize)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut func_line_idx = None;
    let needle_func = format!("func {target_func}(");

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&needle_func) {
            func_line_idx = Some(idx);
            break;
        }
    }

    let f_idx = func_line_idx.with_context(|| {
        format!("Function `{target_func}` not found in Go file")
    })?;

    let f_line = lines[f_idx];
    let trimmed = f_line.trim_start();
    let indent = &f_line[..f_line.len() - trimmed.len()];
    let open_p = trimmed.find('(').context("Missing parameter list")?;
    let close_p = trimmed.find(')').context("Parameter list does not close")?;
    let params_str = &trimmed[open_p + 1..close_p];
    let params = split_call_arguments(params_str);
    let first_param = params.first().context("Function takes no parameters; nothing can become receiver")?.clone();

    let param_words: Vec<&str> = first_param.split_whitespace().collect();
    if param_words.len() < 2 {
        anyhow::bail!("First parameter `{first_param}` has no type");
    }
    let s_type = param_words[1];
    let s_name = s_type.trim_start_matches('*');
    if let Some(target) = target_struct
        && target != s_name {
            anyhow::bail!("First parameter type `{s_name}` does not match target struct `{target}`");
        }

    let remaining_params = if params.len() > 1 {
        params[1..].join(", ")
    } else {
        String::new()
    };

    let after_cp = &trimmed[close_p + 1..];
    let new_f_line = format!("{indent}func ({first_param}) {target_func}({remaining_params}){after_cp}");

    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            new_lines.push(new_f_line.clone());
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let (final_code, rewritten_calls) = rewrite_static_calls_in_code(
        &intermediate,
        target_func,
        s_name,
        Language::Go,
    );

    Ok((s_name.to_string(), target_func.to_string(), first_param.clone(), format!("({first_param})"), final_code, 0, rewritten_calls))
}

pub fn rewrite_static_calls_in_code(
    code: &str,
    target_method: &str,
    owner_class: &str,
    lang: Language,
) -> (String, usize) {
    let mut out = String::new();
    let mut rewritten = 0;
    let target_prefix = match lang {
        Language::TypeScript | Language::Python | Language::Swift => {
            format!("{owner_class}.{target_method}(")
        }
        Language::Cpp => {
            format!("{owner_class}::{target_method}(")
        }
        Language::Go => {
            format!("{target_method}(")
        }
    };

    for line in code.lines() {
        let trimmed = line.trim_start();
        let comment_prefix = match lang {
            Language::Python => "#",
            _ => "//",
        };
        if trimmed.starts_with(comment_prefix) || trimmed.starts_with('*') {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if !line.contains(&target_prefix) {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let mut current_line = line.to_string();
        let mut search_start = 0;
        while let Some(rel_pos) = current_line[search_start..].find(&target_prefix) {
            let pos = search_start + rel_pos;
            if pos > 0 {
                let prev_char = current_line[..pos].chars().next_back().unwrap();
                if is_ident(prev_char) || (lang == Language::Go && prev_char == '.') {
                    search_start = pos + target_prefix.len();
                    continue;
                }
            }
            if lang == Language::Go
                && current_line[..pos].trim_start().starts_with("func ")
                && !current_line[..pos].contains('{')
            {
                search_start = pos + target_prefix.len();
                continue;
            }

            let after_open = pos + target_prefix.len();
            let rest = &current_line[after_open..];
            let mut depth = 1i32;
            let mut close_pos = None;
            for (i, c) in rest.char_indices() {
                if c == '(' {
                    depth += 1;
                } else if c == ')' {
                    depth -= 1;
                    if depth == 0 {
                        close_pos = Some(after_open + i);
                        break;
                    }
                }
            }

            let Some(cp) = close_pos else {
                search_start = pos + target_prefix.len();
                continue;
            };

            let args_str = &current_line[after_open..cp];
            let args = split_call_arguments(args_str);
            if args.is_empty() {
                search_start = pos + target_prefix.len();
                continue;
            }

            let raw_recv = args[0].trim();
            let recv = raw_recv
                .split_once(':')
                .or_else(|| raw_recv.split_once('='))
                .map(|(_, val)| val.trim())
                .unwrap_or(raw_recv);
            let rest_args = if args.len() > 1 {
                args[1..].join(", ")
            } else {
                String::new()
            };

            let op = if lang == Language::Cpp && (recv.starts_with('*') || recv.ends_with("->")) {
                "->"
            } else {
                "."
            };

            let formatted_recv = if recv.contains(' ') && !recv.starts_with('(') {
                format!("({recv})")
            } else {
                recv.to_string()
            };

            let call_replacement = format!("{formatted_recv}{op}{target_method}({rest_args})");
            current_line.replace_range(pos..=cp, &call_replacement);
            rewritten += 1;
            search_start = pos + call_replacement.len();
        }

        out.push_str(&current_line);
        out.push('\n');
    }

    if !code.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    (out, rewritten)
}

#[allow(clippy::too_many_arguments)]
pub async fn convert_to_method_polyglot(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    class_name: Option<&str>,
    method_name: &str,
    apply: bool,
    force: bool,
) -> Result<MadeMethod> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Cannot read {}", file_path.display()))?;
    let lang = Language::from_path(file_path)
        .with_context(|| format!("Unsupported language for file: {}", file_path.display()))?;

    let (owner, method, parameter, receiver, new_content, renamed_uses, file_rewritten) = match lang {
        Language::TypeScript => to_method_ts(&content, class_name, method_name)?,
        Language::Python => to_method_py(&content, class_name, method_name)?,
        Language::Cpp => to_method_cpp(&content, class_name, method_name)?,
        Language::Swift => to_method_swift(&content, class_name, method_name)?,
        Language::Go => to_method_go(&content, class_name, method_name)?,
    };

    let mut rewritten = vec![(file_path.to_string_lossy().to_string(), new_content)];
    let mut total_rewritten_calls = file_rewritten;

    for entry in ignore::WalkBuilder::new(workspace_root).build().flatten() {
        let path = entry.path();
        if path.is_file() && path != file_path && lang.matches_extension(path)
            && let Ok(other_content) = std::fs::read_to_string(path)
                && other_content.contains(method_name) {
                    let (new_other, calls) = rewrite_static_calls_in_code(
                        &other_content,
                        method_name,
                        &owner,
                        lang,
                    );
                    if new_other != other_content {
                        rewritten.push((path.to_string_lossy().to_string(), new_other));
                        total_rewritten_calls += calls;
                    }
                }
    }

    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, workspace_root, &to_check, &[]).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.source
                    .as_deref()
                    .map(|s| format!("[{s}] "))
                    .unwrap_or_default(),
                d.message,
                d.line,
                d.col
            )
        })
        .collect();

    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let rewritten_map: BTreeMap<PathBuf, String> = rewritten
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.clone()))
            .collect();
        let edit = crate::signature::whole_file_edit(&rewritten_map);
        crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
    }

    let rel_file = display(workspace_root, file_path);
    Ok(MadeMethod {
        owner,
        method,
        root: workspace_root.to_path_buf(),
        file: rel_file,
        parameter,
        receiver,
        renamed_uses,
        rewritten_calls: total_rewritten_calls,
        unchanged: vec![],
        unmatched: vec![],
        rewritten,
        diagnostics,
        applied: apply,
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
            unmatched: vec![],
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

    #[test]
    fn to_method_ts_transforms_declaration_and_calls() {
        let ts_code = r#"class Calculator {
    static add(c: Calculator, x: number): number {
        return c.val + x;
    }
}

function test() {
    const calc = new Calculator();
    const res = Calculator.add(calc, 5);
}
"#;
        let (owner, method, param, recv, transformed, renamed, calls) =
            to_method_ts(ts_code, Some("Calculator"), "add").unwrap();
        assert_eq!(owner, "Calculator");
        assert_eq!(method, "add");
        assert_eq!(param, "c: Calculator");
        assert_eq!(recv, "this");
        assert_eq!(renamed, 1);
        assert_eq!(calls, 1);
        assert!(transformed.contains("add(x: number): number {"));
        assert!(transformed.contains("return this.val + x;"));
        assert!(transformed.contains("calc.add(5)"));
    }

    #[test]
    fn to_method_py_transforms_declaration_and_calls() {
        let py_code = r#"class MathUtil:
    @staticmethod
    def multiply(u: MathUtil, y: int) -> int:
        return u.factor * y

def run():
    util = MathUtil()
    result = MathUtil.multiply(util, 6)
"#;
        let (owner, method, param, recv, transformed, renamed, calls) =
            to_method_py(py_code, Some("MathUtil"), "multiply").unwrap();
        assert_eq!(owner, "MathUtil");
        assert_eq!(method, "multiply");
        assert_eq!(param, "u: MathUtil");
        assert_eq!(recv, "self");
        assert_eq!(renamed, 1);
        assert_eq!(calls, 1);
        assert!(!transformed.contains("@staticmethod"));
        assert!(transformed.contains("def multiply(self, y: int) -> int:"));
        assert!(transformed.contains("return self.factor * y"));
        assert!(transformed.contains("util.multiply(6)"));
    }

    #[test]
    fn to_method_cpp_transforms_declaration_and_calls() {
        let cpp_code = r#"class Counter {
public:
    static int increment(Counter& c, int step) {
        return c.val + step;
    }
};

void run() {
    Counter cnt;
    int res = Counter::increment(cnt, 2);
}
"#;
        let (owner, method, param, recv, transformed, renamed, calls) =
            to_method_cpp(cpp_code, Some("Counter"), "increment").unwrap();
        assert_eq!(owner, "Counter");
        assert_eq!(method, "increment");
        assert_eq!(param, "Counter& c");
        assert_eq!(recv, "*this");
        assert_eq!(renamed, 1);
        assert_eq!(calls, 1);
        assert!(transformed.contains("int increment(int step) {"));
        assert!(transformed.contains("return this->val + step;"));
        assert!(transformed.contains("cnt.increment(2)"));
    }

    #[test]
    fn to_method_swift_transforms_declaration_and_calls() {
        let swift_code = r#"class Greeter {
    static func greet(g: Greeter, name: String) -> String {
        return g.prefix + name
    }
}

func test() {
    let grt = Greeter()
    let msg = Greeter.greet(grt, name: "Alice")
}
"#;
        let (owner, method, param, recv, transformed, renamed, calls) =
            to_method_swift(swift_code, Some("Greeter"), "greet").unwrap();
        assert_eq!(owner, "Greeter");
        assert_eq!(method, "greet");
        assert_eq!(param, "g: Greeter");
        assert_eq!(recv, "self");
        assert_eq!(renamed, 1);
        assert_eq!(calls, 1);
        assert!(transformed.contains("func greet(name: String) -> String {"));
        assert!(transformed.contains("return self.prefix + name"));
        assert!(transformed.contains("grt.greet(name: \"Alice\")"));
    }

    #[test]
    fn to_method_go_transforms_declaration_and_calls() {
        let go_code = r#"package main

type Service struct {
    tag string
}

func Process(s *Service, data string) string {
    return s.tag + ":" + data
}

func main() {
    svc := &Service{tag: "svc"}
    res := Process(svc, "test")
}
"#;
        let (owner, method, param, recv, transformed, renamed, calls) =
            to_method_go(go_code, Some("Service"), "Process").unwrap();
        assert_eq!(owner, "Service");
        assert_eq!(method, "Process");
        assert_eq!(param, "s *Service");
        assert_eq!(recv, "(s *Service)");
        assert_eq!(renamed, 0);
        assert_eq!(calls, 1);
        assert!(transformed.contains("func (s *Service) Process(data string) string {"));
        assert!(transformed.contains("svc.Process(\"test\")"));
    }
}
