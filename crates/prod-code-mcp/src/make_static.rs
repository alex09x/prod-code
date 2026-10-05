//! Turning a method that never uses `self` into an associated function, with every call site.
//!
//! The declaration loses its receiver; `value.method(args)` becomes `Type::method(args)` and
//! `Type::method(value, args)` loses its first argument. What cannot be done silently is dropping
//! a receiver that does something when it is evaluated — `load()?.method()` runs `load` — so such a
//! call site is reported, and nothing is written while one remains.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MadeStatic {
    pub owner: String,
    pub method: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    /// The receiver the declaration had: `&self`, `&mut self`, `self`.
    pub receiver: String,
    pub rewritten_calls: usize,
    /// Call sites that cannot be safely rewritten, due to receiver effects or unknown type.
    pub blocked: Vec<String>,
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl MadeStatic {
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
        let mut out = format!(
            "`{call_target}` ({})\n\n- the receiver `{}` is removed: it was never used\n- {} call site(s) \
             now call `{call_target}`\n\n",
            self.file, self.receiver, self.rewritten_calls,
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
        if !self.blocked.is_empty() {
            out.push_str(&format!(
                "\n{} call site(s) would drop a receiver that does something when it is evaluated; \
                 bind it to a variable first, or keep the method:\n",
                self.blocked.len()
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) this could not read as a call):\n",
                self.unmatched.len()
            ));
            for r in &self.unmatched {
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

/// Whether `word` occurs in `text` as a whole identifier.
fn mentions(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(is_ident)
            && !text[at + word.len()..].chars().next().is_some_and(is_ident)
    })
}

/// The receiver a parameter list starts with, and the list without it, or `None` when the first
/// parameter is not a receiver.
pub fn split_receiver(params: &str) -> Option<(String, String)> {
    let parts = crate::signature::split_params(params);
    let first = parts.first()?.trim().to_string();
    // `self`, `mut self`, `&self`, `&mut self`, `&'a self`, `self: Box<Self>`.
    let head = first.split(':').next().unwrap_or("").trim();
    let mut word = head.trim_start_matches('&').trim_start();
    if word.starts_with('\'') {
        word = word
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest)
            .trim_start();
    }
    let word = word.strip_prefix("mut ").unwrap_or(word).trim();
    if word != "self" {
        return None;
    }
    let rest: Vec<String> = parts[1..].iter().map(|p| p.trim().to_string()).collect();
    Some((first, rest.join(", ")))
}

/// Whether evaluating `receiver` can do anything: a call, `?`, `.await` or a macro. A path, a
/// field access, `self` or a literal can be dropped without changing what the program does.
pub fn receiver_has_effects(receiver: &str) -> bool {
    let r = receiver.trim();
    r.contains('(') || r.contains('?') || r.contains('!') || r.contains(".await") || r.contains('[')
}

/// Makes the method declared at `line`:`col` of `file` an associated function.
pub async fn make_static(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    apply: bool,
    force: bool,
) -> Result<MadeStatic> {
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
        "the position is not the name of a method declaration"
    );
    let (name, open, close) =
        crate::signature::param_span(&text, start).context("the method has no parameter list")?;
    let (receiver, rest) = split_receiver(&text[open..close]).with_context(|| {
        format!("`{name}` takes no `self`: it is already an associated function")
    })?;
    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{name}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the method's body does not close")?;
    anyhow::ensure!(
        !mentions(&text[body_open..body_close], "self"),
        "`{name}` uses `self`; only a method that never does can lose its receiver"
    );
    let (owner, impl_at, impl_open, _) = crate::extract_field::impl_blocks(&text)
        .into_iter()
        .filter(|(_, _, o, c)| *o < start && start < *c)
        .min_by_key(|(_, _, o, c)| c - o)
        .context("the method is not inside an `impl` block")?;
    // A trait decides whether its methods take a receiver; an implementation cannot drop it.
    anyhow::ensure!(
        !text[impl_at..impl_open].contains(" for "),
        "`{name}` implements a trait method, and the trait decides whether it takes `self`; \
         change the trait instead"
    );

    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    edits
        .entry(file.to_path_buf())
        .or_default()
        .push((open, close - open, rest));
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let mut rewritten_calls = 0usize;
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
        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(&body, at + name.len())
        else {
            unmatched.push(format!("{site} (not a call: the method used as a value)"));
            continue;
        };
        let before = body[..at].trim_end();
        if let Some(dot) = before.strip_suffix('.').map(|b| b.len()) {
            // `receiver.method(args)` → `Owner::method(args)`.
            let recv_start = crate::encapsulate_field::chain_start(&body, dot);
            let recv = body[recv_start..dot].to_string();
            if receiver_has_effects(&recv) {
                blocked.push(format!(
                    "{site} `{}` is evaluated for what it does",
                    recv.trim()
                ));
                // `force` says dropping it is intended: the call is still rewritten, or it
                // would call as a method what no longer takes `self` (#209).
                if !force {
                    continue;
                }
            }
            edits.entry(path.clone()).or_default().push((
                recv_start,
                at + name.len() - recv_start,
                format!("{owner}::{name}"),
            ));
        } else if before.ends_with("::") {
            // `Owner::method(receiver, args)` → `Owner::method(args)`.
            let args = crate::parameter_object::split_args(&body[args_start..args_end]);
            let Some(first) = args.first() else {
                unmatched.push(format!("{site} (a call with no receiver argument)"));
                continue;
            };
            if receiver_has_effects(first.trim_start_matches('&').trim_start_matches("mut ")) {
                blocked.push(format!(
                    "{site} `{}` is evaluated for what it does",
                    first.trim()
                ));
                if !force {
                    continue;
                }
            }
            let remaining: Vec<&str> = args[1..].iter().map(|a| a.trim()).collect();
            edits.entry(path.clone()).or_default().push((
                args_start,
                args_end - args_start,
                remaining.join(", "),
            ));
        } else {
            unmatched.push(format!("{site} (neither a method call nor a path call)"));
            continue;
        }
        rewritten_calls += 1;
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
            "{} call site(s) would drop a receiver that does something; nothing was written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
        );
        // `force` drops a receiver on purpose; it does not write past a reference this did not
        // rewrite (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{name}` were not rewritten, and would still pass a receiver; \
             nothing was written:\n  {}",
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

    Ok(MadeStatic {
        owner,
        method: name,
        root: root.to_path_buf(),
        file: display(root, file),
        receiver,
        rewritten_calls,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    TypeScript,
    Python,
    Cpp,
    Swift,
    Go,
}

impl Language {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension().and_then(|s| s.to_str()) {
            Some("ts" | "tsx" | "js" | "jsx") => Some(Self::TypeScript),
            Some("py") => Some(Self::Python),
            Some("cpp" | "cc" | "cxx" | "h" | "hpp") => Some(Self::Cpp),
            Some("swift") => Some(Self::Swift),
            Some("go") => Some(Self::Go),
            _ => None,
        }
    }

    pub fn matches_extension(&self, path: &Path) -> bool {
        Self::from_path(path) == Some(*self)
    }
}

fn method_declaration_position(
    code: &str,
    lang: Language,
    owner: &str,
    method: &str,
) -> Result<(u32, u32)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut type_start = None;
    let mut type_end = lines.len();
    if lang != Language::Go {
        let type_prefixes: &[&str] = match lang {
            Language::TypeScript => &["class ", "export class ", "export default class "],
            Language::Python => &["class "],
            Language::Cpp => &["class ", "struct "],
            Language::Swift => &["class ", "struct ", "actor "],
            Language::Go => &[],
        };
        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            let type_keyword = match lang {
                Language::TypeScript => "class",
                Language::Python => "class",
                Language::Cpp => {
                    if trimmed.starts_with("struct ") {
                        "struct"
                    } else {
                        "class"
                    }
                }
                Language::Swift => {
                    if trimmed.starts_with("actor ") {
                        "actor"
                    } else if trimmed.starts_with("struct ") {
                        "struct"
                    } else {
                        "class"
                    }
                }
                Language::Go => unreachable!(),
            };
            let declared_type = type_prefixes
                .iter()
                .any(|prefix| trimmed.starts_with(prefix))
                .then(|| {
                    let mut words = trimmed.split_whitespace();
                    words
                        .position(|word| word == type_keyword)
                        .and_then(|_| words.next())
                        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_'))
                })
                .flatten();
            if declared_type == Some(owner) {
                type_start = Some(idx);
                if lang == Language::Python {
                    let indent = line.len() - trimmed.len();
                    type_end = lines
                        .iter()
                        .enumerate()
                        .skip(idx + 1)
                        .find(|(_, nested)| {
                            !nested.trim().is_empty()
                                && !nested.trim_start().starts_with('#')
                                && nested.len() - nested.trim_start().len() <= indent
                        })
                        .map_or(lines.len(), |(end, _)| end);
                } else {
                    let mut depth = 0i32;
                    for (end, nested) in lines.iter().enumerate().skip(idx) {
                        depth += nested.chars().filter(|c| *c == '{').count() as i32;
                        depth -= nested.chars().filter(|c| *c == '}').count() as i32;
                        if end > idx && depth == 0 {
                            type_end = end;
                            break;
                        }
                    }
                }
                break;
            }
        }
    }
    let start = type_start.unwrap_or(0);
    for (idx, line) in lines.iter().enumerate().take(type_end).skip(start) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') {
            continue;
        }
        let name_at = match lang {
            Language::TypeScript => {
                let before = trimmed.split_once('(').map(|(before, _)| before.trim());
                (before.and_then(|before| before.split_whitespace().last()) == Some(method))
                    .then(|| line.find(method))
                    .flatten()
            }
            Language::Python => (trimmed.starts_with(&format!("def {method}("))
                || trimmed.starts_with(&format!("async def {method}(")))
            .then(|| line.find(method))
            .flatten(),
            Language::Cpp => {
                let before = trimmed.split_once('(').map(|(before, _)| before.trim());
                (before.is_some_and(|before| {
                    before.split_whitespace().last().is_some_and(|name| {
                        name.trim_start_matches('*').trim_start_matches('&') == method
                    })
                }))
                .then(|| line.find(method))
                .flatten()
            }
            Language::Swift => trimmed
                .contains(&format!("func {method}("))
                .then(|| line.find(method))
                .flatten(),
            Language::Go => trimmed.strip_prefix("func (").and_then(|after| {
                let close = after.find(')')?;
                let receiver = after[..close]
                    .split_whitespace()
                    .last()?
                    .trim_start_matches('*');
                let tail = after[close + 1..].trim_start();
                (receiver == owner && tail.starts_with(&format!("{method}(")))
                    .then(|| line.find(method))
                    .flatten()
            }),
        };
        if let Some(col) = name_at {
            let offset = code
                .split_inclusive('\n')
                .take(idx)
                .map(str::len)
                .sum::<usize>()
                + col;
            return crate::signature::position_at(code, offset);
        }
    }
    anyhow::bail!("cannot locate the selected `{owner}.{method}` declaration")
}

pub fn extract_receiver(before: &str) -> Option<&str> {
    let trimmed = before.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let mut i = bytes.len();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;

    while i > 0 {
        let b = bytes[i - 1];
        match b {
            b')' => depth_paren += 1,
            b'(' => {
                if depth_paren > 0 {
                    depth_paren -= 1;
                } else {
                    break;
                }
            }
            b']' => depth_bracket += 1,
            b'[' => {
                if depth_bracket > 0 {
                    depth_bracket -= 1;
                } else {
                    break;
                }
            }
            _ => {
                if depth_paren == 0 && depth_bracket == 0 {
                    let c = b as char;
                    if !(is_ident(c) || c == '.' || c == '?' || c == '!' || c == '>' || c == '-') {
                        break;
                    }
                }
            }
        }
        i -= 1;
    }
    let recv = trimmed[i..].trim_start_matches("return ").trim_start();
    if recv.is_empty() { None } else { Some(recv) }
}

pub fn find_method_at_line(code: &str, line_1based: u32) -> Option<(String, Option<String>)> {
    let lines: Vec<&str> = code.lines().collect();
    if line_1based == 0 || line_1based as usize > lines.len() {
        return None;
    }
    let target_idx = (line_1based - 1) as usize;
    let start_idx = target_idx.saturating_sub(2);
    let end_idx = std::cmp::min(target_idx + 2, lines.len().saturating_sub(1));

    for line in lines.iter().take(end_idx + 1).skip(start_idx) {
        let trimmed = line.trim();
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            let rest = trimmed.strip_prefix("async ").unwrap_or(trimmed);
            if let Some(after_def) = rest.strip_prefix("def ")
                && let Some(paren) = after_def.find('(')
            {
                let name = after_def[..paren].trim();
                return Some((name.to_string(), None));
            }
        }
        if let Some(after_func) = trimmed.strip_prefix("func ") {
            if after_func.starts_with('(') {
                if let Some(close_recv) = after_func.find(')') {
                    let recv_part = &after_func[1..close_recv];
                    let type_name = recv_part
                        .split_whitespace()
                        .last()
                        .map(|t| t.trim_start_matches('*'))
                        .unwrap_or("");
                    let after_recv = after_func[close_recv + 1..].trim_start();
                    if let Some(paren) = after_recv.find('(') {
                        let name = after_recv[..paren].trim();
                        return Some((name.to_string(), Some(type_name.to_string())));
                    }
                }
            } else if let Some(paren) = after_func.find('(') {
                let name = after_func[..paren].trim();
                return Some((name.to_string(), None));
            }
        }
        if let Some(pos) = trimmed.find("func ") {
            let after = &trimmed[pos + 5..];
            if let Some(paren) = after.find('(') {
                let name = after[..paren].trim();
                return Some((name.to_string(), None));
            }
        }
        if let Some(paren) = trimmed.find('(') {
            let before = trimmed[..paren].trim();
            if let Some(name) = before.split_whitespace().last() {
                let clean = name.trim_start_matches('*').trim_start_matches('&');
                if !clean.is_empty()
                    && clean.chars().all(is_ident)
                    && clean != "if"
                    && clean != "while"
                    && clean != "for"
                    && clean != "switch"
                    && clean != "catch"
                {
                    return Some((clean.to_string(), None));
                }
            }
        }
    }
    None
}

fn rewrite_calls_in_code(
    code: &str,
    target_method: &str,
    owner_class: &str,
    lang: Language,
    file_rel: &str,
    blocked: &mut Vec<String>,
    semantic_references: Option<&mut std::collections::HashSet<(u32, u32)>>,
) -> (String, usize) {
    let mut out = String::new();
    let mut rewritten = 0;
    let needle_dot = format!(".{target_method}(");
    let needle_arrow = format!("->{target_method}(");
    let lexical_language = match lang {
        Language::TypeScript => crate::parameter_object::Language::TypeScript,
        Language::Python => crate::parameter_object::Language::Python,
        Language::Cpp => crate::parameter_object::Language::Cpp,
        Language::Swift => crate::parameter_object::Language::Swift,
        Language::Go => crate::parameter_object::Language::Go,
    };

    let mut semantic_references = semantic_references;
    let mut absolute_offset = 0usize;
    for (line_idx, raw_line) in code.split_inclusive('\n').enumerate() {
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let ending = &raw_line[line.len()..];
        let line_start = absolute_offset;
        absolute_offset += raw_line.len();
        let trimmed = line.trim_start();
        let comment_prefix = match lang {
            Language::Python => "#",
            _ => "//",
        };
        if trimmed.starts_with(comment_prefix) || trimmed.starts_with('*') {
            out.push_str(raw_line);
            continue;
        }

        let has_needle =
            line.contains(&needle_dot) || (lang == Language::Cpp && line.contains(&needle_arrow));
        if !has_needle {
            out.push_str(raw_line);
            continue;
        }

        let mut current_line = String::new();
        let mut cursor = 0usize;
        loop {
            let next_dot = line[cursor..]
                .find(&needle_dot)
                .map(|p| (cursor + p, false));
            let next_arrow = (lang == Language::Cpp)
                .then(|| {
                    line[cursor..]
                        .find(&needle_arrow)
                        .map(|p| (cursor + p, true))
                })
                .flatten();
            let Some((pos, is_arrow)) = (match (next_dot, next_arrow) {
                (Some(dot), Some(arrow)) => Some(if dot.0 <= arrow.0 { dot } else { arrow }),
                (Some(dot), None) => Some(dot),
                (None, Some(arrow)) => Some(arrow),
                (None, None) => None,
            }) else {
                break;
            };
            let op_len = if is_arrow { 2 } else { 1 };
            let end = pos + op_len + target_method.len() + 1;
            if crate::inline_parameter::is_in_string(code, line_start + pos, lexical_language) {
                current_line.push_str(&line[cursor..end]);
                cursor = end;
                continue;
            }
            let method_offset = line_start + pos + op_len;
            let method_position = crate::signature::position_at(code, method_offset).ok();
            let Some(references) = semantic_references.as_deref_mut() else {
                current_line.push_str(&line[cursor..end]);
                cursor = end;
                continue;
            };
            let Some(method_position) = method_position else {
                current_line.push_str(&line[cursor..end]);
                cursor = end;
                continue;
            };
            if !references.remove(&method_position) {
                current_line.push_str(&line[cursor..end]);
                cursor = end;
                continue;
            }
            let before = &line[..pos];

            if let Some(recv) = extract_receiver(before) {
                if recv == owner_class {
                    current_line.push_str(&line[cursor..end]);
                    cursor = end;
                    continue;
                }
                let site = format!("{file_rel}:{}:{}", line_idx + 1, pos + 1);
                if receiver_has_effects(recv) {
                    blocked.push(format!("{site} `{recv}` is evaluated for what it does"));
                }
                let target_call = match lang {
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
                let recv_start = pos - recv.len();
                current_line.push_str(&line[cursor..recv_start]);
                current_line.push_str(&target_call);
                rewritten += 1;
                cursor = end;
            } else {
                current_line.push_str(&line[cursor..end]);
                cursor = end;
            }
        }
        current_line.push_str(&line[cursor..]);
        out.push_str(&current_line);
        out.push_str(ending);
    }
    (out, rewritten)
}

fn make_static_ts(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
    semantic_references: Option<&mut std::collections::HashSet<(u32, u32)>>,
) -> Result<(String, String, String, usize, Vec<String>)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ")
            || trimmed.starts_with("export class ")
            || trimmed.starts_with("export default class ")
        {
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

    let m_idx = method_line_idx
        .with_context(|| format!("Method `{target_method}` not found in class `{class_name}`"))?;

    let m_line = lines[m_idx];
    if m_line.contains("static ") {
        anyhow::bail!("`{target_method}` is already a static method");
    }

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

    let body_text = lines[m_idx..=m_body_end].join("\n");
    if mentions(&body_text, "this") {
        anyhow::bail!(
            "`{target_method}` uses `this`; only a method that never accesses instance state can be made static"
        );
    }

    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            let trimmed = line.trim_start();
            let indent = &line[..line.len() - trimmed.len()];
            let new_line = if let Some(rest) = trimmed.strip_prefix("public ") {
                format!("{indent}public static {rest}")
            } else if let Some(rest) = trimmed.strip_prefix("private ") {
                format!("{indent}private static {rest}")
            } else if let Some(rest) = trimmed.strip_prefix("protected ") {
                format!("{indent}protected static {rest}")
            } else {
                format!("{indent}static {trimmed}")
            };
            new_lines.push(new_line);
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let mut blocked = Vec::new();
    let (final_code, rewritten_calls) = rewrite_calls_in_code(
        &intermediate,
        target_method,
        &class_name,
        Language::TypeScript,
        "",
        &mut blocked,
        semantic_references,
    );

    Ok((
        class_name,
        target_method.to_string(),
        final_code,
        rewritten_calls,
        blocked,
    ))
}

fn make_static_py(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
    semantic_references: Option<&mut std::collections::HashSet<(u32, u32)>>,
) -> Result<(String, String, String, usize, Vec<String>)> {
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
            let name = after_class.split(['(', ':']).next().unwrap_or("").trim();
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

    let m_idx = method_line_idx
        .with_context(|| format!("Method `{target_method}` not found in class `{class_name}`"))?;

    if m_idx > 0 && lines[m_idx - 1].trim() == "@staticmethod" {
        anyhow::bail!("`{target_method}` is already a static method");
    }

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

    let body_text = lines[m_idx + 1..=m_body_end].join("\n");
    if mentions(&body_text, "self") {
        anyhow::bail!(
            "`{target_method}` uses `self`; only a method that never accesses instance state can be made static"
        );
    }

    let m_trimmed = lines[m_idx].trim_start();
    let indent_str = &lines[m_idx][..lines[m_idx].len() - m_trimmed.len()];

    let modified_m_line = if let Some(open_p) = m_trimmed.find('(') {
        if let Some(close_p) = m_trimmed.find(')') {
            let before_p = &m_trimmed[..open_p + 1];
            let params = &m_trimmed[open_p + 1..close_p];
            let after_p = &m_trimmed[close_p..];
            let stripped_params = if let Some(rest) = params.trim().strip_prefix("self,") {
                rest.trim_start()
            } else if params.trim() == "self" {
                ""
            } else if let Some(rest) = params.trim().strip_prefix("self: ") {
                if let Some(comma) = rest.find(',') {
                    rest[comma + 1..].trim_start()
                } else {
                    ""
                }
            } else {
                params
            };
            format!("{indent_str}{before_p}{stripped_params}{after_p}")
        } else {
            lines[m_idx].to_string()
        }
    } else {
        lines[m_idx].to_string()
    };

    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            new_lines.push(format!("{indent_str}@staticmethod"));
            new_lines.push(modified_m_line.clone());
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let mut blocked = Vec::new();
    let mut semantic_references = semantic_references;
    if let Some(references) = semantic_references.as_deref_mut() {
        let declaration_line = u32::try_from(m_idx + 1).unwrap_or(u32::MAX);
        *references = references
            .drain()
            .map(|(line, col)| {
                if line >= declaration_line {
                    (line.saturating_add(1), col)
                } else {
                    (line, col)
                }
            })
            .collect();
    }
    let (final_code, rewritten_calls) = rewrite_calls_in_code(
        &intermediate,
        target_method,
        &class_name,
        Language::Python,
        "",
        &mut blocked,
        semantic_references,
    );

    Ok((
        class_name,
        target_method.to_string(),
        final_code,
        rewritten_calls,
        blocked,
    ))
}

fn make_static_cpp(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
    semantic_references: Option<&mut std::collections::HashSet<(u32, u32)>>,
) -> Result<(String, String, String, usize, Vec<String>)> {
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
                    name = words[w_idx + 1]
                        .trim_matches(|c| c == '{' || c == ':')
                        .trim();
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

    let m_idx = method_line_idx
        .with_context(|| format!("Method `{target_method}` not found in class `{class_name}`"))?;

    let m_line = lines[m_idx];
    if m_line.trim_start().starts_with("static ") {
        anyhow::bail!("`{target_method}` is already static");
    }

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

    let body_text = lines[m_idx..=m_body_end].join("\n");
    if mentions(&body_text, "this") {
        anyhow::bail!(
            "`{target_method}` uses `this`; only a method that never accesses instance state can be made static"
        );
    }

    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            let trimmed = line.trim_start();
            let indent = &line[..line.len() - trimmed.len()];
            let without_const = if let Some(pos) = trimmed.find(") const") {
                let before = &trimmed[..pos + 1];
                let after = &trimmed[pos + 7..];
                format!("{before}{after}")
            } else {
                trimmed.to_string()
            };
            new_lines.push(format!("{indent}static {without_const}"));
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let mut blocked = Vec::new();
    let (final_code, rewritten_calls) = rewrite_calls_in_code(
        &intermediate,
        target_method,
        &class_name,
        Language::Cpp,
        "",
        &mut blocked,
        semantic_references,
    );

    Ok((
        class_name,
        target_method.to_string(),
        final_code,
        rewritten_calls,
        blocked,
    ))
}

fn make_static_swift(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
    semantic_references: Option<&mut std::collections::HashSet<(u32, u32)>>,
) -> Result<(String, String, String, usize, Vec<String>)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ")
            || trimmed.starts_with("struct ")
            || trimmed.starts_with("actor ")
        {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let mut name = "";
            for (w_idx, w) in words.iter().enumerate() {
                if (*w == "class" || *w == "struct" || *w == "actor") && w_idx + 1 < words.len() {
                    name = words[w_idx + 1]
                        .trim_matches(|c| c == '{' || c == ':')
                        .trim();
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

    let m_idx = method_line_idx
        .with_context(|| format!("Method `{target_method}` not found in `{class_name}`"))?;

    let m_line = lines[m_idx];
    if m_line.contains("static func ") || m_line.contains("class func ") {
        anyhow::bail!("`{target_method}` is already a static method");
    }

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

    let body_text = lines[m_idx..=m_body_end].join("\n");
    if mentions(&body_text, "self") {
        anyhow::bail!(
            "`{target_method}` uses `self`; only a method that never accesses instance state can be made static"
        );
    }

    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            let replaced = if line.contains("mutating func ") {
                line.replace("mutating func ", "static func ")
            } else {
                line.replace("func ", "static func ")
            };
            new_lines.push(replaced);
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let mut blocked = Vec::new();
    let (final_code, rewritten_calls) = rewrite_calls_in_code(
        &intermediate,
        target_method,
        &class_name,
        Language::Swift,
        "",
        &mut blocked,
        semantic_references,
    );

    Ok((
        class_name,
        target_method.to_string(),
        final_code,
        rewritten_calls,
        blocked,
    ))
}

fn make_static_go(
    code: &str,
    target_struct: Option<&str>,
    target_method: &str,
    semantic_references: Option<&mut std::collections::HashSet<(u32, u32)>>,
) -> Result<(String, String, String, usize, Vec<String>)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut method_line_idx = None;
    let mut receiver_name = String::new();
    let mut struct_name = String::new();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("func (") {
            continue;
        }
        let after_func = &trimmed["func (".len()..];
        let Some(close_recv) = after_func.find(')') else {
            continue;
        };
        let recv_part = after_func[..close_recv].trim();
        let after_recv = after_func[close_recv + 1..].trim_start();
        let Some(open_p) = after_recv.find('(') else {
            continue;
        };
        let m_name = after_recv[..open_p].trim();
        if m_name == target_method {
            let recv_words: Vec<&str> = recv_part.split_whitespace().collect();
            if recv_words.len() >= 2 {
                let r_name = recv_words[0];
                let s_name = recv_words[1].trim_start_matches('*');
                if target_struct.is_none() || target_struct == Some(s_name) {
                    method_line_idx = Some(idx);
                    receiver_name = r_name.to_string();
                    struct_name = s_name.to_string();
                    break;
                }
            }
        }
    }

    let m_idx = method_line_idx
        .with_context(|| format!("Method `{target_method}` with receiver not found in Go file"))?;

    let mut m_body_end = m_idx;
    let mut m_depth = 0i32;
    let mut started = false;
    for (idx, line) in lines.iter().enumerate().skip(m_idx) {
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

    let mut body_parts = Vec::new();
    if let Some(pos) = lines[m_idx].find('{') {
        let after_brace = &lines[m_idx][pos + 1..];
        if !after_brace.trim().is_empty() {
            body_parts.push(after_brace);
        }
    }
    if m_body_end > m_idx {
        for line in &lines[m_idx + 1..m_body_end] {
            body_parts.push(*line);
        }
        if let Some(pos) = lines[m_body_end].rfind('}') {
            let before_brace = &lines[m_body_end][..pos];
            if !before_brace.trim().is_empty() {
                body_parts.push(before_brace);
            }
        } else {
            body_parts.push(lines[m_body_end]);
        }
    }
    let body_text = body_parts.join("\n");
    if mentions(&body_text, &receiver_name) {
        anyhow::bail!(
            "`{target_method}` uses receiver `{receiver_name}`; only a method that never accesses its receiver can be made static"
        );
    }

    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            let trimmed = line.trim_start();
            let indent = &line[..line.len() - trimmed.len()];
            let after_func = &trimmed["func (".len()..];
            let close_recv = after_func.find(')').unwrap();
            let after_recv = after_func[close_recv + 1..].trim_start();
            new_lines.push(format!("{indent}func {after_recv}"));
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let mut blocked = Vec::new();
    let (final_code, rewritten_calls) = rewrite_calls_in_code(
        &intermediate,
        target_method,
        &struct_name,
        Language::Go,
        "",
        &mut blocked,
        semantic_references,
    );

    Ok((
        struct_name,
        target_method.to_string(),
        final_code,
        rewritten_calls,
        blocked,
    ))
}

#[allow(clippy::too_many_arguments)]
pub async fn make_static_polyglot(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    class_name: Option<&str>,
    method_name: &str,
    apply: bool,
    force: bool,
) -> Result<MadeStatic> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Cannot read {}", file_path.display()))?;
    let lang = Language::from_path(file_path)
        .with_context(|| format!("Unsupported language for file: {}", file_path.display()))?;

    let (owner, method, _, _, mut blocked) = match lang {
        Language::TypeScript => make_static_ts(&content, class_name, method_name, None)?,
        Language::Python => make_static_py(&content, class_name, method_name, None)?,
        Language::Cpp => make_static_cpp(&content, class_name, method_name, None)?,
        Language::Swift => make_static_swift(&content, class_name, method_name, None)?,
        Language::Go => make_static_go(&content, class_name, method_name, None)?,
    };

    let (declaration_line, declaration_col) =
        method_declaration_position(&content, lang, &owner, &method)?;
    let references = crate::signature::references(
        remote,
        workspace_root,
        file_path,
        declaration_line,
        declaration_col,
    )
    .await
    .with_context(|| format!("cannot find calls to `{owner}.{method}`; nothing was planned"))?;
    let mut references_by_file: BTreeMap<PathBuf, std::collections::HashSet<(u32, u32)>> =
        BTreeMap::new();
    for (path, line, col) in references {
        let canonical = std::fs::canonicalize(&path).unwrap_or(path);
        references_by_file
            .entry(canonical)
            .or_default()
            .insert((line, col));
    }
    let canonical_target =
        std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    let mut target_references = references_by_file
        .remove(&canonical_target)
        .unwrap_or_default();
    let (_, _, new_content, file_rewritten, parser_blocked) = match lang {
        Language::TypeScript => make_static_ts(
            &content,
            Some(&owner),
            &method,
            Some(&mut target_references),
        )?,
        Language::Python => make_static_py(
            &content,
            Some(&owner),
            &method,
            Some(&mut target_references),
        )?,
        Language::Cpp => make_static_cpp(
            &content,
            Some(&owner),
            &method,
            Some(&mut target_references),
        )?,
        Language::Swift => make_static_swift(
            &content,
            Some(&owner),
            &method,
            Some(&mut target_references),
        )?,
        Language::Go => make_static_go(
            &content,
            Some(&owner),
            &method,
            Some(&mut target_references),
        )?,
    };
    blocked.extend(parser_blocked);

    let receiver = match lang {
        Language::TypeScript => "this".to_string(),
        Language::Python => "self".to_string(),
        Language::Cpp => "*this".to_string(),
        Language::Swift => "self".to_string(),
        Language::Go => format!("(*{owner})"),
    };

    let mut rewritten = vec![(file_path.to_string_lossy().to_string(), new_content)];
    let mut total_rewritten_calls = file_rewritten;
    let mut unmatched = Vec::new();
    for (line, col) in target_references {
        unmatched.push(format!(
            "{}:{line}:{col}: analyzer reference was not a supported call",
            display(workspace_root, file_path)
        ));
    }

    for entry in ignore::WalkBuilder::new(workspace_root).build().flatten() {
        let path = entry.path();
        if path.is_file()
            && path != file_path
            && lang.matches_extension(path)
            && let Ok(other_content) = std::fs::read_to_string(path)
            && other_content.contains(method_name)
        {
            let rel = display(workspace_root, path);
            let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            let mut file_references = references_by_file.remove(&canonical).unwrap_or_default();
            let (new_other, calls) = rewrite_calls_in_code(
                &other_content,
                method_name,
                &owner,
                lang,
                &rel,
                &mut blocked,
                Some(&mut file_references),
            );
            for (line, col) in file_references {
                unmatched.push(format!(
                    "{rel}:{line}:{col}: analyzer reference was not a supported call"
                ));
            }
            if new_other != other_content {
                rewritten.push((path.to_string_lossy().to_string(), new_other));
                total_rewritten_calls += calls;
            }
        }
    }

    for (path, references) in references_by_file {
        for (line, col) in references {
            unmatched.push(format!(
                "{}:{line}:{col}: analyzer reference was not a supported call",
                display(workspace_root, &path)
            ));
        }
    }

    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.clone()))
        .collect();
    let reports =
        crate::diagnostics::validate_texts(remote, workspace_root, &to_check, &[]).await?;
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
            blocked.is_empty(),
            "{} call site(s) would drop a receiver that does something; nothing was written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
        );
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) were not rewritten; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
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
    Ok(MadeStatic {
        owner,
        method: method.to_string(),
        root: workspace_root.to_path_buf(),
        file: rel_file,
        receiver,
        rewritten_calls: total_rewritten_calls,
        blocked,
        unmatched,
        rewritten,
        diagnostics,
        applied: apply,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs_for(code: &str, method: &str) -> std::collections::HashSet<(u32, u32)> {
        let mut refs = std::collections::HashSet::new();
        for needle in [format!(".{method}("), format!("->{method}(")] {
            for (offset, _) in code.match_indices(&needle) {
                if let Ok(position) =
                    crate::signature::position_at(code, offset + needle.find(method).unwrap())
                {
                    refs.insert(position);
                }
            }
        }
        refs
    }

    #[test]
    fn a_receiver_is_split_off_the_parameter_list() {
        assert_eq!(
            split_receiver("&self, x: u32"),
            Some(("&self".to_string(), "x: u32".to_string()))
        );
        assert_eq!(
            split_receiver("&mut self"),
            Some(("&mut self".to_string(), String::new()))
        );
        assert_eq!(
            split_receiver("self"),
            Some(("self".to_string(), String::new()))
        );
        assert_eq!(
            split_receiver("mut self, a: u8, b: u8"),
            Some(("mut self".to_string(), "a: u8, b: u8".to_string()))
        );
        assert_eq!(
            split_receiver("&'a self"),
            Some(("&'a self".to_string(), String::new()))
        );
        assert_eq!(
            split_receiver("self: Box<Self>"),
            Some(("self: Box<Self>".to_string(), String::new()))
        );
        assert_eq!(split_receiver("x: u32"), None);
        assert_eq!(split_receiver(""), None);
        assert_eq!(split_receiver("selfish: u8"), None);
    }

    #[test]
    fn only_a_receiver_that_does_something_is_kept() {
        for plain in ["s", "self.store", "crate::GLOBAL", "a.b.c", "&x"] {
            assert!(!receiver_has_effects(plain), "{plain}");
        }
        for effect in [
            "load()",
            "self.get()?",
            "vec![1]",
            "make().await",
            "items[0]",
        ] {
            assert!(receiver_has_effects(effect), "{effect}");
        }
    }

    #[test]
    fn a_word_is_mentioned_only_whole() {
        assert!(mentions("{ self.x }", "self"));
        assert!(!mentions("{ myself }", "self"));
        assert!(!mentions("{ Self::new() }", "self"));
    }

    fn report() -> MadeStatic {
        MadeStatic {
            owner: "S".into(),
            method: "twice".into(),
            root: "/root".into(),
            file: "src/lib.rs".into(),
            receiver: "&self".into(),
            rewritten_calls: 2,
            blocked: vec!["src/app.rs:3:9 `load()?` is evaluated for what it does".into()],
            unmatched: vec!["src/app.rs:7:5 (not a call: the method used as a value)".into()],
            rewritten: vec![("/root/src/lib.rs".into(), "fn main() {}\n".into())],
            diagnostics: vec!["mismatched types (src/app.rs:4:5)".into()],
            applied: false,
        }
    }

    #[test]
    fn the_report_names_the_receiver_the_calls_and_what_stays() {
        let text = report().render(10_000);
        assert!(text.contains("the receiver `&self` is removed"), "{text}");
        assert!(
            text.contains("2 call site(s) now call `S::twice`"),
            "{text}"
        );
        assert!(text.contains("bind it to a variable first"), "{text}");
        assert!(text.contains("not a call"), "{text}");
        assert!(text.contains("the analyzer rejects the result"), "{text}");
        assert!(text.contains("nothing was written"), "{text}");
        let mut done = report();
        done.blocked.clear();
        done.unmatched.clear();
        done.diagnostics.clear();
        done.applied = true;
        let text = done.render(10);
        assert!(
            text.contains("0 errors")
                && text.contains("[applied to 1 file(s)]")
                && text.contains("diff truncated"),
            "{text}"
        );
    }

    #[test]
    fn make_static_ts_transforms_declaration_and_calls() {
        let ts_code = r#"class Calculator {
    add(a: number, b: number): number {
        return a + b;
    }

    total(): number {
        return this.add(10, 20);
    }
}

function test() {
    const calc = new Calculator();
}
"#;
        let mut refs = refs_for(ts_code, "add");
        let (owner, method, transformed, calls, blocked) =
            make_static_ts(ts_code, Some("Calculator"), "add", Some(&mut refs)).unwrap();
        assert_eq!(owner, "Calculator");
        assert_eq!(method, "add");
        assert_eq!(calls, 1);
        assert!(blocked.is_empty());
        assert!(transformed.contains("static add(a: number, b: number): number {"));
        assert!(transformed.contains("Calculator.add(10, 20)"));
    }

    #[test]
    fn make_static_does_not_rewrite_a_same_named_method_on_an_unresolved_receiver() {
        let code = "const value = set.add(10, 20);\n";
        let mut blocked = Vec::new();
        let (rewritten, count) = rewrite_calls_in_code(
            code,
            "add",
            "Calculator",
            Language::TypeScript,
            "example.ts",
            &mut blocked,
            Some(&mut std::collections::HashSet::new()),
        );
        assert_eq!(count, 0);
        assert_eq!(rewritten, code);
        assert!(blocked.is_empty());
    }

    #[test]
    fn make_static_ts_rejects_this_access() {
        let ts_code = r#"class Counter {
    val: number = 0;
    bump(): void {
        this.val += 1;
    }
}
"#;
        let err = make_static_ts(ts_code, Some("Counter"), "bump", None).unwrap_err();
        assert!(err.to_string().contains("uses `this`"));
    }

    #[test]
    fn make_static_py_transforms_declaration_and_calls() {
        let py_code = r#"class MathUtil:
    def multiply(self, x: int, y: int) -> int:
        return x * y

def run():
    util = MathUtil()
    result = util.multiply(5, 6)
"#;
        let mut refs = refs_for(py_code, "multiply");
        let (owner, method, transformed, calls, blocked) =
            make_static_py(py_code, Some("MathUtil"), "multiply", Some(&mut refs)).unwrap();
        assert_eq!(owner, "MathUtil");
        assert_eq!(method, "multiply");
        assert_eq!(calls, 1);
        assert!(blocked.is_empty());
        assert!(transformed.contains("@staticmethod\n    def multiply(x: int, y: int) -> int:"));
        assert!(transformed.contains("MathUtil.multiply(5, 6)"));
    }

    #[test]
    fn make_static_cpp_transforms_declaration_and_calls() {
        let cpp_code = r#"class Util {
public:
    int sum(int a, int b) const {
        return a + b;
    }
};

void run() {
    Util u;
    int s = u.sum(3, 4);
}
"#;
        let mut refs = refs_for(cpp_code, "sum");
        let (owner, method, transformed, calls, blocked) =
            make_static_cpp(cpp_code, Some("Util"), "sum", Some(&mut refs)).unwrap();
        assert_eq!(owner, "Util");
        assert_eq!(method, "sum");
        assert_eq!(calls, 1);
        assert!(blocked.is_empty());
        assert!(transformed.contains("static int sum(int a, int b) {"));
        assert!(transformed.contains("Util::sum(3, 4)"));
    }

    #[test]
    fn make_static_swift_transforms_declaration_and_calls() {
        let swift_code = r#"class Greeter {
    func greet(name: String) -> String {
        return "Hello " + name
    }
}

func test() {
    let g = Greeter()
    let msg = g.greet(name: "World")
}
"#;
        let mut refs = refs_for(swift_code, "greet");
        let (owner, method, transformed, calls, blocked) =
            make_static_swift(swift_code, Some("Greeter"), "greet", Some(&mut refs)).unwrap();
        assert_eq!(owner, "Greeter");
        assert_eq!(method, "greet");
        assert_eq!(calls, 1);
        assert!(blocked.is_empty());
        assert!(transformed.contains("static func greet(name: String) -> String {"));
        assert!(transformed.contains("Greeter.greet(name: \"World\")"));
    }

    #[test]
    fn make_static_does_not_duplicate_modifier_for_mutating_swift_method() {
        let source = "struct Counter {\n    mutating func reset() {\n    }\n}\n";
        let (_, _, rewritten, _, blocked) = make_static_swift(
            source,
            Some("Counter"),
            "reset",
            Some(&mut std::collections::HashSet::new()),
        )
        .unwrap();
        assert!(rewritten.contains("static func reset()"), "{rewritten}");
        assert!(!rewritten.contains("static static func"), "{rewritten}");
        assert!(blocked.is_empty(), "{blocked:?}");
    }

    #[test]
    fn static_call_rewrite_skips_string_literals_on_lines_with_real_calls() {
        let code = "let example = \"this.add(1)\"; const result = this.add(2);\n";
        let mut blocked = Vec::new();
        let (rewritten, count) = rewrite_calls_in_code(
            code,
            "add",
            "Calculator",
            Language::TypeScript,
            "example.ts",
            &mut blocked,
            Some(&mut refs_for(code, "add")),
        );
        assert_eq!(count, 1);
        assert!(rewritten.contains("\"this.add(1)\""), "{rewritten}");
        assert!(rewritten.contains("Calculator.add(2)"), "{rewritten}");
        assert!(blocked.is_empty(), "{blocked:?}");
    }

    #[test]
    fn make_static_go_transforms_declaration_and_calls() {
        let go_code = r#"package main

type Service struct{}

func (s *Service) Process(data string) string {
    return "processed:" + data
}

func main() {
    svc := &Service{}
    res := svc.Process("test")
}
"#;
        let mut refs = refs_for(go_code, "Process");
        let (owner, method, transformed, calls, blocked) =
            make_static_go(go_code, Some("Service"), "Process", Some(&mut refs)).unwrap();
        assert_eq!(owner, "Service");
        assert_eq!(method, "Process");
        assert_eq!(calls, 1);
        assert!(blocked.is_empty());
        assert!(transformed.contains("func Process(data string) string {"));
        assert!(transformed.contains("Process(\"test\")"));
    }

    #[test]
    fn make_static_catches_effectful_receiver() {
        let ts_code = r#"class Worker {
    run(): void {}
}

function test() {
    getWorker()?.run();
}
"#;
        let mut refs = refs_for(ts_code, "run");
        let (_, _, _, _, blocked) =
            make_static_ts(ts_code, Some("Worker"), "run", Some(&mut refs)).unwrap();
        assert_eq!(blocked.len(), 1);
        assert!(blocked[0].contains("is evaluated for what it does"));
    }
}
