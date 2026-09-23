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
    /// Call sites whose receiver would be dropped although evaluating it does something.
    pub blocked: Vec<String>,
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl MadeStatic {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}::{}` ({})\n\n- the receiver `{}` is removed: it was never used\n- {} call site(s) \
             now call `{}::{}`\n\n",
            self.owner,
            self.method,
            self.file,
            self.receiver,
            self.rewritten_calls,
            self.owner,
            self.method
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
                continue;
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
                continue;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
