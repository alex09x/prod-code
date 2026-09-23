//! Moving a method to the type of one of its parameters: `Order::price_with(&self, tax: &Tax)`
//! becomes `Tax::price_with(&self, order: &Order)`.
//!
//! The parameter becomes the receiver, borrowed as it was (`&Tax` → `&self`), and the old
//! receiver becomes a parameter in its place, typed as it was borrowed (`&self` → `&Order`). In
//! the body `self` becomes that parameter, the parameter becomes `self`, and `Self`, which meant
//! the old type, is spelled out. The method goes into the new type's inherent `impl` (one is
//! made after the type when there is none), and every call swaps the two:
//! `o.price_with(t, 1)` → `t.price_with(&o, 1)`, `Order::price_with(o, t, 1)` →
//! `Tax::price_with(t, o, 1)`. `&o` is right even when `o` is already a reference: an argument
//! of type `&&Order` coerces to `&Order`. The whole change is type-checked in one overlay first.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What the move did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MovedMethod {
    pub method: String,
    pub from_type: String,
    pub to_type: String,
    #[serde(skip)]
    pub root: PathBuf,
    /// The signature the method has now.
    pub signature: String,
    pub calls: usize,
    /// Why nothing may be written: a call whose receiver or argument does something, the method
    /// used as a value.
    pub blocked: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl MovedMethod {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}::{}` is now `{}::{}`: `{}`; {} call(s) rewritten\n",
            self.from_type, self.method, self.to_type, self.method, self.signature, self.calls
        );
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
            if self.diagnostics.iter().any(|d| d.contains("cannot find")) {
                out.push_str(&format!(
                    "\na name the body uses resolves where `{}` was and not where it goes: import \
                     it in the new file, or spell its path, and run this again\n",
                    self.method
                ));
            }
        }
        out.push_str(if self.applied {
            "\n[applied]\n"
        } else {
            "\nnothing was written; pass `apply: true` to make these edits\n"
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

/// `Order` for `Order`, `Wrapper<T>` and `crate::m::Order`.
fn base_name(ty: &str) -> &str {
    let ty = ty.trim();
    let ty = ty.split('<').next().unwrap_or(ty);
    ty.rsplit("::").next().unwrap_or(ty).trim()
}

/// `order` for `Order`, `line_item` for `LineItem`.
pub fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The type a receiver stood for, as a parameter's type: `&self` → `&Order`, `&mut self` →
/// `&mut Order`, `self` → `Order`; and whether the binding was `mut`. `None` for a receiver with
/// an explicit type (`self: Box<Self>`).
pub fn receiver_as_type(receiver: &str, owner: &str) -> Option<(String, bool)> {
    let r = receiver.trim();
    if r.contains(':') {
        return None;
    }
    match r {
        "self" => Some((owner.to_string(), false)),
        "mut self" => Some((owner.to_string(), true)),
        _ => {
            let rest = r.strip_prefix('&')?.trim_start();
            let (lifetime, rest) = if rest.starts_with('\'') {
                let end = rest.find(char::is_whitespace)?;
                (format!("{} ", &rest[..end]), rest[end..].trim_start())
            } else {
                (String::new(), rest)
            };
            match rest {
                "self" => Some((format!("&{lifetime}{owner}"), false)),
                "mut self" => Some((format!("&{lifetime}mut {owner}"), false)),
                _ => None,
            }
        }
    }
}

/// `text` with every whole identifier in `map` replaced, in one pass, so `self` → `order` and
/// `tax` → `self` do not run into each other.
pub fn swap_names(text: &str, map: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    let mut last = 0;
    while let Some((i, c)) = chars.next() {
        if !is_ident(c) || text[..i].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let mut end = i + c.len_utf8();
        while let Some((j, d)) = chars.peek().copied() {
            if is_ident(d) {
                end = j + d.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let word = &text[i..end];
        // A field or method named like the word (`x.self_`, `x.tax`) is not the binding.
        let after_dot = text[..i].ends_with('.') && !text[..i].ends_with("..");
        if let Some((_, to)) = map.iter().find(|(from, _)| *from == word)
            && !after_dot
        {
            out.push_str(&text[last..i]);
            out.push_str(to);
            last = end;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// The span of the item that starts at `line` (1-based) together with its doc comment and
/// attributes, as byte offsets of its first line and of the end of its closing line.
fn item_span(text: &str, name_at: usize, body_close: usize) -> (usize, usize) {
    let (line, _) = crate::signature::line_col_at(text, name_at);
    let first_line = crate::move_item::with_doc_comment(text, line);
    let start = crate::signature::offset_of(text, first_line, 1).unwrap_or(name_at);
    let end = text[body_close..]
        .find('\n')
        .map_or(text.len(), |i| body_close + i + 1);
    (start, end)
}

/// What to cut to take the item at `span_start..span_end` out of the `impl` whose header
/// starts at `impl_at` and whose braces are at `impl_open` and `impl_close`: the item alone, or
/// the whole block (and the blank line above it) when nothing else is left in it.
pub fn cut_from_impl(
    text: &str,
    (impl_at, impl_open, impl_close): (usize, usize, usize),
    (span_start, span_end): (usize, usize),
) -> (usize, usize) {
    let emptied = text[impl_open + 1..span_start].trim().is_empty()
        && text[span_end..impl_close].trim().is_empty();
    if !emptied {
        return (span_start, span_end);
    }
    let line_start = text[..impl_at].rfind('\n').map_or(0, |i| i + 1);
    let start = if text[..line_start].ends_with("\n\n") {
        line_start - 1
    } else {
        line_start
    };
    let end = text[impl_close..]
        .find('\n')
        .map_or(text.len(), |i| impl_close + i + 1);
    (start, end)
}

/// Where `method_text` goes in `target_text` and what is inserted there: before the closing
/// brace of `target_name`'s inherent `impl`, after a blank line; or, when the type has none, in
/// a new `impl` right after the type's declaration, which starts on `def_line`.
fn insertion(
    target_text: &str,
    target_name: &str,
    def_line: u32,
    method_text: &str,
) -> Result<(usize, String)> {
    let target_impl = crate::extract_field::impl_blocks(target_text)
        .into_iter()
        .find(|(ty, at, o, _)| ty == target_name && !target_text[*at..*o].contains(" for "));
    if let Some((_, _, _, impl_close)) = target_impl {
        let before = target_text[..impl_close].trim_end_matches([' ', '\t']);
        let lead = if before.ends_with("{\n") || before.ends_with('{') {
            ""
        } else {
            "\n"
        };
        return Ok((before.len(), format!("{lead}{method_text}")));
    }
    // After the type's declaration: its closing `}` or `;`.
    let decl_at = crate::signature::offset_of(target_text, def_line, 1)
        .context("the type's declaration is not in its file")?;
    let end = target_text[decl_at..]
        .find(['{', ';'])
        .map(|i| decl_at + i)
        .context("the type's declaration does not end")?;
    let end = if target_text.as_bytes()[end] == b'{' {
        crate::parameter_object::matching_bracket(target_text, end)
            .context("the type's declaration does not close")?
    } else {
        end
    };
    let insert_at = target_text[end..]
        .find('\n')
        .map_or(target_text.len(), |i| end + i + 1);
    Ok((
        insert_at,
        format!("\nimpl {target_name} {{\n{method_text}}}\n"),
    ))
}

/// The errors the analyzer finds in `edits`, one line each.
async fn errors_of(
    remote: SocketAddr,
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
) -> Result<Vec<String>> {
    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, &[]).await?;
    Ok(reports
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
        .collect())
}

/// Moves the associated function (no `self`) whose name is at `line`:`col` of `file` into the
/// inherent `impl` of the type `to_type`: `Self` in it is spelled out as the old type, and every
/// path to it (`Order::f`, `Self::f`, called or used as a value) names the new type. Nothing is
/// written unless `apply`, nothing blocks it and the analyzer accepts the result, or `force`.
#[allow(clippy::too_many_arguments)]
pub async fn move_associated_function(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    to_type: &str,
    apply: bool,
    force: bool,
) -> Result<MovedMethod> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let root = &canon(root);
    let file = &canon(file);
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = crate::signature::offset_of(&text, line, col)
        .context("the position is not inside the file")?;
    let name_at = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    anyhow::ensure!(
        text[..name_at].trim_end().ends_with("fn"),
        "the position is not the name of a function declaration"
    );
    let (name, open, close) = crate::signature::param_span(&text, name_at)
        .context("the function has no parameter list")?;
    let (receiver, _) = crate::signature::parse_declared(&text[open..close]);
    anyhow::ensure!(
        receiver.is_none(),
        "`{name}` takes `self`; a method moves to the type of one of its parameters (`to_param`)"
    );
    let (owner, impl_at, impl_open, impl_close) = crate::extract_field::impl_blocks(&text)
        .into_iter()
        .filter(|(_, _, o, c)| *o < name_at && name_at < *c)
        .min_by_key(|(_, _, o, c)| c - o)
        .context("the function is not inside an `impl` block")?;
    let impl_header = &text[impl_at..impl_open];
    anyhow::ensure!(
        !impl_header.contains(" for "),
        "`{name}` implements a trait's function; it belongs to the trait, not to `{owner}`"
    );
    anyhow::ensure!(
        !impl_header
            .trim_start_matches("impl")
            .trim_start()
            .starts_with('<'),
        "`impl` blocks with generic parameters are not handled"
    );
    anyhow::ensure!(owner != to_type, "`{name}` is already in `{owner}`");

    // The type it goes to, by name.
    let hits = crate::tools::workspace_symbol_search(remote, root, to_type, Some(file), 50).await?;
    let found: Vec<_> = hits
        .into_iter()
        .filter(|h| h.name == to_type && matches!(h.kind, "Struct" | "Enum" | "Class"))
        .collect();
    let hit = match found.as_slice() {
        [one] => one,
        [] => anyhow::bail!("no struct or enum named `{to_type}` in the workspace"),
        many => anyhow::bail!(
            "`{to_type}` names {} types; the move does not guess: {}",
            many.len(),
            many.iter()
                .map(|h| h.render(root))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    };
    let target_file = canon(&hit.path);
    let def_line = hit.line;
    let (_, owner_module) = crate::move_item::module_of(file)?;
    let (_, target_module) = crate::move_item::module_of(&target_file)?;
    let owner_in_target = if owner_module.segments == target_module.segments {
        owner.clone()
    } else {
        format!(
            "{}::{owner}",
            owner_module.spelled_from(&target_module.krate)
        )
    };

    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{name}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the function's body does not close")?;
    let (item_start, span_end) = item_span(&text, name_at, body_close);
    let span_start = if text[..item_start].ends_with("\n\n") {
        item_start - 1
    } else {
        item_start
    };
    let map = [("Self", owner_in_target.as_str())];
    let method_text = format!(
        "{}{}\n",
        &text[item_start..name_at],
        swap_names(&text[name_at..=body_close], &map)
    );
    let signature = format!(
        "fn {}",
        swap_names(&text[name_at..body_open], &map).trim_end()
    );

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.clone(), text.clone());
    if !texts.contains_key(&target_file) {
        texts.insert(
            target_file.clone(),
            std::fs::read_to_string(&target_file)
                .with_context(|| format!("cannot read {}", target_file.display()))?,
        );
    }
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let (cut_start, cut_end) = cut_from_impl(
        &text,
        (impl_at, impl_open, impl_close),
        (span_start, span_end),
    );
    edits
        .entry(file.clone())
        .or_default()
        .push((cut_start, cut_end, String::new()));
    let (insert_at, inserted) = insertion(&texts[&target_file], to_type, def_line, &method_text)?;
    edits
        .entry(target_file.clone())
        .or_default()
        .push((insert_at, insert_at, inserted));

    // Every path to it names the new type.
    let mut blocked = Vec::new();
    let mut calls = 0;
    let (nl, nc) = crate::signature::line_col_at(&text, name_at);
    for (path, l, c) in crate::signature::references(remote, root, file, nl, nc).await? {
        let path = canon(&path);
        if !texts.contains_key(&path) {
            let Ok(t) = std::fs::read_to_string(&path) else {
                continue;
            };
            texts.insert(path.clone(), t);
        }
        let body = texts[&path].clone();
        let site = format!("{}:{l}", display(root, &path));
        let Some(at) = crate::signature::offset_of(&body, l, c) else {
            continue;
        };
        let before = body[..at].trim_end();
        let Some(qualifier) = before.strip_suffix("::") else {
            blocked.push(format!("{site}: `{name}` is named without its type's path"));
            continue;
        };
        let path_start = qualifier
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_ident(*c) || *c == ':')
            .last()
            .map_or(qualifier.len(), |(i, _)| i);
        let (_, caller_module) = crate::move_item::module_of(&path)?;
        let target_path = if caller_module.segments == target_module.segments {
            to_type.to_string()
        } else {
            format!(
                "{}::{to_type}",
                target_module.spelled_from(&caller_module.krate)
            )
        };
        // Inside the moved function itself the path moves with it.
        if path == *file && span_start <= at && at < span_end {
            continue;
        }
        edits
            .entry(path.clone())
            .or_default()
            .push((path_start, at, format!("{target_path}::")));
        calls += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut t = texts[&path].clone();
        file_edits.sort_by_key(|(from, _, _)| std::cmp::Reverse(*from));
        for (from, to, replacement) in file_edits {
            t.replace_range(from..to, &replacement);
        }
        rewritten.insert(path, t);
    }
    let diagnostics = errors_of(remote, root, &rewritten).await?;
    let mut applied = false;
    if apply && ((blocked.is_empty() && diagnostics.is_empty()) || force) {
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }
    Ok(MovedMethod {
        method: name,
        from_type: owner,
        to_type: to_type.to_string(),
        root: root.clone(),
        signature,
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

/// Makes the method whose name is at `line`:`col` of `file` a method of the type of its
/// parameter `to_param`. Nothing is written unless `apply`, nothing blocks it and the analyzer
/// accepts the result, or `force`.
#[allow(clippy::too_many_arguments)]
pub async fn move_method(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    to_param: &str,
    apply: bool,
    force: bool,
) -> Result<MovedMethod> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let root = &canon(root);
    let file = &canon(file);
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = crate::signature::offset_of(&text, line, col)
        .context("the position is not inside the file")?;
    let name_at = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    anyhow::ensure!(
        text[..name_at].trim_end().ends_with("fn"),
        "the position is not the name of a method declaration"
    );
    let (name, open, close) =
        crate::signature::param_span(&text, name_at).context("the method has no parameter list")?;
    let list = text[open..close].to_string();
    let (receiver, declared) = crate::signature::parse_declared(&list);
    let receiver = receiver.with_context(|| {
        format!("`{name}` takes no `self`; an associated function has no receiver to swap")
    })?;
    let (owner, impl_at, impl_open, impl_close) = crate::extract_field::impl_blocks(&text)
        .into_iter()
        .filter(|(_, _, o, c)| *o < name_at && name_at < *c)
        .min_by_key(|(_, _, o, c)| c - o)
        .context("the method is not inside an `impl` block")?;
    let impl_header = &text[impl_at..impl_open];
    anyhow::ensure!(
        !impl_header.contains(" for "),
        "`{name}` implements a trait method; it belongs to the trait, not to `{owner}`"
    );
    anyhow::ensure!(
        !impl_header
            .trim_start_matches("impl")
            .trim_start()
            .starts_with('<'),
        "`impl` blocks with generic parameters are not handled"
    );
    let index = declared
        .iter()
        .position(|d| d.name == to_param)
        .with_context(|| {
            format!(
                "`{name}` has no parameter `{to_param}`; it takes {}",
                declared
                    .iter()
                    .map(|d| d.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let target = &declared[index];
    let target_ty = crate::signature::split_params(&target.raw)
        .first()
        .and_then(|p| p.split_once(':').map(|(_, t)| t.trim().to_string()))
        .context("the parameter has no type")?;
    let target_name =
        base_name(target_ty.trim_start_matches('&').trim_start_matches("mut ")).to_string();
    anyhow::ensure!(
        target_name.chars().next().is_some_and(char::is_uppercase)
            && !target_ty.contains("impl ")
            && !target_ty.contains("dyn "),
        "`{to_param}: {target_ty}` is not a named type a method can move to"
    );
    let (new_receiver, _) = crate::to_method::receiver_for(&target.raw, &target_name)
        .with_context(|| format!("`{target_ty}` cannot become a receiver"))?;
    let (old_as_type, old_mut) = receiver_as_type(&receiver, "{owner}")
        .with_context(|| format!("the receiver `{receiver}` has a type of its own; not handled"))?;

    // The body, and names that would clash.
    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{name}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the method's body does not close")?;
    let body = &text[body_open..=body_close];
    let new_param = snake_case(&owner);
    anyhow::ensure!(
        !declared.iter().any(|d| d.name == new_param)
            && !body.contains(&format!("let {new_param}"))
            && !body.contains(&format!("let mut {new_param}")),
        "`{name}` already has a `{new_param}`, the name the old receiver would take"
    );

    // Where the target type is declared, and how each file spells the two types.
    let ty_offset = open
        + list
            .find(&format!("{to_param}:"))
            .map(|i| i + to_param.len() + 1)
            .unwrap_or(0);
    let ty_at = ty_offset
        + text[ty_offset..]
            .find(target_name.as_str())
            .context("the parameter's type is not in the list")?;
    let (tl, tc) = crate::signature::line_col_at(&text, ty_at);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    let answer = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/definition",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": tl - 1, "character": tc - 1 },
        }),
    )
    .await
    .with_context(|| format!("the analyzer does not say where `{target_name}` is declared"))?;
    let def = answer
        .as_array()
        .and_then(|a| a.first())
        .or_else(|| answer.get("uri").map(|_| &answer))
        .context("the analyzer does not say where the type is declared")?;
    let def_uri = def
        .get("uri")
        .or_else(|| def.get("targetUri"))
        .and_then(|u| u.as_str())
        .context("the type's declaration has no file")?;
    let def_line = def
        .pointer("/range/start/line")
        .or_else(|| def.pointer("/targetSelectionRange/start/line"))
        .and_then(|v| v.as_u64())
        .context("the type's declaration has no position")? as u32
        + 1;
    let target_file = canon(Path::new(&crate::remote_fs::uri_to_path(def_uri)));
    let (_, owner_module) = crate::move_item::module_of(file)?;
    let (_, target_module) = crate::move_item::module_of(&target_file)?;
    let owner_in_target = if owner_module.segments == target_module.segments {
        owner.clone()
    } else {
        format!(
            "{}::{owner}",
            owner_module.spelled_from(&target_module.krate)
        )
    };

    // The method as it will read in its new home.
    let old_as_type = old_as_type.replace("{owner}", &owner_in_target);
    let binding = if old_mut {
        format!("mut {new_param}")
    } else {
        new_param.clone()
    };
    let mut params: Vec<String> = vec![new_receiver.clone()];
    for (i, d) in declared.iter().enumerate() {
        if i == index {
            params.push(format!("{binding}: {old_as_type}"));
        } else {
            params.push(d.raw.trim().to_string());
        }
    }
    // The body's uses of the parameter, as the analyzer resolves them: another binding of the
    // same name (`if let Some(tax)`, `for tax in`, a match arm) keeps its own uses (#207).
    let param_at = open
        + list
            .match_indices(to_param)
            .map(|(i, _)| i)
            .find(|i| {
                !list[..*i].chars().next_back().is_some_and(is_ident)
                    && !list[i + to_param.len()..]
                        .chars()
                        .next()
                        .is_some_and(is_ident)
            })
            .context("the parameter's name is not in the list")?;
    let (pl, pc) = crate::signature::line_col_at(&text, param_at);
    let mut param_uses: Vec<usize> = crate::signature::references(remote, root, file, pl, pc)
        .await?
        .into_iter()
        .filter(|(path, _, _)| canon(path) == *file)
        .filter_map(|(_, l, c)| crate::signature::offset_of(&text, l, c))
        .filter(|at| body_open <= *at && *at <= body_close && text[*at..].starts_with(to_param))
        .map(|at| at - body_open)
        .collect();
    param_uses.sort_unstable();
    param_uses.dedup();
    let map = [
        ("self", new_param.as_str()),
        ("Self", owner_in_target.as_str()),
    ];
    let (item_start, span_end) = item_span(&text, name_at, body_close);
    // The blank line that separated it from the method above goes with it.
    let span_start = if text[..item_start].ends_with("\n\n") {
        item_start - 1
    } else {
        item_start
    };
    let head = &text[item_start..name_at];
    let generics = &text[name_at + name.len()..open - 1];
    let between = swap_names(&text[close + 1..body_open], &map);
    // The parameter's uses become a mark first, so the swap below does not see them, then `self`.
    // No letter in it, so the swap cannot read a name inside it.
    const MARK: &str = "\u{1}\u{2}\u{1}";
    let mut marked = body.to_string();
    for at in param_uses.iter().rev() {
        marked.replace_range(*at..*at + to_param.len(), MARK);
    }
    let new_body = swap_names(&marked, &map).replace(MARK, "self");
    let method_text = format!(
        "{head}{name}{generics}({}){between}{new_body}\n",
        params.join(", ")
    );
    let signature = format!(
        "fn {name}{generics}({}){}",
        params.join(", "),
        between.trim_end()
    );

    // Every file this touches, as edits against its text now.
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.clone(), text.clone());
    if !texts.contains_key(&target_file) {
        texts.insert(
            target_file.clone(),
            std::fs::read_to_string(&target_file)
                .with_context(|| format!("cannot read {}", target_file.display()))?,
        );
    }
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let (cut_start, cut_end) = cut_from_impl(
        &text,
        (impl_at, impl_open, impl_close),
        (span_start, span_end),
    );
    edits
        .entry(file.clone())
        .or_default()
        .push((cut_start, cut_end, String::new()));
    let target_text = texts[&target_file].clone();
    let (insert_at, inserted) = insertion(&target_text, &target_name, def_line, &method_text)?;
    edits
        .entry(target_file.clone())
        .or_default()
        .push((insert_at, insert_at, inserted));

    // The calls: the receiver and the argument swap places.
    let mut blocked = Vec::new();
    let mut calls = 0;
    let (nl, nc) = crate::signature::line_col_at(&text, name_at);
    for (path, l, c) in crate::signature::references(remote, root, file, nl, nc).await? {
        let path = canon(&path);
        if !texts.contains_key(&path) {
            let Ok(t) = std::fs::read_to_string(&path) else {
                continue;
            };
            texts.insert(path.clone(), t);
        }
        let body = texts[&path].clone();
        let site = format!("{}:{l}", display(root, &path));
        let Some(at) = crate::signature::offset_of(&body, l, c) else {
            continue;
        };
        if path == *file && span_start <= at && at < span_end {
            blocked.push(format!("{site}: `{name}` calls itself; not handled"));
            continue;
        }
        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(&body, at + name.len())
        else {
            blocked.push(format!(
                "{site}: `{name}` is used as a value, not called; its callers pass the receiver"
            ));
            continue;
        };
        let args = crate::parameter_object::split_args(&body[args_start..args_end]);
        let before = body[..at].trim_end();
        let close_paren = args_end + body[args_end..].find(')').unwrap_or(0) + 1;
        let (call_start, new_call) = if let Some(dot) = before.strip_suffix('.').map(|b| b.len()) {
            // `o.price_with(t, 1)` → `t.price_with(&o, 1)`.
            let recv_start = crate::encapsulate_field::chain_start(&body, dot);
            let recv = body[recv_start..dot].trim().to_string();
            let Some(arg) = args.get(index) else {
                blocked.push(format!(
                    "{site}: the call has fewer arguments than `{name}`"
                ));
                continue;
            };
            if crate::make_static::receiver_has_effects(&recv)
                || crate::make_static::receiver_has_effects(arg)
            {
                blocked.push(format!(
                    "{site}: `{recv}` and `{}` would be evaluated in the other order",
                    arg.trim()
                ));
                // `force` says the order does not matter: the call is still rewritten, or the
                // method would be gone from under it.
                if !force {
                    continue;
                }
            }
            let recv_expr = crate::to_method::receiver_of(&recv);
            let passed = match receiver.trim() {
                r if r.starts_with("&mut") || r.contains("mut self") && r.starts_with('&') => {
                    format!("&mut {recv_expr}")
                }
                r if r.starts_with('&') => format!("&{recv_expr}"),
                _ => recv_expr,
            };
            let mut new_args: Vec<String> = args.iter().map(|a| a.trim().to_string()).collect();
            new_args[index] = passed;
            (
                recv_start,
                format!(
                    "{}.{name}({})",
                    crate::to_method::receiver_of(arg),
                    new_args.join(", ")
                ),
            )
        } else if let Some(qualifier) = before.strip_suffix("::") {
            // `Order::price_with(o, t, 1)` → `Tax::price_with(t, o, 1)`.
            let path_start = qualifier
                .char_indices()
                .rev()
                .take_while(|(_, c)| is_ident(*c) || *c == ':')
                .last()
                .map_or(qualifier.len(), |(i, _)| i);
            if args.len() < index + 2 {
                blocked.push(format!(
                    "{site}: the call has fewer arguments than `{name}`"
                ));
                continue;
            }
            // The swap reorders the receiver's argument and the moved one: the same check as a
            // method call's (#207).
            if crate::make_static::receiver_has_effects(&args[0])
                || crate::make_static::receiver_has_effects(&args[index + 1])
            {
                blocked.push(format!(
                    "{site}: `{}` and `{}` would be evaluated in the other order",
                    args[0].trim(),
                    args[index + 1].trim()
                ));
                if !force {
                    continue;
                }
            }
            let mut new_args: Vec<String> = args.iter().map(|a| a.trim().to_string()).collect();
            new_args.swap(0, index + 1);
            let (_, caller_module) = crate::move_item::module_of(&path)?;
            let target_path = if caller_module.segments == target_module.segments {
                target_name.clone()
            } else {
                format!(
                    "{}::{target_name}",
                    target_module.spelled_from(&caller_module.krate)
                )
            };
            (
                path_start,
                format!("{target_path}::{name}({})", new_args.join(", ")),
            )
        } else {
            blocked.push(format!("{site}: neither a method call nor a path call"));
            continue;
        };
        edits
            .entry(path.clone())
            .or_default()
            .push((call_start, close_paren, new_call));
        calls += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut t = texts[&path].clone();
        file_edits.sort_by_key(|(from, _, _)| std::cmp::Reverse(*from));
        for (from, to, replacement) in file_edits {
            t.replace_range(from..to, &replacement);
        }
        rewritten.insert(path, t);
    }

    let diagnostics = errors_of(remote, root, &rewritten).await?;

    let mut applied = false;
    if apply && ((blocked.is_empty() && diagnostics.is_empty()) || force) {
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }
    Ok(MovedMethod {
        method: name,
        from_type: owner,
        to_type: target_name,
        root: root.clone(),
        signature,
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
    fn an_impl_left_empty_goes_with_its_last_item() {
        let t = "struct A;\n\nimpl A {\n    fn f() {}\n}\n\nfn g() {}\n";
        let impl_at = t.find("impl").unwrap();
        let open = t.find("{\n    fn").unwrap();
        let close = t.rfind("}\n\nfn g").unwrap();
        let span = (t.find("    fn f").unwrap(), t.find("}\n\nfn g").unwrap());
        let (s, e) = cut_from_impl(t, (impl_at, open, close), span);
        assert_eq!(
            format!("{}{}", &t[..s], &t[e..]),
            "struct A;\n\nfn g() {}\n"
        );
        let two = "impl A {\n    fn f() {}\n    fn h() {}\n}\n";
        let span = (two.find("    fn f").unwrap(), two.find("    fn h").unwrap());
        let (s, e) = cut_from_impl(
            two,
            (0, two.find('{').unwrap(), two.rfind('}').unwrap()),
            span,
        );
        assert_eq!((s, e), span);
    }

    #[test]
    fn a_function_goes_into_the_type_s_impl_or_a_new_one_after_it() {
        let with = "pub struct T;\n\nimpl T {\n    fn a() {}\n}\n";
        let (at, text) = insertion(with, "T", 1, "    fn b() {}\n").unwrap();
        let mut out = with.to_string();
        out.insert_str(at, &text);
        assert_eq!(
            out,
            "pub struct T;\n\nimpl T {\n    fn a() {}\n\n    fn b() {}\n}\n"
        );
        let empty = "pub struct T;\n\nimpl T {\n}\n";
        let (at, text) = insertion(empty, "T", 1, "    fn b() {}\n").unwrap();
        let mut out = empty.to_string();
        out.insert_str(at, &text);
        assert_eq!(out, "pub struct T;\n\nimpl T {\n    fn b() {}\n}\n");
        let none = "pub struct T {\n    x: u8,\n}\nfn f() {}\n";
        let (at, text) = insertion(none, "T", 1, "    fn b() {}\n").unwrap();
        let mut out = none.to_string();
        out.insert_str(at, &text);
        assert_eq!(
            out,
            "pub struct T {\n    x: u8,\n}\n\nimpl T {\n    fn b() {}\n}\nfn f() {}\n"
        );
    }

    #[test]
    fn names_swap_in_one_pass_and_fields_keep_theirs() {
        let body = "{ self.total + self.total * tax.rate + tax.tax + Self::zero().x }";
        let out = swap_names(
            body,
            &[("self", "order"), ("tax", "self"), ("Self", "crate::Order")],
        );
        assert_eq!(
            out,
            "{ order.total + order.total * self.rate + self.tax + crate::Order::zero().x }"
        );
        assert_eq!(
            swap_names("taxes + mytax", &[("tax", "self")]),
            "taxes + mytax"
        );
        assert_eq!(swap_names("a..tax", &[("tax", "self")]), "a..self");
    }

    #[test]
    fn a_receiver_becomes_the_type_it_borrowed() {
        assert_eq!(receiver_as_type("&self", "O"), Some(("&O".into(), false)));
        assert_eq!(
            receiver_as_type("&mut self", "O"),
            Some(("&mut O".into(), false))
        );
        assert_eq!(
            receiver_as_type("&'a self", "O"),
            Some(("&'a O".into(), false))
        );
        assert_eq!(receiver_as_type("self", "O"), Some(("O".into(), false)));
        assert_eq!(receiver_as_type("mut self", "O"), Some(("O".into(), true)));
        assert_eq!(receiver_as_type("self: Box<Self>", "O"), None);
        assert_eq!(snake_case("LineItem"), "line_item");
        assert_eq!(snake_case("Order"), "order");
        assert_eq!(base_name("crate::m::Wrapper<T>"), "Wrapper");
    }

    #[test]
    fn an_item_takes_its_doc_comment_and_its_last_line() {
        let t =
            "impl O {\n    /// Doc.\n    #[inline]\n    pub fn f(&self) {\n        1;\n    }\n}\n";
        let name_at = t.find("f(").unwrap();
        let close = t.find("    }").unwrap() + 4;
        let (s, e) = item_span(t, name_at, close);
        assert_eq!(
            &t[s..e],
            "    /// Doc.\n    #[inline]\n    pub fn f(&self) {\n        1;\n    }\n"
        );
    }
}
