//! Removing a parameter of a trait method: from the trait's declaration, from every
//! implementation, and from every call.
//!
//! `change_signature` rewrites one declaration and its callers. A trait method has one
//! declaration in the trait and one in each `impl`, and the implementations may name the
//! parameter differently (`_unused`), so the parameter is removed by its position. A method call
//! passes it at that position; a path call (`Shape::area(c, …)`) passes the receiver first. The
//! parameter must not be used by any body that declares it, and an argument that does something
//! (a call, a macro, `?`, `.await`) is not dropped silently. The whole change is type-checked in
//! one overlay before anything is written.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Where a method is declared: in a trait, or in an implementation of one.
#[derive(Debug, Clone, PartialEq)]
pub enum Owner {
    /// In `trait Name { … }`.
    Trait { name: String },
    /// In `impl Trait for Type { … }`; the offset of the trait's name in the header.
    Impl { trait_at: usize },
}

/// What removing the parameter did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TraitParameter {
    pub method: String,
    pub parameter: String,
    /// The parameter's position, not counting the receiver.
    pub index: usize,
    #[serde(skip)]
    pub root: PathBuf,
    /// `file:line` of the trait's declaration and of every implementation.
    pub declarations: Vec<String>,
    pub calls: usize,
    /// Why nothing may be written: a body that uses the parameter, an argument that does
    /// something, a use that is not a call.
    pub blocked: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl TraitParameter {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "the parameter `{}` (#{} after the receiver) leaves `{}`: {} declaration(s), {} call(s)\n",
            self.parameter,
            self.index + 1,
            self.method,
            self.declarations.len(),
            self.calls
        );
        for d in &self.declarations {
            out.push_str(&format!("- {d}\n"));
        }
        if !self.blocked.is_empty() {
            out.push_str("\nnothing may be written while:\n");
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        out.push('\n');
        let mut body = String::new();
        for (path, new_text) in &self.rewritten {
            let old = crate::refactor::text_before_apply(Path::new(path));
            let rel = display(&self.root, Path::new(path));
            body.push_str(
                &similar::TextDiff::from_lines(&old, new_text)
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        out.push_str(if self.applied {
            "\n[applied]\n"
        } else {
            "\nnothing was written\n"
        });
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

/// Whether `text` uses `name` as a word of its own.
fn mentions(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(i, _)| {
        !text[..i].chars().next_back().is_some_and(is_ident)
            && !text[i + name.len()..].chars().next().is_some_and(is_ident)
    })
}

/// The trait or trait implementation whose block holds `at`, if the innermost block around it
/// is one.
pub fn owner_of(text: &str, at: usize) -> Option<Owner> {
    let mut depth = 0i32;
    let open = text[..at].char_indices().rev().find_map(|(i, c)| {
        match c {
            '}' => depth += 1,
            '{' if depth == 0 => return Some(i),
            '{' => depth -= 1,
            _ => {}
        }
        None
    })?;
    let header_start = text[..open].rfind([';', '}', '{']).map_or(0, |i| i + 1);
    let header = &text[header_start..open];
    // Attributes and doc comments above the item are not its header.
    let item_at = header
        .match_indices("trait ")
        .chain(header.match_indices("impl"))
        .map(|(i, _)| i)
        .filter(|i| !header[..*i].chars().next_back().is_some_and(is_ident))
        .min()?;
    let item = &header[item_at..];
    if let Some(rest) = item.strip_prefix("trait ") {
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|c| is_ident(*c))
            .collect();
        return (!name.is_empty()).then_some(Owner::Trait { name });
    }
    // `impl<T> path::Trait<X> for Type`: the trait is the last segment before ` for `.
    let rest = item.strip_prefix("impl")?;
    let rest_at = header_start + item_at + 4;
    // The implementation's own generics (`impl<T: Copy>`) come first and are skipped.
    let lead = rest.len() - rest.trim_start().len();
    let mut generics_end = 0;
    if rest[lead..].starts_with('<') {
        let mut angle = 0i32;
        for (i, c) in rest.char_indices().skip(lead) {
            match c {
                '<' => angle += 1,
                '>' => {
                    angle -= 1;
                    if angle == 0 {
                        generics_end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let mut angle = 0i32;
    let mut trait_end = None;
    for (i, c) in rest.char_indices().skip_while(|(i, _)| *i < generics_end) {
        match c {
            '<' => angle += 1,
            '>' => angle -= 1,
            _ if angle == 0 && rest[i..].starts_with(" for ") => {
                trait_end = Some(i);
                break;
            }
            _ => {}
        }
    }
    let trait_path = &rest[generics_end..trait_end?];
    // The last path segment, before any generic arguments of the trait.
    let bare_end = trait_path.find('<').unwrap_or(trait_path.len());
    let bare = &trait_path[..bare_end];
    let name_start = bare
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map(|(i, _)| i)?;
    Some(Owner::Impl {
        trait_at: rest_at + generics_end + name_start,
    })
}

/// The byte spans of the items of a comma-separated list (`a, b(c, d), e`), trimmed, relative
/// to the list.
pub fn item_spans(list: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    // `<` opens generic arguments only right after a name or `::` (`Vec<u8>`, `f::<T>`); with a
    // space before it, it is a comparison.
    let mut generics = 0i32;
    let mut start = 0;
    let chars: Vec<(usize, char)> = list.char_indices().collect();
    let push = |out: &mut Vec<(usize, usize)>, from: usize, to: usize| {
        let item = &list[from..to];
        let lead = item.len() - item.trim_start().len();
        let trail = item.len() - item.trim_end().len();
        if from + lead < to - trail {
            out.push((from + lead, to - trail));
        }
    };
    for (k, (i, c)) in chars.iter().copied().enumerate() {
        let prev = if k > 0 { chars[k - 1].1 } else { ' ' };
        let next = chars.get(k + 1).map_or(' ', |(_, c)| *c);
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '<' if (is_ident(prev) || prev == ':') && next != '<' && next != '=' => generics += 1,
            '>' if generics > 0 && prev != '-' && prev != '=' => generics -= 1,
            ',' if depth == 0 && generics == 0 => {
                push(&mut out, start, i);
                start = i + 1;
            }
            _ => {}
        }
    }
    push(&mut out, start, list.len());
    out
}

/// The range of `list` to cut to remove item `index`, with the comma that separates it: from
/// its start to the next item, or, for the last, from the end of the one before.
pub fn removal(spans: &[(usize, usize)], index: usize) -> Option<(usize, usize)> {
    let (from, to) = *spans.get(index)?;
    Some(if let Some((next, _)) = spans.get(index + 1) {
        (from, *next)
    } else if index > 0 {
        (spans[index - 1].1, to)
    } else {
        (from, to)
    })
}

/// Why dropping `arg` would change what the program does, if it would.
pub fn effect_of(arg: &str) -> Option<&'static str> {
    if arg.contains(".await") {
        Some("awaits")
    } else if arg.contains('?') {
        Some("can return early with `?`")
    } else if ["!(", "![", "!{"].iter().any(|m| arg.contains(m)) {
        Some("expands a macro")
    } else if arg.contains('(') {
        Some("calls something")
    } else {
        None
    }
}

/// The locations in an LSP `Location[]` answer, as (path, 1-based line, 1-based column).
fn locations(answer: &serde_json::Value) -> Vec<(PathBuf, u32, u32)> {
    answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|loc| {
            let uri = loc.get("uri").or_else(|| loc.get("targetUri"))?.as_str()?;
            let start = loc
                .pointer("/range/start")
                .or_else(|| loc.pointer("/targetSelectionRange/start"))?;
            Some((
                PathBuf::from(crate::remote_fs::uri_to_path(uri)),
                start.get("line")?.as_u64()? as u32 + 1,
                start.get("character")?.as_u64()? as u32 + 1,
            ))
        })
        .collect()
}

fn position(path: &Path, line: u32, col: u32) -> Result<serde_json::Value> {
    let uri = url::Url::from_file_path(path)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", path))?
        .to_string();
    Ok(serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
    }))
}

/// The offset of `fn method` inside the block that opens at or after `from` in `text`.
fn method_in_block(text: &str, from: usize, method: &str) -> Option<usize> {
    let open = from + text[from..].find('{')?;
    let close = crate::parameter_object::matching_bracket(text, open)?;
    let needle = format!("fn {method}");
    text[open..close]
        .match_indices(&needle)
        .map(|(i, _)| open + i)
        .find(|i| {
            !text[..*i].chars().next_back().is_some_and(is_ident)
                && !text[i + needle.len()..]
                    .chars()
                    .next()
                    .is_some_and(is_ident)
        })
        .map(|i| i + 3)
}

/// Removes the parameter `index` (after the receiver) of the trait method whose name is at
/// `fn_at` of `file` (in the trait or in an implementation of it), from every declaration and
/// every call. Nothing is written unless `apply` and nothing blocks it and the analyzer accepts
/// the result, or `force`.
pub async fn remove_parameter(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    fn_at: usize,
    index: usize,
    apply: bool,
    force: bool,
) -> Result<TraitParameter> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let root = &canon(root);
    let file = &canon(file);
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    texts.insert(file.clone(), text.clone());
    let method: String = text[fn_at..].chars().take_while(|c| is_ident(*c)).collect();
    anyhow::ensure!(!method.is_empty(), "no method name at that position");

    // The trait's own declaration of the method.
    let (trait_file, trait_at, trait_name) = match owner_of(&text, fn_at)
        .context("the method is not in a trait or a trait implementation")?
    {
        Owner::Trait { name } => (file.clone(), fn_at, name),
        Owner::Impl { trait_at } => {
            let (line, col) = crate::signature::line_col_at(&text, trait_at);
            let answer = crate::tools::execute_lsp_query(
                remote,
                root,
                file,
                "textDocument/definition",
                position(file, line, col)?,
            )
            .await
            .context("the analyzer does not say where the trait is declared")?;
            let (tfile, tline, tcol) = locations(&answer)
                .into_iter()
                .next()
                .context("the analyzer does not say where the trait is declared")?;
            let tfile = canon(&tfile);
            let ttext = std::fs::read_to_string(&tfile)
                .with_context(|| format!("cannot read {}", tfile.display()))?;
            let at = crate::signature::offset_of(&ttext, tline, tcol)
                .context("the trait's position is not in its file")?;
            let name: String = ttext[at..].chars().take_while(|c| is_ident(*c)).collect();
            let fn_at = method_in_block(&ttext, at, &method)
                .with_context(|| format!("the trait `{name}` does not declare `fn {method}`"))?;
            texts.insert(tfile.clone(), ttext);
            (tfile, fn_at, name)
        }
    };
    let (tl, tc) = crate::signature::line_col_at(&texts[&trait_file], trait_at);

    // Every implementation, and every reference: calls, and the implementations' names.
    let impls = locations(
        &crate::tools::execute_lsp_query(
            remote,
            root,
            &trait_file,
            "textDocument/implementation",
            position(&trait_file, tl, tc)?,
        )
        .await?,
    );
    let refs = crate::signature::references(remote, root, &trait_file, tl, tc).await?;
    let mut declarations: Vec<(PathBuf, u32, u32)> = vec![(trait_file.clone(), tl, tc)];
    declarations.extend(impls.into_iter().map(|(p, l, c)| (canon(&p), l, c)));
    for (path, _, _) in &declarations {
        if !texts.contains_key(path) {
            texts.insert(
                path.clone(),
                std::fs::read_to_string(path)
                    .with_context(|| format!("cannot read {}", path.display()))?,
            );
        }
    }

    let mut cuts: BTreeMap<PathBuf, Vec<(usize, usize)>> = BTreeMap::new();
    let mut blocked = Vec::new();
    let mut declared_lines = Vec::new();
    let mut parameter = String::new();
    let mut has_receiver = false;
    for (path, line, col) in &declarations {
        let text = &texts[path];
        let shown = format!("{}:{line}", display(root, path));
        let at = crate::signature::offset_of(text, *line, *col)
            .with_context(|| format!("{shown} is not in its file"))?;
        let (_, open, close) = crate::signature::param_span(text, at)
            .with_context(|| format!("{shown}: no parameter list after the name"))?;
        let list = &text[open..close];
        let (receiver, declared) = crate::signature::parse_declared(list);
        has_receiver = receiver.is_some();
        let Some(removed) = declared.get(index) else {
            blocked.push(format!(
                "{shown}: declares {} parameter(s), not {}",
                declared.len(),
                index + 1
            ));
            continue;
        };
        if path == &trait_file && at == trait_at {
            parameter = removed.name.clone();
        }
        declared_lines.push(format!("{shown} ({})", removed.name));
        // The body, when there is one, must not use it.
        let after = &text[close..];
        if let Some(brace) = after.find(['{', ';'])
            && after.as_bytes()[brace] == b'{'
            && let Some(end) = crate::parameter_object::matching_bracket(text, close + brace)
        {
            let name = removed.name.as_str();
            if !name.starts_with('_') && mentions(&text[close + brace..end], name) {
                blocked.push(format!(
                    "{shown}: the body uses `{name}`; remove that use first"
                ));
            }
        }
        let offset = usize::from(receiver.is_some());
        let spans = item_spans(list);
        if let Some((from, to)) = removal(&spans, index + offset) {
            cuts.entry(path.clone())
                .or_default()
                .push((open + from, open + to));
        }
    }

    let mut calls = 0;
    for (path, line, col) in refs {
        let path = canon(&path);
        if declarations
            .iter()
            .any(|(p, l, c)| *p == path && *l == line && *c == col)
        {
            continue;
        }
        if !texts.contains_key(&path) {
            let Ok(t) = std::fs::read_to_string(&path) else {
                continue;
            };
            texts.insert(path.clone(), t);
        }
        let text = &texts[&path];
        let shown = format!("{}:{line}", display(root, &path));
        let Some(at) = crate::signature::offset_of(text, line, col) else {
            continue;
        };
        let name_end = at + method.len();
        let mut rest_at = name_end + (text[name_end..].len() - text[name_end..].trim_start().len());
        if text[rest_at..].starts_with("::<")
            && let Some(close) = {
                let lt = rest_at + 2;
                let mut depth = 0i32;
                text[lt..].char_indices().find_map(|(i, c)| {
                    match c {
                        '<' => depth += 1,
                        '>' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(lt + i);
                            }
                        }
                        _ => {}
                    }
                    None
                })
            }
        {
            rest_at = close + 1;
        }
        if !text[rest_at..].starts_with('(') {
            blocked.push(format!(
                "{shown}: `{method}` is used as a value, not called; what calls it passes the \
                 argument"
            ));
            continue;
        }
        let Some(close) = crate::parameter_object::matching_bracket(text, rest_at) else {
            continue;
        };
        let args = &text[rest_at + 1..close];
        let spans = item_spans(args);
        let method_call = text[..at].trim_end().ends_with('.');
        let arg_index = index + usize::from(!method_call && has_receiver);
        let Some((from, to)) = spans.get(arg_index).copied() else {
            blocked.push(format!(
                "{shown}: the call passes {} argument(s), fewer than the declaration",
                spans.len()
            ));
            continue;
        };
        let arg = &args[from..to];
        if let Some(effect) = effect_of(arg)
            && !force
        {
            blocked.push(format!(
                "{shown}: the argument `{arg}` {effect}; removing it would drop that (pass \
                 `force` to remove it anyway)"
            ));
        }
        if let Some((cut_from, cut_to)) = removal(&spans, arg_index) {
            cuts.entry(path.clone())
                .or_default()
                .push((rest_at + 1 + cut_from, rest_at + 1 + cut_to));
        }
        calls += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut ranges) in cuts {
        let mut text = texts[&path].clone();
        ranges.sort_by_key(|r| std::cmp::Reverse(r.0));
        ranges.dedup();
        for (from, to) in ranges {
            text.replace_range(from..to, "");
        }
        rewritten.insert(path, text);
    }

    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &[]).await?;
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
    if apply && ((blocked.is_empty() && diagnostics.is_empty()) || force) {
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }
    Ok(TraitParameter {
        method: format!("{trait_name}::{method}"),
        parameter,
        index,
        root: root.clone(),
        declarations: declared_lines,
        calls,
        blocked,
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
    fn a_method_s_owner_is_its_trait_or_the_trait_it_implements() {
        let t = "pub trait Shape {\n    fn area(&self) -> u32;\n}\nimpl<T: Copy> geo::Shape<T> for Circle {\n    fn area(&self) -> u32 { 1 }\n}\nimpl Circle {\n    fn r(&self) {}\n}\nfn free() {}\n";
        assert_eq!(
            owner_of(t, t.find("area").unwrap()),
            Some(Owner::Trait {
                name: "Shape".into()
            })
        );
        let in_impl = t.match_indices("area").nth(1).unwrap().0;
        let Some(Owner::Impl { trait_at }) = owner_of(t, in_impl) else {
            panic!("an implementation of a trait")
        };
        assert!(t[trait_at..].starts_with("Shape<T> for"));
        assert_eq!(owner_of(t, t.find("fn r").unwrap()), None);
        assert_eq!(owner_of(t, t.find("fn free").unwrap()), None);
    }

    #[test]
    fn an_item_is_cut_with_its_comma_and_nothing_else() {
        let list = "&self, scale: u32, f: Box<dyn Fn(u32, u32) -> u32>, last: u8";
        let spans = item_spans(list);
        assert_eq!(spans.len(), 4);
        let cut = |i: usize| {
            let (from, to) = removal(&spans, i).unwrap();
            format!("{}{}", &list[..from], &list[to..])
        };
        assert_eq!(cut(1), "&self, f: Box<dyn Fn(u32, u32) -> u32>, last: u8");
        assert_eq!(cut(3), "&self, scale: u32, f: Box<dyn Fn(u32, u32) -> u32>");
        let multi = "\n    a: u32,\n    b: u32,\n";
        let spans = item_spans(multi);
        let (from, to) = removal(&spans, 1).unwrap();
        assert_eq!(
            format!("{}{}", &multi[..from], &multi[to..]),
            "\n    a: u32,\n"
        );
        let one = item_spans("x");
        assert_eq!(removal(&one, 0), Some((0, 1)));
        assert_eq!(removal(&one, 1), None);
        assert_eq!(item_spans("a < b, c << 2").len(), 2);
    }

    #[test]
    fn an_argument_that_does_something_is_named() {
        assert_eq!(effect_of("7"), None);
        assert_eq!(effect_of("a != b"), None);
        assert_eq!(effect_of("next()"), Some("calls something"));
        assert_eq!(effect_of("vec![1]"), Some("expands a macro"));
        assert_eq!(effect_of("!flag"), None);
        assert_eq!(effect_of("r?"), Some("can return early with `?`"));
        assert_eq!(effect_of("f.await"), Some("awaits"));
        assert!(mentions("let _ = unused;", "unused") && !mentions("_unused", "unused"));
        let t = "trait T { fn a(&self); fn area(&self); }";
        assert_eq!(
            method_in_block(t, 0, "area").map(|i| &t[i..i + 4]),
            Some("area")
        );
        assert_eq!(method_in_block(t, 0, "b"), None);
    }
}
