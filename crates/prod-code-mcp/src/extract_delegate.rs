//! Extracting a delegate: some fields of a struct and the methods that work only on them move
//! into a new helper type, which the struct then holds (Extract Class).
//!
//! rust-analyzer offers delegates for one field at a time (`generate_delegate_methods`,
//! `generate_delegate_trait`); nothing moves a group of fields with their behaviour. Here:
//! - the fields named leave the struct, and one field of the new type takes their place;
//! - the methods named move to `impl Helper`, and the struct keeps a method of the same
//!   signature that forwards to it, so no caller changes;
//! - every other access to a moved field goes through the new field (`a.city` becomes
//!   `a.address.city`), found through the analyzer's references;
//! - every struct literal of the type builds the helper (`Account { city, .. }` becomes
//!   `Account { address: Address { city }, .. }`).
//!
//! A moved method may use only the moved fields and the other moved methods. The whole change is
//! type-checked before anything is written.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Top-level pieces of `text` separated by `sep`, as byte ranges, outside every bracket.
pub fn split_top(text: &str, sep: char) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut prev = ' ';
    for (i, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '<' => depth += 1,
            '>' if prev != '-' && prev != '=' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push((start, i));
                start = i + c.len_utf8();
            }
            _ => {}
        }
        prev = c;
    }
    if !text[start..].trim().is_empty() {
        out.push((start, text.len()));
    }
    out
}

/// One field of a struct: its text from its first attribute or doc line to its comma.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub vis: String,
    /// The field's text without the trailing comma, first line trimmed of its indentation.
    pub text: String,
}

/// A named struct: where its declaration (attributes included) starts, its braces and fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructDecl {
    pub name: String,
    pub vis: String,
    pub start: usize,
    pub open: usize,
    pub close: usize,
    pub derive: Option<String>,
    pub fields: Vec<Field>,
}

/// The struct whose `struct` keyword is on the line of `at`.
pub fn parse_struct(text: &str, at: usize) -> Result<StructDecl> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let kw = text[line_start..line_end]
        .find("struct ")
        .map(|i| line_start + i)
        .context("no `struct` on this line")?;
    let vis = text[line_start..kw].trim().to_string();
    let name: String = text[kw + 7..]
        .trim_start()
        .chars()
        .take_while(|c| is_ident(*c))
        .collect();
    anyhow::ensure!(!name.is_empty(), "the struct has no name");
    let after_name = kw + 7 + text[kw + 7..].find(&name).unwrap_or(0) + name.len();
    let rest = text[after_name..].trim_start();
    anyhow::ensure!(
        rest.starts_with('{'),
        "`{name}` is not a plain struct with named fields (generic and tuple structs are not supported)"
    );
    let open = after_name + (text[after_name..].len() - rest.len());
    let close = crate::parameter_object::matching_bracket(text, open)
        .context("the struct is not closed")?;
    // The attributes and doc comments above belong to the declaration.
    let mut start = line_start;
    let mut derive = None;
    loop {
        let above = text[..start].trim_end_matches('\n');
        let prev_start = above.rfind('\n').map_or(0, |i| i + 1);
        let prev = above[prev_start..].trim();
        if start == 0 || !(prev.starts_with("#[") || prev.starts_with("///")) {
            break;
        }
        if prev.starts_with("#[derive(") {
            derive = Some(prev.to_string());
        }
        start = prev_start;
    }
    let body = &text[open + 1..close];
    let mut fields = Vec::new();
    for (s, e) in split_top(body, ',') {
        let chunk = body[s..e].trim();
        let decl = crate::extract_trait::declaration(chunk).1;
        let (fvis, bare) = crate::extract_trait::visibility(decl);
        let Some((fname, _)) = bare.split_once(':') else {
            continue;
        };
        fields.push(Field {
            name: fname.trim().to_string(),
            vis: fvis.to_string(),
            text: chunk.to_string(),
        });
    }
    Ok(StructDecl {
        name,
        vis,
        start,
        open,
        close,
        derive,
        fields,
    })
}

/// The inherent `impl Name` blocks of `text`, as (start of `impl`, open, close).
pub fn impl_blocks(text: &str, name: &str) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices("impl ") {
        if text[..i].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let Some(open) = text[i..].find('{').map(|o| i + o) else {
            continue;
        };
        let header = text[i + 5..open].trim();
        if header == name
            && let Some(close) = crate::parameter_object::matching_bracket(text, open)
        {
            out.push((i, open, close));
        }
    }
    out
}

/// The parameter names of a signature, `self` left out: `(&self, a: u32, mut b: T)` gives `a, b`.
pub fn argument_names(sig: &str) -> Option<Vec<String>> {
    let open = sig.find('(')?;
    let close = crate::parameter_object::matching_bracket(sig, open)?;
    let mut out = Vec::new();
    for (s, e) in split_top(&sig[open + 1..close], ',') {
        let p = sig[open + 1 + s..open + 1 + e].trim();
        if p.is_empty() || p.ends_with("self") {
            continue;
        }
        let pat = p.split_once(':')?.0.trim();
        let pat = pat.strip_prefix("mut ").unwrap_or(pat);
        if !pat.chars().all(is_ident) {
            return None;
        }
        out.push(pat.to_string());
    }
    Some(out)
}

/// Rewrites every struct literal `Name { .. }` (and `Self { .. }` inside `self_ranges`) in
/// `text` whose entries include one of `moved`: those entries go into `field: Helper { .. }`.
pub fn rewrite_literals(
    text: &str,
    name: &str,
    self_ranges: &[(usize, usize)],
    moved: &[String],
    field: &str,
    helper: &str,
) -> Result<String> {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for word in [name, "Self"] {
        for (i, _) in text.match_indices(word) {
            let before = text[..i].trim_end();
            if text[..i].chars().next_back().is_some_and(is_ident)
                || text[i + word.len()..].chars().next().is_some_and(is_ident)
                || before.ends_with("struct")
                || before.ends_with("impl")
                || before.ends_with("for")
                || before.ends_with("->")
                || (word == "Self" && !self_ranges.iter().any(|(s, e)| *s < i && i < *e))
            {
                continue;
            }
            let rest = &text[i + word.len()..];
            let trimmed = rest.trim_start();
            if !trimmed.starts_with('{') {
                continue;
            }
            let open = i + word.len() + (rest.len() - trimmed.len());
            let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
                continue;
            };
            let body = &text[open + 1..close];
            let entries: Vec<&str> = split_top(body, ',')
                .into_iter()
                .map(|(s, e)| body[s..e].trim())
                .filter(|e| !e.is_empty())
                .collect();
            let key = |e: &str| -> String { e.split(':').next().unwrap_or(e).trim().to_string() };
            let inner: Vec<&str> = entries
                .iter()
                .copied()
                .filter(|e| moved.contains(&key(e)))
                .collect();
            if inner.is_empty() {
                continue;
            }
            anyhow::ensure!(
                !entries.iter().any(|e| e.starts_with("..")),
                "a literal or pattern of `{name}` uses `..`; rewrite it by hand"
            );
            let indent: String = body
                .trim_start_matches(['\n'])
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let nested = format!("{field}: {helper} {{ {} }}", inner.join(", "));
            let mut all: Vec<String> = Vec::new();
            let mut placed = false;
            for e in &entries {
                if moved.contains(&key(e)) {
                    if !placed {
                        all.push(nested.clone());
                        placed = true;
                    }
                } else {
                    all.push(e.to_string());
                }
            }
            let new_body = if body.contains('\n') {
                let close_indent: String = text[..close]
                    .rsplit('\n')
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                format!(
                    "\n{indent}{},\n{close_indent}",
                    all.join(&format!(",\n{indent}"))
                )
            } else {
                format!(" {} ", all.join(", "))
            };
            edits.push((open + 1, close, new_body));
        }
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = text.to_string();
    for (s, e, t) in edits {
        out.replace_range(s..e, &t);
    }
    Ok(out)
}

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Extracted {
    pub helper: String,
    pub field: String,
    pub fields: Vec<String>,
    pub methods: Vec<String>,
    #[serde(skip)]
    pub root: PathBuf,
    pub rewritten: Vec<(String, String)>,
    pub accesses: usize,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Extracted {
    pub fn render(&self) -> String {
        let mut out = format!(
            "`{}: {}` holds {}; {} method(s) moved, {} access(es) rerouted\n",
            self.field,
            self.helper,
            self.fields.join(", "),
            self.methods.len(),
            self.accesses
        );
        for (path, new_text) in &self.rewritten {
            let full = Path::new(path);
            let old_text = if self.applied {
                crate::refactor::text_before_apply(full)
            } else {
                std::fs::read_to_string(full).unwrap_or_default()
            };
            let rel = full
                .strip_prefix(&self.root)
                .unwrap_or(full)
                .display()
                .to_string();
            out.push('\n');
            out.push_str(
                &similar::TextDiff::from_lines(old_text.as_str(), new_text.as_str())
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
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
            "\nnothing was written; pass `apply: true` to make this edit\n"
        });
        out
    }
}

/// The struct and impl changes in the declaring file, once every outside access already goes
/// through `field`: the fields leave the struct, the helper type is declared after it, and the
/// methods move behind forwarding methods.
pub fn restructure(
    text: &str,
    at: usize,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
) -> Result<String> {
    let decl = parse_struct(text, at)?;
    for f in fields {
        anyhow::ensure!(
            decl.fields.iter().any(|d| &d.name == f),
            "`{}` has no field `{f}`; it has {}",
            decl.name,
            decl.fields
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    anyhow::ensure!(
        decl.fields.iter().all(|d| d.name != field),
        "`{}` already has a field `{field}`",
        decl.name
    );
    let moved_fields: Vec<&Field> = decl
        .fields
        .iter()
        .filter(|f| fields.contains(&f.name))
        .collect();
    let widest = if moved_fields.iter().any(|f| f.vis == "pub") {
        "pub"
    } else {
        moved_fields
            .iter()
            .map(|f| f.vis.as_str())
            .find(|v| !v.is_empty())
            .unwrap_or("")
    };
    let indent = "    ";
    let mut kept: Vec<String> = Vec::new();
    let mut placed = false;
    for f in &decl.fields {
        if fields.contains(&f.name) {
            if !placed {
                let v = if widest.is_empty() {
                    String::new()
                } else {
                    format!("{widest} ")
                };
                kept.push(format!("{indent}{v}{field}: {helper},"));
                placed = true;
            }
        } else {
            kept.push(format!("{indent}{},", f.text));
        }
    }
    let struct_vis = if decl.vis.is_empty() {
        String::new()
    } else {
        format!("{} ", decl.vis)
    };
    let new_struct_body = format!("{{\n{}\n}}", kept.join("\n"));
    let helper_decl = format!(
        "{}{struct_vis}struct {helper} {{\n{}\n}}\n",
        decl.derive
            .as_ref()
            .map(|d| format!("{d}\n"))
            .unwrap_or_default(),
        moved_fields
            .iter()
            .map(|f| format!("{indent}{},", f.text))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // Methods: taken out of every `impl Name` block, a forwarding method left in their place.
    let mut out = text.to_string();
    let mut moved_items: Vec<String> = Vec::new();
    let mut found: Vec<String> = Vec::new();
    for (_, open, close) in impl_blocks(text, &decl.name).into_iter().rev() {
        let items = crate::extract_trait::items(text, open, close);
        for item in items.iter().rev() {
            let Some(mname) = item.name.as_ref().filter(|n| methods.contains(n)) else {
                continue;
            };
            let chunk = &text[item.start..item.end];
            let (leading, decl_text) = crate::extract_trait::declaration(chunk);
            let sig = crate::extract_trait::signature(decl_text)
                .with_context(|| format!("cannot read the signature of `{mname}`"))?;
            anyhow::ensure!(
                sig.contains("&self") || sig.contains("&mut self"),
                "`{mname}` does not take `&self` or `&mut self`; only methods on a borrowed \
                 receiver can forward to the helper"
            );
            // It may use only the moved fields and the other moved methods.
            let body = &decl_text[sig.len()..];
            for (i, _) in body.match_indices("self.") {
                let used: String = body[i + 5..].chars().take_while(|c| is_ident(*c)).collect();
                anyhow::ensure!(
                    fields.contains(&used) || methods.contains(&used),
                    "`{mname}` uses `self.{used}`, which does not move to `{helper}`"
                );
            }
            let names = argument_names(sig).with_context(|| {
                format!("`{mname}` has a parameter pattern this cannot forward")
            })?;
            let item_indent: String = text[..item.start]
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let attrs: String = leading
                .iter()
                .map(|l| format!("{l}\n{item_indent}"))
                .collect();
            let stub = format!(
                "{attrs}{sig} {{\n{item_indent}    self.{field}.{mname}({})\n{item_indent}}}",
                names.join(", ")
            );
            out.replace_range(item.start..item.end, &stub);
            moved_items.push(format!("{item_indent}{}", chunk.trim()));
            found.push(mname.clone());
        }
    }
    for m in methods {
        anyhow::ensure!(
            found.contains(m),
            "`{}` has no method `{m}` in an inherent `impl` block",
            decl.name
        );
    }
    moved_items.reverse();

    // The struct itself: rewritten last, it comes before every impl block in the offsets above
    // only if it is declared first; find it again in the text as it is now.
    let decl_now = parse_struct(
        &out,
        out.find(&format!("struct {} ", decl.name)).unwrap_or(at),
    )?;
    let helper_impl = if moved_items.is_empty() {
        String::new()
    } else {
        format!("\nimpl {helper} {{\n{}\n}}\n", moved_items.join("\n\n"))
    };
    out.replace_range(
        decl_now.open..decl_now.close + 1,
        format!("{new_struct_body}\n\n{helper_decl}{helper_impl}").trim_end(),
    );
    Ok(out)
}

/// Extracts `fields` and `methods` of the struct at `line`:`col` of `file` into `helper`, held
/// in the new field `field`.
#[allow(clippy::too_many_arguments)]
pub async fn extract_delegate(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
    apply: bool,
    force: bool,
) -> Result<Extracted> {
    for n in [helper, field] {
        anyhow::ensure!(
            !n.is_empty() && n.chars().all(is_ident),
            "`{n}` is not an identifier"
        );
    }
    anyhow::ensure!(!fields.is_empty(), "name at least one field");
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at =
        crate::signature::offset_of(&text, line, col).context("the position is not in the file")?;
    let decl = parse_struct(&text, at)?;
    // Check the struct side first, so a wrong name fails before any analyzer query.
    restructure(&text, at, fields, methods, helper, field)?;

    // The moved methods keep `self.city`; every other access gets the new field in front.
    let moved_ranges: Vec<(usize, usize)> = impl_blocks(&text, &decl.name)
        .into_iter()
        .flat_map(|(_, open, close)| crate::extract_trait::items(&text, open, close))
        .filter(|i| i.name.as_ref().is_some_and(|n| methods.contains(n)))
        .map(|i| (i.start, i.end))
        .collect();
    let mut inserts: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    let body = &text[decl.open + 1..decl.close];
    for f in fields {
        let Some(rel_at) = body.match_indices(f.as_str()).map(|(i, _)| i).find(|i| {
            !body[..*i].chars().next_back().is_some_and(is_ident)
                && body[i + f.len()..].trim_start().starts_with(':')
        }) else {
            continue;
        };
        let (fl, fc) = crate::signature::line_col_at(&text, decl.open + 1 + rel_at);
        for (path, rl, rc) in crate::signature::references(remote, root, file, fl, fc).await? {
            let other = if path == file {
                text.clone()
            } else {
                std::fs::read_to_string(&path).unwrap_or_default()
            };
            let Some(off) = crate::signature::offset_of(&other, rl, rc) else {
                continue;
            };
            if path == file && moved_ranges.iter().any(|(s, e)| *s <= off && off < *e) {
                continue;
            }
            // A field access (`a.city`); a literal or pattern entry is left to the literal pass.
            if other[..off].ends_with('.') {
                inserts.entry(path).or_default().push(off);
            }
        }
    }
    let accesses: usize = inserts.values().map(|v| v.len()).sum();
    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    files.insert(file.to_path_buf(), text.clone());
    for (path, mut offs) in inserts {
        let mut t = match files.get(&path) {
            Some(t) => t.clone(),
            None => std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?,
        };
        offs.sort_unstable();
        offs.dedup();
        for off in offs.into_iter().rev() {
            t.insert_str(off, &format!("{field}."));
        }
        files.insert(path, t);
    }
    // The declaring file: the struct, the helper and the methods.
    let declaring = files.get(file).cloned().unwrap_or_default();
    let at_now = declaring
        .find(&format!("struct {} ", decl.name))
        .unwrap_or(at);
    let restructured = restructure(&declaring, at_now, fields, methods, helper, field)?;
    files.insert(file.to_path_buf(), restructured);
    // Struct literals, in every file that touches the fields.
    let paths: Vec<PathBuf> = files.keys().cloned().collect();
    for path in paths {
        let t = files[&path].clone();
        let self_ranges: Vec<(usize, usize)> = impl_blocks(&t, &decl.name)
            .into_iter()
            .map(|(s, _, e)| (s, e))
            .collect();
        let rewritten = rewrite_literals(&t, &decl.name, &self_ranges, fields, field, helper)?;
        files.insert(path, rewritten);
    }
    files.retain(|p, t| std::fs::read_to_string(p).map(|o| o != *t).unwrap_or(true));

    let edits: Vec<(PathBuf, String)> = files.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
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
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }
    Ok(Extracted {
        helper: helper.to_string(),
        field: field.to_string(),
        fields: fields.to_vec(),
        methods: methods.to_vec(),
        root: root.to_path_buf(),
        rewritten: files
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        accesses,
        diagnostics,
        applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT: &str = "#[derive(Debug, Clone)]\npub struct Account {\n    pub owner: String,\n    pub street: String,\n    pub city: String,\n    balance: u64,\n}\n\nimpl Account {\n    pub fn new(owner: &str, street: &str, city: &str) -> Self {\n        Account {\n            owner: owner.into(),\n            street: street.into(),\n            city: city.into(),\n            balance: 0,\n        }\n    }\n\n    pub fn address(&self) -> String {\n        format!(\"{}, {}\", self.street, self.city)\n    }\n\n    pub fn moves_to(&mut self, city: &str) {\n        self.city = city.to_string();\n    }\n\n    pub fn deposit(&mut self, n: u64) {\n        self.balance += n;\n    }\n}\n";

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_struct_and_its_fields_are_read_with_their_attributes() {
        let d = parse_struct(ACCOUNT, ACCOUNT.find("pub struct").unwrap()).unwrap();
        assert_eq!(d.name, "Account");
        assert_eq!(d.derive.as_deref(), Some("#[derive(Debug, Clone)]"));
        let names: Vec<&str> = d.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["owner", "street", "city", "balance"]);
        assert_eq!(split_top("a: HashMap<K, V>, b: (u8, u8)", ',').len(), 2);
        assert_eq!(
            argument_names("pub fn f(&mut self, a: u32, mut b: Vec<(u8, u8)>) -> u8"),
            Some(strings(&["a", "b"]))
        );
        assert_eq!(argument_names("fn g(&self, (a, b): (u8, u8))"), None);
    }

    #[test]
    fn the_fields_and_methods_move_behind_a_forwarding_method() {
        let at = ACCOUNT.find("pub struct").unwrap();
        let fields = strings(&["street", "city"]);
        let methods = strings(&["address", "moves_to"]);
        let out = restructure(ACCOUNT, at, &fields, &methods, "Address", "address").unwrap();
        for expected in [
            "pub struct Account {\n    pub owner: String,\n    pub address: Address,\n    balance: u64,\n}",
            "#[derive(Debug, Clone)]\npub struct Address {\n    pub street: String,\n    pub city: String,\n}",
            "impl Address {\n    pub fn address(&self) -> String {\n        format!(\"{}, {}\", self.street, self.city)\n    }",
            "    pub fn moves_to(&mut self, city: &str) {\n        self.address.moves_to(city)\n    }",
            "    pub fn deposit(&mut self, n: u64) {\n        self.balance += n;\n    }",
        ] {
            assert!(out.contains(expected), "{expected}\n---\n{out}");
        }
        let blocks = impl_blocks(&out, "Account");
        let ranges: Vec<(usize, usize)> = blocks.iter().map(|(s, _, e)| (*s, *e)).collect();
        let lit =
            rewrite_literals(&out, "Account", &ranges, &fields, "address", "Address").unwrap();
        assert!(
            lit.contains("            owner: owner.into(),\n            address: Address { street: street.into(), city: city.into() },\n            balance: 0,\n"),
            "{lit}"
        );

        // A moved method that uses a field that stays is refused, and so is an unknown name.
        let err = restructure(
            ACCOUNT,
            at,
            &fields,
            &strings(&["deposit"]),
            "Address",
            "address",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("uses `self.balance`"), "{err}");
        let err =
            restructure(ACCOUNT, at, &strings(&["zip"]), &[], "Address", "address").unwrap_err();
        assert!(format!("{err}").contains("has no field `zip`"), "{err}");
        let err = rewrite_literals(
            "fn f() { let Account { city, .. } = a; }",
            "Account",
            &[],
            &fields,
            "address",
            "Address",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("uses `..`"), "{err}");

        // A tuple or generic struct, and a method that does not borrow its receiver.
        for text in ["pub struct P(u8);\n", "pub struct G<T> {\n    t: T,\n}\n"] {
            let err = parse_struct(text, 0).unwrap_err();
            assert!(format!("{err}").contains("not a plain struct"), "{err}");
        }
        let by_value = ACCOUNT.replace(
            "    pub fn moves_to(&mut self, city: &str) {",
            "    pub fn moves_to(mut self, city: &str) {",
        );
        let err = restructure(&by_value, at, &fields, &methods, "Address", "address").unwrap_err();
        assert!(format!("{err}").contains("does not take `&self`"), "{err}");
    }
}
