//! Making a public field private, and every access to it outside its module a method call.
//!
//! rust-analyzer generates the getter and the setter and changes the visibility, one assist at a
//! time; what it does not do is the part that takes an afternoon — finding every `x.field` in
//! the workspace and turning it into `x.field()`, and every `x.field = v` into `x.set_field(v)`.
//! That is what this does. Accesses inside the file that declares the field are left as they
//! are, because a private field is still visible there; a use that cannot be a method call — a
//! struct literal, a pattern, `+=`, `&mut x.field` — is reported, not rewritten, and nothing is
//! written while one remains.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What the encapsulation did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EncapsulatedField {
    /// The struct the field belongs to.
    pub owner: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub field: String,
    pub ty: String,
    /// Whether the getter returns the value (a `Copy` type) or a reference to it.
    pub by_value: bool,
    pub reads: usize,
    pub writes: usize,
    /// Reads that go on to call a method on the field or index it: if one of those mutates it,
    /// a getter that returns `&T` does not compile, and only the compiler says so.
    pub chained_reads: usize,
    /// References inside the declaring file, which stay direct accesses.
    pub left_in_file: usize,
    /// Uses that cannot become a method call, with the reason.
    pub blocked: Vec<String>,
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl EncapsulatedField {
    /// The report: the accessors, the rewritten accesses, and what could not be rewritten.
    pub fn render(&self, diff_budget: usize) -> String {
        let getter = if self.by_value {
            self.ty.clone()
        } else {
            format!("&{}", self.ty)
        };
        let mut out = format!(
            "`{}.{}` ({})\n\n- the field becomes private\n- getter: `fn {}(&self) -> {getter}`\n",
            self.owner, self.field, self.file, self.field
        );
        if self.writes > 0 {
            out.push_str(&format!(
                "- setter: `fn set_{}(&mut self, {}: {})`\n",
                self.field, self.field, self.ty
            ));
        }
        out.push_str(&format!(
            "- {} read(s) and {} write(s) outside {} rewritten; {} reference(s) inside it left \
             as they are, because a private field is still visible there\n\n",
            self.reads, self.writes, self.file, self.left_in_file
        ));
        let mut body = String::new();
        let mut changed_lines = 0usize;
        for (path, new_text) in &self.rewritten {
            let old_text = std::fs::read_to_string(path).unwrap_or_default();
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
                "\n{} use(s) outside the declaring file cannot become a method call, and a \
                 private field would not compile there:\n",
                self.blocked.len()
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) this could not read as a field access):\n",
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
        if self.chained_reads > 0 && !self.by_value {
            out.push_str(&format!(
                "\n{} read(s) call a method on the field or index it. The getter returns a \
                 shared reference, so one that mutates the field no longer compiles — and the \
                 analyzer does not check borrows. Ask for `verify: \"compile\"` to be sure.\n",
                self.chained_reads
            ));
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

/// A named field's declaration, read from the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    pub name: String,
    /// Where the name starts.
    pub name_at: usize,
    /// The visibility as written, with its trailing space: `pub `, `pub(crate) `, or empty.
    pub vis: String,
    /// Where the visibility starts; it runs up to `name_at`.
    pub vis_at: usize,
    pub ty: String,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The field declared at `offset`, which may be anywhere in its name.
pub fn field_at(text: &str, offset: usize) -> Result<FieldDecl> {
    let offset = offset.min(text.len());
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_ident(*c))
        .map_or(text.len(), |(i, _)| offset + i);
    let name = &text[start..end];
    anyhow::ensure!(!name.is_empty(), "there is no name at this position");
    let after = text[end..].trim_start();
    anyhow::ensure!(
        after.starts_with(':') && !after.starts_with("::"),
        "`{name}` here is not a field declaration: a field is `name: Type` inside a struct"
    );
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &text[line_start..start];
    let vis_at = line_start + (prefix.len() - prefix.trim_start().len());
    let vis = &text[vis_at..start];
    let vis_word = vis.trim_end();
    anyhow::ensure!(
        vis_word.is_empty() || vis_word == "pub" || vis_word.starts_with("pub("),
        "`{name}` here is not a field declaration: `{}` comes before it",
        vis_word
    );
    // The type runs to the comma that ends the field, or the brace that ends the struct.
    let ty_from = text.len() - after.len() + 1;
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut i = ty_from;
    while i < bytes.len() {
        match bytes[i] {
            b'-' if bytes.get(i + 1) == Some(&b'>') => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') && depth == 0 => break,
            b'<' | b'(' | b'[' | b'{' => depth += 1,
            b'>' | b')' | b']' | b'}' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            b',' if depth == 0 => break,
            _ => {}
        }
        i += 1;
    }
    let ty = text[ty_from..i.min(text.len())].trim().to_string();
    anyhow::ensure!(!ty.is_empty(), "`{name}` has no type after its colon");
    Ok(FieldDecl {
        name: name.to_string(),
        name_at: start,
        vis: vis.to_string(),
        vis_at,
        ty,
    })
}

/// The struct whose braces contain `offset`: its name, where `struct` is, and its closing brace.
pub fn owner_at(text: &str, offset: usize) -> Option<(String, usize, usize)> {
    let mut best: Option<(String, usize, usize)> = None;
    for (at, _) in text[..offset.min(text.len())].match_indices("struct ") {
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let rest = &text[at + "struct ".len()..];
        let name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
        if name.is_empty() {
            continue;
        }
        let Some(open) = text[at..].find(['{', ';', '(']).map(|i| at + i) else {
            continue;
        };
        if text.as_bytes()[open] != b'{' {
            continue;
        }
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        if open < offset && offset < close {
            best = Some((name, at, close));
        }
    }
    best
}

/// Whether the struct at `struct_at` takes type or lifetime parameters.
pub fn is_generic(text: &str, struct_at: usize, owner: &str) -> bool {
    text[struct_at..]
        .strip_prefix("struct ")
        .and_then(|r| r.strip_prefix(owner))
        .is_some_and(|r| r.trim_start().starts_with('<'))
}

/// The opening brace of the first inherent `impl` of `owner` in `text` (not a trait impl).
pub fn inherent_impl(text: &str, owner: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let here = offset;
        offset += line.len();
        let trimmed = line.trim_start();
        let Some(mut rest) = trimmed.strip_prefix("impl") else {
            continue;
        };
        if rest.starts_with('<') {
            let mut depth = 0i32;
            let mut end = rest.len();
            for (i, c) in rest.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = &rest[end..];
        } else if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let Some(after) = rest.strip_prefix(owner) else {
            continue;
        };
        if after.starts_with(is_ident) {
            continue;
        }
        let header_end = text[here..].find('{').map(|i| here + i)?;
        if text[here..header_end].contains(" for ") {
            continue;
        }
        return Some(header_end);
    }
    None
}

/// Whether a getter for `ty` should return the value rather than a reference: the primitive
/// `Copy` types, shared references, and an `Option` of either.
pub fn returns_by_value(ty: &str) -> bool {
    const COPY: &[&str] = &[
        "bool", "char", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
        "i128", "isize", "f32", "f64",
    ];
    let ty = ty.trim();
    if let Some(inner) = ty.strip_prefix("Option<").and_then(|r| r.strip_suffix('>')) {
        return returns_by_value(inner);
    }
    COPY.contains(&ty) || (ty.starts_with('&') && !ty.starts_with("&mut"))
}

/// How a reference to the field is used, read from the text around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// `x.f`, possibly continuing with `.method()` or `[i]` (`chained`).
    Read { chained: bool },
    /// `x.f = rhs`, with the right-hand side's span.
    Write { rhs: (usize, usize) },
    /// Anything this cannot turn into a method call, and why.
    Blocked(&'static str),
    /// Not a field access at all: a method with the same name.
    NotAccess,
}

/// What the reference to a field of length `len` at `at` does.
pub fn access_at(text: &str, at: usize, len: usize) -> Access {
    let before = text[..at].trim_end();
    if !before.ends_with('.') || before.ends_with("..") {
        return Access::Blocked("a struct literal or pattern names the field");
    }
    let rest = text[at + len..].trim_start();
    if rest.starts_with('(') {
        return Access::NotAccess;
    }
    for op in ["<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "|=", "&=", "^="] {
        if rest.starts_with(op) {
            return Access::Blocked("a compound assignment needs both the getter and the setter");
        }
    }
    if rest.starts_with('=') && !rest.starts_with("==") && !rest.starts_with("=>") {
        let rhs_start = text.len() - rest.len() + 1;
        return Access::Write {
            rhs: (rhs_start, expression_end(text, rhs_start)),
        };
    }
    let start = chain_start(text, before.len() - 1);
    let lead = text[..start].trim_end();
    if lead.ends_with("&mut")
        && !lead[..lead.len() - 4]
            .chars()
            .next_back()
            .is_some_and(is_ident)
    {
        return Access::Blocked("a mutable borrow of the field");
    }
    Access::Read {
        chained: rest.starts_with('.') || rest.starts_with('['),
    }
}

/// Where the expression starting at `from` ends: the `;`, `,` or closing bracket that is not
/// inside it.
fn expression_end(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => {
                match crate::parameter_object::matching_bracket(text, i) {
                    Some(close) => i = close + 1,
                    None => return bytes.len(),
                }
                continue;
            }
            b'"' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                i = j + 1;
                continue;
            }
            b'\'' if bytes.get(i + 2) == Some(&b'\'') => {
                i += 3;
                continue;
            }
            b';' | b',' | b')' | b']' | b'}' => return i,
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// Where the receiver of the `.` at `dot` starts: `a.b().c[0]` for the dot before a field.
pub(crate) fn chain_start(text: &str, dot: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = dot;
    loop {
        while i > 0 && (bytes[i - 1] as char).is_whitespace() {
            i -= 1;
        }
        if i > 0 && matches!(bytes[i - 1], b')' | b']') {
            let mut depth = 0i32;
            let mut j = i;
            while j > 0 {
                j -= 1;
                match bytes[j] {
                    b')' | b']' => depth += 1,
                    b'(' | b'[' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            i = j;
            continue;
        }
        if i > 0 && bytes[i - 1] == b'?' {
            i -= 1;
            continue;
        }
        let ident_end = i;
        while i > 0 && is_ident(bytes[i - 1] as char) {
            i -= 1;
        }
        if i == ident_end {
            return i;
        }
        if i > 0 && bytes[i - 1] == b'.' && !(i > 1 && bytes[i - 2] == b'.') {
            i -= 1;
            continue;
        }
        if i > 1 && &text[i - 2..i] == "::" {
            i -= 2;
            continue;
        }
        return i;
    }
}

/// The accessor methods, indented to sit inside an `impl` block at `indent`.
pub fn accessors(
    indent: &str,
    vis: &str,
    name: &str,
    ty: &str,
    by_value: bool,
    setter: bool,
) -> String {
    let (ret, body) = if by_value {
        (ty.to_string(), format!("self.{name}"))
    } else {
        (format!("&{ty}"), format!("&self.{name}"))
    };
    let mut out =
        format!("{indent}{vis}fn {name}(&self) -> {ret} {{\n{indent}    {body}\n{indent}}}\n");
    if setter {
        out.push_str(&format!(
            "\n{indent}{vis}fn set_{name}(&mut self, {name}: {ty}) {{\n{indent}    self.{name} \
             = {name};\n{indent}}}\n"
        ));
    }
    out
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Makes the field declared at `line`:`col` of `file` private and rewrites every access to it
/// outside that file into a call of its getter or setter.
#[allow(clippy::too_many_arguments)]
pub async fn encapsulate(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    by_value: Option<bool>,
    apply: bool,
    force: bool,
) -> Result<EncapsulatedField> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset = crate::signature::offset_of(&text, line, col)
        .context("the position is not inside the file")?;
    let decl = field_at(&text, offset)?;
    let field = decl.name.clone();
    anyhow::ensure!(
        !decl.vis.trim().is_empty(),
        "`{field}` is already private; there is nothing outside its module to rewrite"
    );
    let (owner, struct_at, struct_close) = owner_at(&text, decl.name_at)
        .with_context(|| format!("`{field}` is not a field of a struct with named fields"))?;
    let by_value = by_value.unwrap_or_else(|| returns_by_value(&decl.ty));

    // Every reference outside the declaring file, classified by what it does there.
    let (name_line, name_col) = crate::signature::line_col_at(&text, decl.name_at);
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let (mut reads, mut writes, mut chained_reads, mut left_in_file) = (0, 0, 0, 0);
    let mut blocked = Vec::new();
    let mut unmatched = Vec::new();
    for (path, rl, rc) in crate::signature::references(remote, root, file, name_line, name_col)
        .await
        .unwrap_or_default()
    {
        if path == *file {
            left_in_file += 1;
            continue;
        }
        let body = texts
            .entry(path.clone())
            .or_insert_with(|| std::fs::read_to_string(&path).unwrap_or_default());
        let at_site = format!("{}:{rl}:{rc}", display(root, &path));
        let Some(at) = crate::signature::offset_of(body, rl, rc) else {
            unmatched.push(format!("{at_site} (the position is not in the file)"));
            continue;
        };
        // The analyzer's position is trusted only when the name is actually there (#75).
        if !body[at..].starts_with(field.as_str()) || body[at + field.len()..].starts_with(is_ident)
        {
            unmatched.push(format!(
                "{at_site} (the analyzer places `{field}` here, but the file says otherwise)"
            ));
            continue;
        }
        match access_at(body, at, field.len()) {
            Access::Read { chained } => {
                edits
                    .entry(path.clone())
                    .or_default()
                    .push((at + field.len(), 0, "()".into()));
                reads += 1;
                if chained {
                    chained_reads += 1;
                }
            }
            Access::Write { rhs } => {
                let value = body[rhs.0..rhs.1].trim().to_string();
                edits.entry(path.clone()).or_default().push((
                    at,
                    rhs.1 - at,
                    format!("set_{field}({value})"),
                ));
                writes += 1;
            }
            Access::Blocked(why) => {
                let source = body[..at].rfind('\n').map_or(0, |i| i + 1).min(body.len());
                let line_text = body[source..].lines().next().unwrap_or("").trim();
                blocked.push(format!("{at_site} {why}: `{line_text}`"));
            }
            Access::NotAccess => unmatched.push(format!("{at_site} (a call, not a field access)")),
        }
    }

    // The declaring file: the field loses its visibility and the accessors are added.
    let setter = writes > 0;
    for method in std::iter::once(field.clone()).chain(setter.then(|| format!("set_{field}"))) {
        anyhow::ensure!(
            !text.contains(&format!("fn {method}(")) && !text.contains(&format!("fn {method}<")),
            "`{owner}` already has a `fn {method}` in {}; rename it or the field first",
            display(root, file)
        );
    }
    let own = edits.entry(file.to_path_buf()).or_default();
    own.push((decl.vis_at, decl.name_at - decl.vis_at, String::new()));
    match inherent_impl(&text, &owner) {
        Some(open) => {
            let close = crate::parameter_object::matching_bracket(&text, open)
                .with_context(|| format!("the `impl {owner}` block does not close"))?;
            let line_start = text[..open].rfind('\n').map_or(0, |i| i + 1);
            let impl_indent: String = text[line_start..]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let methods = accessors(
                &format!("{impl_indent}    "),
                &decl.vis,
                &field,
                &decl.ty,
                by_value,
                setter,
            );
            let content_end = text[..close].trim_end().len();
            let gap = if content_end == open + 1 {
                "\n"
            } else {
                "\n\n"
            };
            own.push((
                content_end,
                close - content_end,
                format!("{gap}{}\n{impl_indent}", methods.trim_end()),
            ));
        }
        None => {
            anyhow::ensure!(
                !is_generic(&text, struct_at, &owner),
                "`{owner}` is generic and has no inherent `impl` in {} to put the accessors in; \
                 add an empty one first",
                display(root, file)
            );
            let methods = accessors("    ", &decl.vis, &field, &decl.ty, by_value, setter);
            own.push((
                struct_close + 1,
                0,
                format!("\n\nimpl {owner} {{\n{}\n}}", methods.trim_end()),
            ));
        }
    }
    texts.insert(file.to_path_buf(), text.clone());

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
            "{} use(s) of `{field}` outside {} cannot become a method call, so a private field \
             would not compile there; nothing was written:\n  {}",
            blocked.len(),
            display(root, file),
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

    Ok(EncapsulatedField {
        owner,
        root: root.to_path_buf(),
        file: display(root, file),
        field,
        ty: decl.ty,
        by_value,
        reads,
        writes,
        chained_reads,
        left_in_file,
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

    const STRUCT: &str = "pub struct Config {\n    /// How long.\n    pub timeout: u64,\n    \
                          pub(crate) name: String, // who\n    pub map: HashMap<String, Vec<u8>>\n}\n";

    #[test]
    fn a_field_declaration_gives_its_name_visibility_and_type() {
        let at = STRUCT.find("timeout").unwrap() + 3;
        let decl = field_at(STRUCT, at).unwrap();
        assert_eq!(decl.name, "timeout");
        assert_eq!(decl.vis, "pub ");
        assert_eq!(decl.ty, "u64");
        assert_eq!(&STRUCT[decl.vis_at..decl.name_at], "pub ");

        let decl = field_at(STRUCT, STRUCT.find("name:").unwrap()).unwrap();
        assert_eq!(decl.vis, "pub(crate) ");
        assert_eq!(decl.ty, "String");

        let decl = field_at(STRUCT, STRUCT.find("map").unwrap()).unwrap();
        assert_eq!(decl.ty, "HashMap<String, Vec<u8>>");

        let err = field_at("let x: u32 = 1;", 4).unwrap_err().to_string();
        assert!(err.contains("`let` comes before it"), "{err}");
        let err = field_at("use a::b;", 4).unwrap_err().to_string();
        assert!(err.contains("not a field declaration"), "{err}");
        assert!(field_at("   ", 1).is_err());
    }

    #[test]
    fn the_owner_is_the_struct_whose_braces_hold_the_field() {
        let text = format!("struct Unit;\nstruct Pair(u8, u8);\n{STRUCT}");
        let (name, at, close) = owner_at(&text, text.find("timeout").unwrap()).unwrap();
        assert_eq!(name, "Config");
        assert!(text[at..].starts_with("struct Config"));
        assert_eq!(&text[close..=close], "}");
        assert!(owner_at(&text, 3).is_none());
        assert!(!is_generic(&text, at, "Config"));
        let generic = "pub struct Page<'a, T> {\n    pub rows: &'a [T],\n}\n";
        assert!(is_generic(generic, generic.find("struct").unwrap(), "Page"));
    }

    #[test]
    fn only_an_inherent_impl_takes_the_accessors() {
        let text = "impl Default for Config {}\nimpl ConfigBuilder {}\nimpl<T> Config<T> where T: \
                    Clone {\n}\nimpl Config {}\n";
        let open = inherent_impl(text, "Config").unwrap();
        assert!(text[..open].ends_with("impl<T> Config<T> where T: Clone "));
        assert!(inherent_impl("impl Other {}\n", "Config").is_none());
        assert!(inherent_impl("implement Config {}\n", "Config").is_none());
    }

    #[test]
    fn copy_types_are_returned_by_value_and_everything_else_by_reference() {
        for ty in ["u64", "bool", "&str", "Option<u32>", "Option<&'static str>"] {
            assert!(returns_by_value(ty), "{ty}");
        }
        for ty in ["String", "Vec<u8>", "&mut u8", "Option<String>", "Duration"] {
            assert!(!returns_by_value(ty), "{ty}");
        }
    }

    fn access(text: &str) -> Access {
        let at = text.find("timeout").unwrap();
        access_at(text, at, "timeout".len())
    }

    #[test]
    fn each_use_of_a_field_is_read_for_what_it_does() {
        assert_eq!(
            access("let t = cfg.timeout;"),
            Access::Read { chained: false }
        );
        assert_eq!(
            access("if a.b().timeout == 3 {}"),
            Access::Read { chained: false }
        );
        assert_eq!(access("cfg.timeout.max(1)"), Access::Read { chained: true });
        let text = "cfg.timeout = f(a, \"; ,\", ';');\n";
        let Access::Write { rhs } = access(text) else {
            panic!("{:?}", access(text));
        };
        assert_eq!(text[rhs.0..rhs.1].trim(), "f(a, \"; ,\", ';')");
        let text = "match x { _ => cfg.timeout = 2, }";
        let Access::Write { rhs } = access(text) else {
            panic!();
        };
        assert_eq!(text[rhs.0..rhs.1].trim(), "2");
        assert!(matches!(access("cfg.timeout += 1;"), Access::Blocked(_)));
        assert!(matches!(access("cfg.timeout <<= 1;"), Access::Blocked(_)));
        assert!(matches!(
            access("cfg.timeout <= 1"),
            Access::Read { chained: false }
        ));
        assert!(matches!(
            access("Config { timeout: 1 }"),
            Access::Blocked(_)
        ));
        assert!(matches!(
            access("let Config { timeout, .. } = c;"),
            Access::Blocked(_)
        ));
        assert!(matches!(access("0..timeout"), Access::Blocked(_)));
        assert_eq!(
            access("bump(&mut self.cfgs[0].timeout);"),
            Access::Blocked("a mutable borrow of the field")
        );
        assert_eq!(
            access("f(&mut_ref.timeout)"),
            Access::Read { chained: false }
        );
        assert_eq!(
            access("f(&mut x.get()?.timeout)"),
            Access::Blocked("a mutable borrow of the field")
        );
        assert_eq!(access("cfg.timeout()"), Access::NotAccess);
        assert_eq!(access("a::b.timeout"), Access::Read { chained: false });
    }

    #[test]
    fn the_accessors_are_what_rustfmt_would_write() {
        assert_eq!(
            accessors("    ", "pub ", "timeout", "u64", true, true),
            "    pub fn timeout(&self) -> u64 {\n        self.timeout\n    }\n\n    pub fn \
             set_timeout(&mut self, timeout: u64) {\n        self.timeout = timeout;\n    }\n"
        );
        assert_eq!(
            accessors("", "pub(crate) ", "name", "String", false, false),
            "pub(crate) fn name(&self) -> &String {\n    &self.name\n}\n"
        );
    }

    fn report() -> EncapsulatedField {
        EncapsulatedField {
            owner: "Config".into(),
            root: "/root".into(),
            file: "src/config.rs".into(),
            field: "name".into(),
            ty: "String".into(),
            by_value: false,
            reads: 2,
            writes: 1,
            chained_reads: 1,
            left_in_file: 3,
            blocked: vec!["src/main.rs:4:14 a struct literal or pattern names the field".into()],
            unmatched: vec!["src/main.rs:9:1 (a call, not a field access)".into()],
            rewritten: vec![("/root/src/main.rs".into(), "fn main() {}\n".into())],
            diagnostics: vec!["mismatched types (src/main.rs:5:9)".into()],
            applied: false,
        }
    }

    #[test]
    fn the_report_names_the_accessors_and_everything_left_over() {
        let text = report().render(10_000);
        assert!(
            text.contains("getter: `fn name(&self) -> &String`"),
            "{text}"
        );
        assert!(
            text.contains("setter: `fn set_name(&mut self, name: String)`"),
            "{text}"
        );
        assert!(text.contains("2 read(s) and 1 write(s)"), "{text}");
        assert!(text.contains("3 reference(s) inside it left"), "{text}");
        assert!(text.contains("cannot become a method call"), "{text}");
        assert!(text.contains("a call, not a field access"), "{text}");
        assert!(text.contains("the analyzer rejects the result"), "{text}");
        assert!(text.contains("verify: \"compile\""), "{text}");
        assert!(text.contains("nothing was written"), "{text}");

        let mut done = report();
        done.by_value = true;
        done.writes = 0;
        done.blocked.clear();
        done.unmatched.clear();
        done.diagnostics.clear();
        done.applied = true;
        let text = done.render(10);
        assert!(text.contains("-> String`"), "{text}");
        assert!(!text.contains("setter"), "{text}");
        assert!(!text.contains("verify"), "{text}");
        assert!(text.contains("0 errors"), "{text}");
        assert!(text.contains("diff truncated"), "{text}");
        assert!(text.contains("[applied to 1 file(s)]"), "{text}");
    }
}
