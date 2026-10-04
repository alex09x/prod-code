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
        let mut out = if self.file.ends_with(".ts")
            || self.file.ends_with(".tsx")
            || self.file.ends_with(".js")
            || self.file.ends_with(".jsx")
        {
            let pascal = to_pascal_case(&self.field);
            let mut s = format!(
                "`{}.{}` ({})\n\n- the field becomes private\n- getter: `get{}(): {}`\n",
                self.owner, self.field, self.file, pascal, self.ty
            );
            if self.writes > 0 {
                s.push_str(&format!(
                    "- setter: `set{}({}: {}): void`\n",
                    pascal, self.field, self.ty
                ));
            }
            s
        } else if self.file.ends_with(".py") {
            let snake = to_snake_case(&self.field);
            let mut s = format!(
                "`{}.{}` ({})\n\n- the field becomes private\n- getter: `def get_{}(self) -> {}`\n",
                self.owner, self.field, self.file, snake, self.ty
            );
            if self.writes > 0 {
                s.push_str(&format!(
                    "- setter: `def set_{}(self, {}: {}) -> None`\n",
                    snake, self.field, self.ty
                ));
            }
            s
        } else if self.file.ends_with(".cpp")
            || self.file.ends_with(".cc")
            || self.file.ends_with(".cxx")
            || self.file.ends_with(".h")
            || self.file.ends_with(".hpp")
        {
            let snake = to_snake_case(&self.field);
            let ret_ty = if self.by_value {
                self.ty.clone()
            } else {
                format!("const {}&", self.ty)
            };
            let param_ty = if self.by_value {
                self.ty.clone()
            } else {
                format!("const {}&", self.ty)
            };
            let mut s = format!(
                "`{}::{}` ({})\n\n- the field becomes private\n- getter: `{} get_{}() const`\n",
                self.owner, self.field, self.file, ret_ty, snake
            );
            if self.writes > 0 {
                s.push_str(&format!(
                    "- setter: `void set_{}({} {})`\n",
                    snake, param_ty, self.field
                ));
            }
            s
        } else if self.file.ends_with(".swift") {
            let pascal = to_pascal_case(&self.field);
            let mut s = format!(
                "`{}.{}` ({})\n\n- the field becomes private\n- getter: `func get{}() -> {}`\n",
                self.owner, self.field, self.file, pascal, self.ty
            );
            if self.writes > 0 {
                s.push_str(&format!("- setter: `func set{}(_: {})`\n", pascal, self.ty));
            }
            s
        } else if self.file.ends_with(".go") {
            let pascal = to_pascal_case(&self.field);
            let mut s = format!(
                "`{}.{}` ({})\n\n- the field becomes private\n- getter: `func (s *{}) {}() {}`\n",
                self.owner, self.field, self.file, self.owner, pascal, self.ty
            );
            if self.writes > 0 {
                s.push_str(&format!(
                    "- setter: `func (s *{}) Set{}({} {})`\n",
                    self.owner, pascal, self.field, self.ty
                ));
            }
            s
        } else {
            let getter = if self.by_value {
                self.ty.clone()
            } else {
                format!("&{}", self.ty)
            };
            let mut s = format!(
                "`{}.{}` ({})\n\n- the field becomes private\n- getter: `fn {}(&self) -> {getter}`\n",
                self.owner, self.field, self.file, self.field
            );
            if self.writes > 0 {
                s.push_str(&format!(
                    "- setter: `fn set_{}(&mut self, {}: {})`\n",
                    self.field, self.field, self.ty
                ));
            }
            s
        };
        out.push_str(&format!(
            "- {} read(s) and {} write(s) outside {} rewritten; {} reference(s) inside it left \
             as they are, because a private field is still visible there\n\n",
            self.reads, self.writes, self.file, self.left_in_file
        ));
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
    let (name_line, name_col) = crate::signature::position_at(&text, decl.name_at)?;
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let (mut reads, mut writes, mut chained_reads, mut left_in_file) = (0, 0, 0, 0);
    let mut blocked = Vec::new();
    let mut unmatched = Vec::new();
    let refs = crate::signature::references(remote, root, file, name_line, name_col)
        .await
        .with_context(|| format!("cannot find the uses of `{field}`; nothing was planned"))?;
    for (path, rl, rc) in refs {
        if path == *file {
            left_in_file += 1;
            continue;
        }
        let body = crate::refactor::referenced_text(&mut texts, &path)?;
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
        // A use left as it was reaches a private field in a file nothing here checks; `force`
        // overrides the analyzer, not a use this did not rewrite (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{field}` were not rewritten; nothing was written:\n  {}",
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

/// Helper to convert a string to PascalCase.
pub fn to_pascal_case(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize = true;
    for c in s.chars() {
        if c == '_' {
            capitalize = true;
        } else if capitalize {
            result.extend(c.to_uppercase());
            capitalize = false;
        } else {
            result.push(c);
        }
    }
    if result.is_empty() {
        s.to_string()
    } else {
        result
    }
}

/// Helper to convert a string to snake_case.
pub fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.char_indices() {
        if c.is_uppercase() {
            if i > 0 && !result.ends_with('_') {
                result.push('_');
            }
            result.extend(c.to_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

/// Helper to lowercase the first character of a string.
pub fn lowercase_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_lowercase().chain(chars).collect(),
    }
}

/// Extract the field name at a 1-based line and column in text.
pub fn field_at_line_col(text: &str, line: u32, col: u32) -> Option<String> {
    if line == 0 {
        return None;
    }
    let target_line = text.lines().nth((line - 1) as usize)?;
    let target_units = col.saturating_sub(1);
    let mut units = 0u32;
    let mut col_idx = None;
    for (idx, ch) in target_line.char_indices() {
        if units == target_units {
            col_idx = Some(idx);
            break;
        }
        let next = units + ch.len_utf16() as u32;
        if target_units < next {
            return None;
        }
        units = next;
    }
    let col_idx = col_idx.or_else(|| (units == target_units).then_some(target_line.len()))?;
    let start = target_line[..col_idx]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(col_idx, |(i, _)| i);
    let end = target_line[col_idx..]
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
        .map_or(target_line.len(), |(i, _)| col_idx + i);
    let name = &target_line[start..end];
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    TypeScript,
    JavaScript,
    Python,
    Cpp,
    Swift,
    Go,
}

impl Language {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension().and_then(|s| s.to_str()) {
            Some("ts" | "tsx") => Some(Self::TypeScript),
            Some("js" | "jsx") => Some(Self::JavaScript),
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

fn replace_line_this(line: &str, field: &str) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;
    let mut rest = line;
    let needle = format!("this.{field}");
    while let Some(pos) = rest.find(&needle) {
        let after_pos = pos + needle.len();
        let after_char = rest[after_pos..].chars().next();
        if after_char.is_none_or(|c| !c.is_alphanumeric() && c != '_') {
            out.push_str(&rest[..pos]);
            out.push_str(&format!("this._{field}"));
            count += 1;
            rest = &rest[after_pos..];
        } else {
            out.push_str(&rest[..after_pos]);
            rest = &rest[after_pos..];
        }
    }
    out.push_str(rest);
    (out, count)
}

fn replace_line_this_private(line: &str, field: &str) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;
    let mut rest = line;
    let needle = format!("this.{field}");
    while let Some(pos) = rest.find(&needle) {
        let after_pos = pos + needle.len();
        let after_char = rest[after_pos..].chars().next();
        if after_char.is_none_or(|c| !c.is_alphanumeric() && c != '_') {
            out.push_str(&rest[..pos]);
            out.push_str(&format!("this.#{field}"));
            count += 1;
            rest = &rest[after_pos..];
        } else {
            out.push_str(&rest[..after_pos]);
            rest = &rest[after_pos..];
        }
    }
    out.push_str(rest);
    (out, count)
}

fn replace_line_self(line: &str, field: &str) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;
    let mut rest = line;
    let needle = format!("self.{field}");
    while let Some(pos) = rest.find(&needle) {
        let after_pos = pos + needle.len();
        let after_char = rest[after_pos..].chars().next();
        if after_char.is_none_or(|c| !c.is_alphanumeric() && c != '_') {
            out.push_str(&rest[..pos]);
            out.push_str(&format!("self._{field}"));
            count += 1;
            rest = &rest[after_pos..];
        } else {
            out.push_str(&rest[..after_pos]);
            rest = &rest[after_pos..];
        }
    }
    out.push_str(rest);
    (out, count)
}

fn replace_cpp_unqualified(
    line: &str,
    field: &str,
    in_block_comment: &mut bool,
) -> (String, usize, bool) {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut out = String::with_capacity(line.len());
    let mut changed = 0;
    let mut shadowed = false;
    let mut i = 0;
    let mut quote = None;
    let mut escaped = false;
    while i < chars.len() {
        let (at, ch) = chars[i];
        let next = chars.get(i + 1).map(|(_, c)| *c);
        if *in_block_comment {
            out.push(ch);
            if ch == '*' && next == Some('/') {
                out.push('/');
                i += 2;
                *in_block_comment = false;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
            i += 1;
            continue;
        }
        if ch == '/' && next == Some('/') {
            out.push_str(&line[at..]);
            break;
        }
        if ch == '/' && next == Some('*') {
            out.push_str("/*");
            i += 2;
            *in_block_comment = true;
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            out.push(ch);
            i += 1;
            continue;
        }
        if is_ident(ch) && !ch.is_ascii_digit() {
            let start_i = i;
            i += 1;
            while i < chars.len() && is_ident(chars[i].1) {
                i += 1;
            }
            let start = chars[start_i].0;
            let end = chars.get(i).map_or(line.len(), |(byte, _)| *byte);
            let before = line[..start].trim_end();
            let after = line[end..].trim_start();
            let qualified_this = before.ends_with("this->");
            let qualified = (before.ends_with('.') || before.ends_with('>')) && !qualified_this;
            if &line[start..end] == field
                && (!qualified || qualified_this)
                && !after.starts_with('(')
            {
                let previous = before.split_whitespace().next_back().unwrap_or_default();
                if matches!(after.chars().next(), Some('=' | ';' | ',' | ')' | '{'))
                    && !matches!(previous, "return" | "throw" | "co_return" | "case")
                    && !before.ends_with('(')
                {
                    shadowed = true;
                }
                out.push_str(&format!("{field}_"));
                changed += 1;
            } else {
                out.push_str(&line[start..end]);
            }
            continue;
        }
        out.push(ch);
        i += 1;
    }
    (out, changed, shadowed)
}

pub fn rewrite_external_ts(code: &str, field: &str) -> (String, usize, usize) {
    let pascal = to_pascal_case(field);
    let mut reads = 0;
    let mut writes = 0;
    let mut out = String::new();
    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let needle = format!(".{field}");
        if !line.contains(&needle) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut new_line = String::new();
        let mut rest = line;
        while let Some(pos) = rest.find(&needle) {
            let before = &rest[..pos];
            let after = &rest[pos + needle.len()..];
            let before_trimmed = before.trim_end();
            if before_trimmed.ends_with("this") || before_trimmed.ends_with("this._") {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            let trimmed_after = after.trim_start();
            if trimmed_after.starts_with('(') {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            if let Some(c) = after.chars().next()
                && (c.is_alphanumeric() || c == '_')
            {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            if trimmed_after.starts_with('=')
                && !trimmed_after.starts_with("==")
                && !trimmed_after.starts_with("=>")
            {
                writes += 1;
                let rhs_with_sep = trimmed_after[1..].trim_start();
                let (rhs, sep) = if let Some(semi_pos) = rhs_with_sep.find(';') {
                    (&rhs_with_sep[..semi_pos], &rhs_with_sep[semi_pos..])
                } else {
                    (rhs_with_sep, "")
                };
                new_line.push_str(before);
                new_line.push_str(&format!(".set{pascal}({rhs}){sep}"));
                rest = "";
                break;
            } else {
                reads += 1;
                new_line.push_str(before);
                new_line.push_str(&format!(".get{pascal}()"));
                rest = after;
            }
        }
        new_line.push_str(rest);
        out.push_str(&new_line);
        out.push('\n');
    }
    if !code.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    (out, reads, writes)
}

pub fn rewrite_external_py(code: &str, field: &str) -> (String, usize, usize) {
    let snake = to_snake_case(field);
    let mut reads = 0;
    let mut writes = 0;
    let mut out = String::new();
    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let needle = format!(".{field}");
        if !line.contains(&needle) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut new_line = String::new();
        let mut rest = line;
        while let Some(pos) = rest.find(&needle) {
            let before = &rest[..pos];
            let after = &rest[pos + needle.len()..];
            let before_trimmed = before.trim_end();
            if before_trimmed.ends_with("self") || before_trimmed.ends_with("self._") {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            let trimmed_after = after.trim_start();
            if trimmed_after.starts_with('(') {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            if let Some(c) = after.chars().next()
                && (c.is_alphanumeric() || c == '_')
            {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            if trimmed_after.starts_with('=') && !trimmed_after.starts_with("==") {
                writes += 1;
                let rhs = trimmed_after[1..].trim();
                new_line.push_str(before);
                new_line.push_str(&format!(".set_{snake}({rhs})"));
                rest = "";
                break;
            } else {
                reads += 1;
                new_line.push_str(before);
                new_line.push_str(&format!(".get_{snake}()"));
                rest = after;
            }
        }
        new_line.push_str(rest);
        out.push_str(&new_line);
        out.push('\n');
    }
    if !code.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    (out, reads, writes)
}

pub fn rewrite_external_cpp(code: &str, field: &str) -> (String, usize, usize) {
    let snake = to_snake_case(field);
    let mut reads = 0;
    let mut writes = 0;
    let mut out = String::new();
    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let dot_needle = format!(".{field}");
        let arrow_needle = format!("->{field}");
        if !line.contains(&dot_needle) && !line.contains(&arrow_needle) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut new_line = String::new();
        let mut rest = line;
        while let Some(pos) = rest.find(&dot_needle).or_else(|| rest.find(&arrow_needle)) {
            let is_arrow = rest[pos..].starts_with("->");
            let op_len = if is_arrow { 2 } else { 1 };
            let total_len = op_len + field.len();
            let before = &rest[..pos];
            let after = &rest[pos + total_len..];
            let before_trimmed = before.trim_end();
            if before_trimmed.ends_with("this") {
                new_line.push_str(&rest[..pos + total_len]);
                rest = after;
                continue;
            }
            let trimmed_after = after.trim_start();
            if trimmed_after.starts_with('(') {
                new_line.push_str(&rest[..pos + total_len]);
                rest = after;
                continue;
            }
            if let Some(c) = after.chars().next()
                && (c.is_alphanumeric() || c == '_')
            {
                new_line.push_str(&rest[..pos + total_len]);
                rest = after;
                continue;
            }
            let op_str = if is_arrow { "->" } else { "." };
            if trimmed_after.starts_with('=') && !trimmed_after.starts_with("==") {
                writes += 1;
                let rhs_with_sep = trimmed_after[1..].trim_start();
                let (rhs, sep) = if let Some(semi_pos) = rhs_with_sep.find(';') {
                    (&rhs_with_sep[..semi_pos], &rhs_with_sep[semi_pos..])
                } else {
                    (rhs_with_sep, "")
                };
                new_line.push_str(before);
                new_line.push_str(&format!("{op_str}set_{snake}({rhs}){sep}"));
                rest = "";
                break;
            } else {
                reads += 1;
                new_line.push_str(before);
                new_line.push_str(&format!("{op_str}get_{snake}()"));
                rest = after;
            }
        }
        new_line.push_str(rest);
        out.push_str(&new_line);
        out.push('\n');
    }
    if !code.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    (out, reads, writes)
}

pub fn rewrite_external_swift(code: &str, field: &str) -> (String, usize, usize) {
    let pascal = to_pascal_case(field);
    let mut reads = 0;
    let mut writes = 0;
    let mut out = String::new();
    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let needle = format!(".{field}");
        if !line.contains(&needle) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut new_line = String::new();
        let mut rest = line;
        while let Some(pos) = rest.find(&needle) {
            let before = &rest[..pos];
            let after = &rest[pos + needle.len()..];
            let before_trimmed = before.trim_end();
            if before_trimmed.ends_with("self") || before_trimmed.ends_with("self._") {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            let trimmed_after = after.trim_start();
            if trimmed_after.starts_with('(') {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            if let Some(c) = after.chars().next()
                && (c.is_alphanumeric() || c == '_')
            {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            if trimmed_after.starts_with('=') && !trimmed_after.starts_with("==") {
                writes += 1;
                let rhs_with_sep = trimmed_after[1..].trim_start();
                let (rhs, sep) = if let Some(semi_pos) = rhs_with_sep.find(';') {
                    (&rhs_with_sep[..semi_pos], &rhs_with_sep[semi_pos..])
                } else {
                    (rhs_with_sep, "")
                };
                new_line.push_str(before);
                new_line.push_str(&format!(".set{pascal}({rhs}){sep}"));
                rest = "";
                break;
            } else {
                reads += 1;
                new_line.push_str(before);
                new_line.push_str(&format!(".get{pascal}()"));
                rest = after;
            }
        }
        new_line.push_str(rest);
        out.push_str(&new_line);
        out.push('\n');
    }
    if !code.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    (out, reads, writes)
}

pub fn rewrite_external_go(code: &str, field: &str) -> (String, usize, usize) {
    let pascal = to_pascal_case(field);
    let mut reads = 0;
    let mut writes = 0;
    let mut out = String::new();
    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let needle_pascal = format!(".{pascal}");
        let needle_orig = format!(".{field}");
        let needle = if line.contains(&needle_pascal) {
            needle_pascal
        } else if line.contains(&needle_orig) {
            needle_orig
        } else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let mut new_line = String::new();
        let mut rest = line;
        while let Some(pos) = rest.find(&needle) {
            let before = &rest[..pos];
            let after = &rest[pos + needle.len()..];
            let trimmed_after = after.trim_start();
            if trimmed_after.starts_with('(') {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            if let Some(c) = after.chars().next()
                && (c.is_alphanumeric() || c == '_')
            {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            if trimmed_after.starts_with('=') && !trimmed_after.starts_with("==") {
                writes += 1;
                let rhs = trimmed_after[1..].trim();
                new_line.push_str(before);
                new_line.push_str(&format!(".Set{pascal}({rhs})"));
                rest = "";
                break;
            } else {
                reads += 1;
                new_line.push_str(before);
                new_line.push_str(&format!(".{pascal}()"));
                rest = after;
            }
        }
        new_line.push_str(rest);
        out.push_str(&new_line);
        out.push('\n');
    }
    if !code.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    (out, reads, writes)
}

pub fn encapsulate_field_ts(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
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

    let c_start = class_start.context("Could not find class in file")?;
    let c_end = class_end.context("Could not find closing brace of class")?;

    let mut field_line_idx = None;
    let mut field_type = String::new();
    let mut field_default = None;
    let mut field_indent = "    ".to_string();

    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let is_match = words.iter().any(|w| {
            let clean = w.trim_matches(|c| c == ':' || c == ';' || c == '=');
            clean == target_field
        });
        if is_match && !trimmed.contains('(') {
            field_line_idx = Some(idx);
            let indent_len = line.len() - trimmed.len();
            field_indent = line[..indent_len].to_string();
            if let Some(colon_pos) = trimmed.find(':') {
                let after_colon = &trimmed[colon_pos + 1..];
                let ty_end = after_colon
                    .find('=')
                    .or_else(|| after_colon.find(';'))
                    .unwrap_or(after_colon.len());
                field_type = after_colon[..ty_end].trim().to_string();
            }
            if let Some(eq_pos) = trimmed.find('=') {
                let after_eq = &trimmed[eq_pos + 1..];
                let def_end = after_eq.find(';').unwrap_or(after_eq.len());
                field_default = Some(after_eq[..def_end].trim().to_string());
            }
            break;
        }
    }

    let f_idx = field_line_idx
        .with_context(|| format!("Field `{target_field}` not found in class `{class_name}`"))?;
    let type_colon = if field_type.is_empty() {
        String::new()
    } else {
        format!(": {field_type}")
    };
    let default_eq = field_default.map(|d| format!(" = {d}")).unwrap_or_default();
    let new_field_line = format!("{field_indent}private _{target_field}{type_colon}{default_eq};");

    let pascal = to_pascal_case(target_field);
    let accessors = format!(
        "\n{field_indent}public get{pascal}(){type_colon} {{\n{field_indent}    return this._{target_field};\n{field_indent}}}\n\n{field_indent}public set{pascal}({target_field}{type_colon}): void {{\n{field_indent}    this._{target_field} = {target_field};\n{field_indent}}}\n"
    );

    let mut out_lines = Vec::new();
    let mut left_direct = 0;
    let mut reads = 0;
    let mut writes = 0;
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            out_lines.push(new_field_line.clone());
        } else if idx > c_start && idx < c_end {
            let (replaced_line, count) = replace_line_this(line, target_field);
            left_direct += count;
            let (replaced_line, r, w) = rewrite_external_ts(&replaced_line, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced_line);
        } else if idx == c_end {
            out_lines.push(accessors.clone());
            out_lines.push(line.to_string());
        } else {
            let (replaced, r, w) = rewrite_external_ts(line, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        }
    }

    let final_code = out_lines.join("\n");
    Ok((
        class_name,
        field_type,
        final_code,
        reads,
        writes,
        left_direct,
    ))
}

/// JavaScript uses private class fields and ordinary methods rather than TypeScript modifiers
/// and annotations. External accesses keep the same method-call form used by the other
/// generators.
pub fn encapsulate_field_js(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
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
            let name = words
                .iter()
                .enumerate()
                .find_map(|(i, word)| {
                    (*word == "class")
                        .then(|| words.get(i + 1).copied())
                        .flatten()
                })
                .unwrap_or("")
                .trim_matches('{')
                .trim();
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

    let c_start = class_start.context("Could not find class in JavaScript file")?;
    let c_end = class_end.context("Could not find closing brace of JavaScript class")?;
    let mut field_line_idx = None;
    let mut field_default = String::new();
    let mut field_indent = "    ".to_string();
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let is_match = words
            .iter()
            .any(|w| w.trim_matches(|c| c == ';' || c == '=' || c == ',') == target_field);
        if is_match && !trimmed.contains('(') {
            field_line_idx = Some(idx);
            field_indent = line[..line.len() - trimmed.len()].to_string();
            if let Some(eq) = trimmed.find('=') {
                let value = trimmed[eq + 1..].trim().trim_end_matches(';').trim();
                field_default = format!(" = {value}");
            }
            break;
        }
    }
    let f_idx = field_line_idx.with_context(|| {
        format!("Field `{target_field}` not found in JavaScript class `{class_name}`")
    })?;
    let pascal = to_pascal_case(target_field);
    let new_field = format!("{field_indent}#{target_field}{field_default};");
    let accessors = format!(
        "\n{field_indent}get{pascal}() {{\n{field_indent}    return this.#{target_field};\n{field_indent}}}\n\n{field_indent}set{pascal}({target_field}) {{\n{field_indent}    this.#{target_field} = {target_field};\n{field_indent}}}\n"
    );
    let mut out_lines = Vec::new();
    let mut reads = 0;
    let mut writes = 0;
    let mut internal = 0;
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            out_lines.push(new_field.clone());
        } else if idx > c_start && idx < c_end {
            let (replaced, count) = replace_line_this_private(line, target_field);
            internal += count;
            let (replaced, r, w) = rewrite_external_ts(&replaced, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        } else if idx == c_end {
            out_lines.push(accessors.clone());
            out_lines.push(line.to_string());
        } else {
            let (replaced, r, w) = rewrite_external_ts(line, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        }
    }
    let mut final_code = out_lines.join("\n");
    if !text.ends_with('\n') && final_code.ends_with('\n') {
        final_code.pop();
    }
    Ok((
        class_name,
        String::new(),
        final_code,
        reads,
        writes,
        internal,
    ))
}

pub fn encapsulate_field_py(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut class_start = None;
    let mut class_name = String::new();
    let mut class_end = lines.len();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("class ") {
            let name = rest.split(['(', ':']).next().unwrap_or("").trim();
            if target_class.is_none() || target_class == Some(name) {
                class_start = Some(idx);
                class_name = name.to_string();
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class in Python file")?;
    for (idx, line) in lines.iter().enumerate().skip(c_start + 1) {
        if !line.trim().is_empty() && !line.starts_with(' ') && !line.starts_with('\t') {
            class_end = idx;
            break;
        }
    }

    let mut field_type = String::new();
    let mut indent = "    ".to_string();
    let mut left_direct = 0;
    let mut reads = 0;
    let mut writes = 0;
    let mut out_lines = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        if idx > c_start && idx < class_end {
            let trimmed = line.trim_start();
            if trimmed.starts_with("def ") {
                let cur_indent_len = line.len() - trimmed.len();
                indent = line[..cur_indent_len].to_string();
            }
            let self_needle = format!("self.{target_field}");
            if line.contains(&self_needle) {
                let (replaced, cnt) = replace_line_self(line, target_field);
                left_direct += cnt;
                if trimmed.contains(&format!("self.{target_field}:"))
                    && let Some(pos) = trimmed.find(':')
                {
                    let after = &trimmed[pos + 1..];
                    let ty_end = after.find('=').unwrap_or(after.len());
                    field_type = after[..ty_end].trim().to_string();
                }
                let (replaced, r, w) = rewrite_external_py(&replaced, target_field);
                reads += r;
                writes += w;
                out_lines.push(replaced);
            } else if trimmed.starts_with(&format!("{target_field}:"))
                || trimmed.starts_with(&format!("{target_field} ="))
            {
                let cur_indent_len = line.len() - trimmed.len();
                let cur_indent = &line[..cur_indent_len];
                if let Some(pos) = trimmed.find(':') {
                    let after = &trimmed[pos + 1..];
                    let ty_end = after.find('=').unwrap_or(after.len());
                    field_type = after[..ty_end].trim().to_string();
                }
                let rest_of_line = &trimmed[target_field.len()..];
                out_lines.push(format!("{cur_indent}_{target_field}{rest_of_line}"));
                left_direct += 1;
            } else {
                let (replaced, r, w) = rewrite_external_py(line, target_field);
                reads += r;
                writes += w;
                out_lines.push(replaced);
            }
        } else {
            let (replaced, r, w) = rewrite_external_py(line, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        }
    }

    if field_type.is_empty() {
        for line in &lines[c_start..class_end] {
            if let Some(pos) = line.find(&format!("{target_field}:")) {
                let after = &line[pos + target_field.len() + 1..];
                let ty_end = after
                    .find([',', ')', '=', '#', '\n'])
                    .unwrap_or(after.len());
                let found = after[..ty_end].trim();
                if !found.is_empty() {
                    field_type = found.to_string();
                    break;
                }
            }
        }
    }

    let snake = to_snake_case(target_field);
    let ret_annot = if field_type.is_empty() {
        String::new()
    } else {
        format!(" -> {field_type}")
    };
    let param_annot = if field_type.is_empty() {
        String::new()
    } else {
        format!(": {field_type}")
    };
    let accessors = format!(
        "\n{indent}def get_{snake}(self){ret_annot}:\n{indent}    return self._{target_field}\n\n{indent}def set_{snake}(self, {target_field}{param_annot}) -> None:\n{indent}    self._{target_field} = {target_field}\n"
    );

    out_lines.insert(class_end, accessors);

    let final_code = out_lines.join("\n");
    Ok((
        class_name,
        field_type,
        final_code,
        reads,
        writes,
        left_direct,
    ))
}

pub fn encapsulate_field_cpp(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
    by_value: Option<bool>,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
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

    let c_start = class_start.context("Could not find class or struct in C++ file")?;
    let c_end = class_end.context("Could not find closing brace of C++ class")?;

    let mut field_line_idx = None;
    let mut field_type = String::new();
    let mut field_indent = "    ".to_string();

    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        if trimmed.contains('(') {
            continue;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let is_match = words.iter().any(|w| {
            let clean = w.trim_matches(|c| c == ';' || c == '=');
            clean == target_field
        });
        if is_match {
            field_line_idx = Some(idx);
            let indent_len = line.len() - trimmed.len();
            field_indent = line[..indent_len].to_string();
            if let Some(pos) = line.find(target_field) {
                let ty_part = line[..pos].trim_start();
                let clean_ty = ty_part
                    .trim_start_matches("public:")
                    .trim_start_matches("private:")
                    .trim_start_matches("protected:")
                    .trim();
                field_type = clean_ty.to_string();
            }
            break;
        }
    }

    let f_idx = field_line_idx
        .with_context(|| format!("Field `{target_field}` not found in class `{class_name}`"))?;
    let is_primitive = matches!(
        field_type.as_str(),
        "int"
            | "long"
            | "short"
            | "float"
            | "double"
            | "bool"
            | "char"
            | "size_t"
            | "int32_t"
            | "int64_t"
            | "uint32_t"
            | "uint64_t"
    );
    let ret_by_val = by_value.unwrap_or(is_primitive);
    let ret_ty = if ret_by_val {
        field_type.clone()
    } else {
        format!("const {}&", field_type)
    };
    let param_ty = if ret_by_val {
        field_type.clone()
    } else {
        format!("const {}&", field_type)
    };
    let snake = to_snake_case(target_field);

    let accessors = format!(
        "\npublic:\n{field_indent}{ret_ty} get_{snake}() const {{\n{field_indent}    return {target_field}_;\n{field_indent}}}\n\n{field_indent}void set_{snake}({param_ty} {target_field}) {{\n{field_indent}    {target_field}_ = {target_field};\n{field_indent}}}\n\nprivate:\n{field_indent}{field_type} {target_field}_;\n"
    );

    let mut out_lines = Vec::new();
    let mut left_direct = 0;
    let mut in_block_comment = false;
    let mut reads = 0;
    let mut writes = 0;
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            continue;
        } else if idx > c_start && idx < c_end {
            let (replaced, bare_count, shadowed) =
                replace_cpp_unqualified(line, target_field, &mut in_block_comment);
            anyhow::ensure!(
                !shadowed,
                "cannot safely rewrite unqualified `{target_field}` uses because a local or parameter with that name may shadow the field"
            );
            left_direct += bare_count;
            let (replaced, r, w) = rewrite_external_cpp(&replaced, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        } else if idx == c_end {
            out_lines.push(accessors.clone());
            out_lines.push(line.to_string());
        } else {
            let (replaced, r, w) = rewrite_external_cpp(line, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        }
    }

    let final_code = out_lines.join("\n");
    Ok((
        class_name,
        field_type,
        final_code,
        reads,
        writes,
        left_direct,
    ))
}

pub fn encapsulate_field_swift(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut is_struct = false;
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ")
            || trimmed.starts_with("struct ")
            || trimmed.starts_with("public class ")
            || trimmed.starts_with("public struct ")
        {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let mut name = "";
            for (w_idx, w) in words.iter().enumerate() {
                if (*w == "class" || *w == "struct") && w_idx + 1 < words.len() {
                    name = words[w_idx + 1]
                        .trim_matches(|c| c == '{' || c == ':')
                        .trim();
                    is_struct = *w == "struct";
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

    let c_start = class_start.context("Could not find class or struct in Swift file")?;
    let c_end = class_end.context("Could not find closing brace of Swift class")?;

    let mut field_line_idx = None;
    let mut field_type = String::new();
    let mut field_indent = "    ".to_string();

    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        if trimmed.contains('(') {
            continue;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let is_match = words.iter().any(|w| {
            let clean = w.trim_matches(|c| c == ':' || c == '=');
            clean == target_field
        });
        if is_match
            && (trimmed.starts_with("var ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("public var ")
                || trimmed.starts_with("public let "))
        {
            field_line_idx = Some(idx);
            let indent_len = line.len() - trimmed.len();
            field_indent = line[..indent_len].to_string();
            if let Some(colon_pos) = trimmed.find(':') {
                let after = &trimmed[colon_pos + 1..];
                let ty_end = after.find('=').unwrap_or(after.len());
                field_type = after[..ty_end].trim().to_string();
            }
            break;
        }
    }

    let f_idx = field_line_idx.with_context(|| {
        format!("Field `{target_field}` not found in Swift type `{class_name}`")
    })?;
    let pascal = to_pascal_case(target_field);
    let mut_kw = if is_struct { "mutating " } else { "" };
    let accessors = format!(
        "\n{field_indent}func get{pascal}() -> {field_type} {{\n{field_indent}    return _{target_field}\n{field_indent}}}\n\n{field_indent}{mut_kw}func set{pascal}(_ {target_field}: {field_type}) {{\n{field_indent}    _{target_field} = {target_field}\n{field_indent}}}\n"
    );

    let new_field_decl = format!("{field_indent}private var _{target_field}: {field_type}");

    let mut out_lines = Vec::new();
    let mut left_direct = 0;
    let mut reads = 0;
    let mut writes = 0;
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            out_lines.push(new_field_decl.clone());
        } else if idx > c_start && idx < c_end {
            let (replaced, cnt) = replace_line_self(line, target_field);
            left_direct += cnt;
            let (replaced, r, w) = rewrite_external_swift(&replaced, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        } else if idx == c_end {
            out_lines.push(accessors.clone());
            out_lines.push(line.to_string());
        } else {
            let (replaced, r, w) = rewrite_external_swift(line, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        }
    }

    let final_code = out_lines.join("\n");
    Ok((
        class_name,
        field_type,
        final_code,
        reads,
        writes,
        left_direct,
    ))
}

pub fn encapsulate_field_go(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut struct_start = None;
    let mut struct_end = None;
    let mut struct_name = String::new();
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("type ") && trimmed.contains(" struct") {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            if words.len() >= 2 {
                let name = words[1];
                if target_class.is_none() || target_class == Some(name) {
                    struct_start = Some(idx);
                    struct_name = name.to_string();
                    brace_depth = 0;
                }
            }
        }
        if struct_start.is_some() && struct_end.is_none() {
            brace_depth += line.chars().filter(|&c| c == '{').count() as i32;
            brace_depth -= line.chars().filter(|&c| c == '}').count() as i32;
            if brace_depth == 0 && line.contains('}') {
                struct_end = Some(idx);
                break;
            }
        }
    }

    let s_start = struct_start.context("Could not find struct in Go file")?;
    let s_end = struct_end.context("Could not find closing brace of Go struct")?;

    let mut field_line_idx = None;
    let mut field_type = String::new();
    let mut field_indent = "\t".to_string();

    let target_unexported = lowercase_first(target_field);
    let target_pascal = to_pascal_case(target_field);

    for (idx, line) in lines.iter().enumerate().take(s_end).skip(s_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if !words.is_empty() {
            let fld = words[0];
            if fld == target_field || fld == target_pascal || fld == target_unexported {
                field_line_idx = Some(idx);
                let indent_len = line.len() - trimmed.len();
                field_indent = line[..indent_len].to_string();
                if words.len() >= 2 {
                    field_type = words[1].to_string();
                }
                break;
            }
        }
    }

    let f_idx = field_line_idx
        .with_context(|| format!("Field `{target_field}` not found in struct `{struct_name}`"))?;
    let declared_name = lines[f_idx].split_whitespace().next().unwrap_or_default();
    anyhow::ensure!(
        !declared_name.chars().next().is_some_and(char::is_uppercase),
        "cannot encapsulate exported Go field `{declared_name}` without changing its public API and serialization behavior"
    );
    anyhow::ensure!(
        !lines[f_idx].contains('`'),
        "cannot encapsulate tagged Go field `{declared_name}` without preserving reflection behavior"
    );
    let unexported_field = if target_pascal.chars().all(|c| c.is_ascii_uppercase()) {
        target_pascal.to_lowercase()
    } else {
        lowercase_first(&target_pascal)
    };
    let new_field_line = format!("{field_indent}{unexported_field} {field_type}");

    let recv = struct_name
        .chars()
        .next()
        .unwrap_or('s')
        .to_lowercase()
        .to_string();
    let accessors = format!(
        "\nfunc ({recv} *{struct_name}) {target_pascal}() {field_type} {{\n\treturn {recv}.{unexported_field}\n}}\n\nfunc ({recv} *{struct_name}) Set{target_pascal}({unexported_field} {field_type}) {{\n\t{recv}.{unexported_field} = {unexported_field}\n}}\n"
    );

    let mut out_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            out_lines.push(new_field_line.clone());
        } else if idx == s_end {
            out_lines.push(line.to_string());
            out_lines.push(accessors.clone());
        } else {
            out_lines.push(line.to_string());
        }
    }

    let final_code = out_lines.join("\n");
    let reads = 0;
    let writes = 0;
    Ok((struct_name, field_type, final_code, reads, writes, 0))
}

fn has_ambiguous_property_use(text: &str, field: &str) -> bool {
    let needles = [format!(".{field}"), format!("->{field}")];
    text.lines().any(|line| {
        needles.iter().any(|needle| {
            let mut rest = line;
            while let Some(pos) = rest.find(needle) {
                let before = rest[..pos].trim_end();
                let receiver = before
                    .rsplit(|c: char| !is_ident(c))
                    .next()
                    .unwrap_or_default();
                if receiver != "this" && receiver != "self" {
                    return true;
                }
                rest = &rest[pos + needle.len()..];
            }
            false
        })
    })
}

fn lsp_symbol_position(symbol: &serde_json::Value) -> Option<(u32, u32)> {
    let start = symbol
        .pointer("/selectionRange/start")
        .or_else(|| symbol.pointer("/location/range/start"))
        .or_else(|| symbol.pointer("/range/start"))?;
    let line = u32::try_from(start.get("line")?.as_u64()?)
        .ok()?
        .checked_add(1)?;
    let col = u32::try_from(start.get("character")?.as_u64()?)
        .ok()?
        .checked_add(1)?;
    Some((line, col))
}

fn field_position_in_symbols(
    symbols: &serde_json::Value,
    owner: &str,
    field: &str,
) -> Option<(u32, u32)> {
    fn visit(
        symbols: &[serde_json::Value],
        owner: &str,
        field: &str,
        inside_owner: bool,
    ) -> Option<(u32, u32)> {
        for symbol in symbols {
            let name = symbol.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let is_owner = name == owner;
            if (inside_owner || is_owner) && name.trim_start_matches('#') == field {
                if let Some(position) = lsp_symbol_position(symbol) {
                    return Some(position);
                }
            }
            if name.trim_start_matches('#') == field
                && symbol.get("containerName").and_then(|n| n.as_str()) == Some(owner)
                && let Some(position) = lsp_symbol_position(symbol)
            {
                return Some(position);
            }
            if let Some(children) = symbol.get("children").and_then(|v| v.as_array())
                && let Some(position) = visit(children, owner, field, inside_owner || is_owner)
            {
                return Some(position);
            }
        }
        None
    }

    visit(symbols.as_array()?, owner, field, false)
}

pub(crate) async fn field_position_in_lsp(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    owner: &str,
    field: &str,
) -> Result<Option<(u32, u32)>> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid field source path {:?}", file))?
        .to_string();
    let symbols = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await?;
    Ok(field_position_in_symbols(&symbols, owner, field))
}

fn rewrite_external_line(lang: Language, line: &str, field: &str) -> (String, usize, usize) {
    match lang {
        Language::TypeScript | Language::JavaScript => rewrite_external_ts(line, field),
        Language::Python => rewrite_external_py(line, field),
        Language::Cpp => rewrite_external_cpp(line, field),
        Language::Swift => rewrite_external_swift(line, field),
        Language::Go => rewrite_external_go(line, field),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn encapsulate_polyglot(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    class_name: Option<&str>,
    field_name: &str,
    by_value: Option<bool>,
    apply: bool,
    force: bool,
) -> Result<EncapsulatedField> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Cannot read {}", file_path.display()))?;
    let lang = Language::from_path(file_path)
        .with_context(|| format!("Unsupported language for file: {}", file_path.display()))?;

    let (owner, ty, new_content, file_reads, file_writes, left_in_file) = match lang {
        Language::TypeScript => encapsulate_field_ts(&content, class_name, field_name)?,
        Language::JavaScript => encapsulate_field_js(&content, class_name, field_name)?,
        Language::Python => encapsulate_field_py(&content, class_name, field_name)?,
        Language::Cpp => encapsulate_field_cpp(&content, class_name, field_name, by_value)?,
        Language::Swift => encapsulate_field_swift(&content, class_name, field_name)?,
        Language::Go => encapsulate_field_go(&content, class_name, field_name)?,
    };

    let mut rewritten = vec![(file_path.to_string_lossy().to_string(), new_content)];
    let mut total_reads = file_reads;
    let mut total_writes = file_writes;
    let mut unmatched = Vec::new();
    let dot_access = format!(".{field_name}");
    let quoted_or_commented_access = content.lines().any(|line| {
        line.contains(&dot_access)
            && (line.contains('\"')
                || line.contains('\'')
                || line.contains("//")
                || line.contains("/*"))
    });
    let canonical_root =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let canonical_file =
        std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    let mut references_by_file: BTreeMap<PathBuf, BTreeMap<u32, Vec<u32>>> = BTreeMap::new();
    let references_available =
        match field_position_in_lsp(remote, workspace_root, file_path, &owner, field_name).await {
            Ok(Some((line, col))) => {
                match crate::signature::references(remote, workspace_root, file_path, line, col)
                    .await
                {
                    Ok(references) => {
                        for (path, line, col) in references {
                            let path = std::fs::canonicalize(&path).unwrap_or(path);
                            references_by_file
                                .entry(path)
                                .or_default()
                                .entry(line)
                                .or_default()
                                .push(col);
                        }
                        true
                    }
                    Err(err) => {
                        unmatched.push(format!(
                            "{}: analyzer references could not be verified: {err:#}",
                            display(workspace_root, file_path)
                        ));
                        false
                    }
                }
            }
            Ok(None) => false,
            Err(err) => {
                unmatched.push(format!(
                    "{}: field symbols could not be resolved: {err:#}",
                    display(workspace_root, file_path)
                ));
                false
            }
        };

    if !references_available && quoted_or_commented_access {
        unmatched.push(format!(
            "{}: possible `{field_name}` references occur in string or comment text and need semantic resolution",
            display(workspace_root, file_path)
        ));
    }

    let target_reference_count: usize = references_by_file
        .get(&canonical_file)
        .into_iter()
        .flat_map(|lines| lines.values())
        .map(Vec::len)
        .sum();
    if references_available && target_reference_count != file_reads + file_writes + left_in_file {
        unmatched.push(format!(
            "{}: analyzer found {target_reference_count} field reference(s) in the declaring file, but only {} could be rewritten",
            display(workspace_root, file_path),
            file_reads + file_writes + left_in_file
        ));
    }
    if !references_available && has_ambiguous_property_use(&content, field_name) {
        unmatched.push(format!(
            "{}: property accesses cannot be proven to belong to `{owner}`",
            display(workspace_root, file_path)
        ));
    }

    for (path, ref_lines) in &references_by_file {
        if path == &canonical_file {
            continue;
        }
        if !path.starts_with(&canonical_root) || !lang.matches_extension(path) {
            unmatched.push(format!(
                "{}: analyzer reference is outside the supported workspace language",
                display(workspace_root, path)
            ));
            continue;
        }
        let other_content = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                unmatched.push(format!(
                    "{}: referenced source could not be read: {err}",
                    display(workspace_root, path)
                ));
                continue;
            }
        };
        let mut output = String::with_capacity(other_content.len());
        let mut file_reads = 0;
        let mut file_writes = 0;
        let mut rewrite_failed = false;
        for (index, source_line) in other_content.split_inclusive('\n').enumerate() {
            let line_number = index as u32 + 1;
            let Some(columns) = ref_lines.get(&line_number) else {
                if source_line.contains(&format!(".{field_name}"))
                    || (lang == Language::Cpp && source_line.contains(&format!("->{field_name}")))
                {
                    unmatched.push(format!(
                        "{}:{}: possible `{field_name}` access is absent from analyzer references",
                        display(workspace_root, path),
                        line_number
                    ));
                    rewrite_failed = true;
                }
                output.push_str(source_line);
                continue;
            };
            let (body, ending) = if let Some(body) = source_line.strip_suffix("\r\n") {
                (body, "\r\n")
            } else if let Some(body) = source_line.strip_suffix('\n') {
                (body, "\n")
            } else {
                (source_line, "")
            };
            if columns
                .iter()
                .any(|col| field_at_line_col(body, 1, *col).as_deref() != Some(field_name))
            {
                unmatched.push(format!(
                    "{}:{}: analyzer reference does not point at `{field_name}`",
                    display(workspace_root, path),
                    line_number
                ));
                rewrite_failed = true;
                output.push_str(source_line);
                continue;
            }
            let (changed, reads, writes) = rewrite_external_line(lang, body, field_name);
            if reads + writes != columns.len() {
                unmatched.push(format!(
                    "{}:{}: only {} of {} analyzer reference(s) could be rewritten",
                    display(workspace_root, path),
                    line_number,
                    reads + writes,
                    columns.len()
                ));
                rewrite_failed = true;
                output.push_str(source_line);
                continue;
            }
            file_reads += reads;
            file_writes += writes;
            output.push_str(&changed);
            output.push_str(ending);
        }
        if ref_lines
            .keys()
            .any(|line| *line as usize > other_content.lines().count())
        {
            unmatched.push(format!(
                "{}: analyzer reference points beyond end of file",
                display(workspace_root, path)
            ));
            rewrite_failed = true;
        }
        if !rewrite_failed {
            total_reads += file_reads;
            total_writes += file_writes;
            rewritten.push((path.to_string_lossy().into_owned(), output));
        }
    }

    for entry in ignore::WalkBuilder::new(workspace_root).build().flatten() {
        let path = entry.path();
        if !path.is_file() || !lang.matches_extension(path) {
            continue;
        }
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if canonical == canonical_file {
            continue;
        }
        let Ok(other_content) = std::fs::read_to_string(path) else {
            continue;
        };
        let ref_lines = references_by_file.get(&canonical);
        for (index, line) in other_content.lines().enumerate() {
            if line.contains(&format!(".{field_name}"))
                || (lang == Language::Cpp && line.contains(&format!("->{field_name}")))
            {
                if ref_lines.is_none_or(|lines| !lines.contains_key(&(index as u32 + 1))) {
                    unmatched.push(format!(
                        "{}:{}: possible `{field_name}` access could not be matched to `{owner}`",
                        display(workspace_root, path),
                        index + 1
                    ));
                }
            }
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

    let mut applied = false;
    if apply && unmatched.is_empty() && (diagnostics.is_empty() || force) {
        let rewritten_map: BTreeMap<PathBuf, String> = rewritten
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.clone()))
            .collect();
        let edit = crate::signature::whole_file_edit(&rewritten_map);
        crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
        applied = true;
    }

    let rel_file = display(workspace_root, file_path);
    Ok(EncapsulatedField {
        owner,
        root: workspace_root.to_path_buf(),
        file: rel_file,
        field: field_name.to_string(),
        ty,
        by_value: by_value.unwrap_or(false),
        reads: total_reads,
        writes: total_writes,
        chained_reads: 0,
        left_in_file,
        blocked: vec![],
        unmatched,
        rewritten,
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
    fn field_at_line_col_uses_lsp_utf16_columns() {
        let text = "😀 const name: string = '';\n";
        assert_eq!(field_at_line_col(text, 1, 10).as_deref(), Some("name"));
        assert_eq!(field_at_line_col(text, 1, 2), None);
    }

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

    #[test]
    fn test_encapsulate_field_ts() {
        let ts = r#"export class UserService {
    public username: string;
    public age: number;

    constructor(username: string, age: number) {
        this.username = username;
        this.age = age;
    }
}

export function testUser(svc: UserService) {
    svc.username = "alice";
    console.log(svc.username);
}
"#;
        let (owner, ty, res, reads, writes, left) =
            encapsulate_field_ts(ts, Some("UserService"), "username").unwrap();
        assert_eq!(owner, "UserService");
        assert_eq!(ty, "string");
        assert_eq!(reads, 1);
        assert_eq!(writes, 1);
        assert_eq!(left, 1);
        assert!(res.contains("private _username: string;"));
        assert!(res.contains("public getUsername(): string {"));
        assert!(res.contains("return this._username;"));
        assert!(res.contains("public setUsername(username: string): void {"));
        assert!(res.contains("this._username = username;"));
        assert!(res.contains("svc.setUsername(\"alice\");"));
        assert!(res.contains("console.log(svc.getUsername());"));
    }

    #[test]
    fn test_encapsulate_field_js_generates_valid_javascript_accessors() {
        assert_eq!(
            Language::from_path(Path::new("user.js")),
            Some(Language::JavaScript)
        );
        assert_eq!(
            Language::from_path(Path::new("user.jsx")),
            Some(Language::JavaScript)
        );
        let js = r#"export class User {
    username = "";

    constructor(username) {
        this.username = username;
    }

    display() {
        return this.username;
    }
}

export function testUser(svc) {
    svc.username = "alice";
    console.log(svc.username);
}
"#;
        let (owner, ty, res, reads, writes, internal) =
            encapsulate_field_js(js, Some("User"), "username").unwrap();
        assert_eq!(owner, "User");
        assert!(ty.is_empty());
        assert_eq!(reads, 1);
        assert_eq!(writes, 1);
        assert_eq!(internal, 2);
        assert!(res.contains("#username = \"\";"));
        assert!(res.contains("getUsername() {"));
        assert!(res.contains("setUsername(username) {"));
        assert!(res.contains("this.#username = username;"));
        assert!(res.contains("svc.setUsername(\"alice\");"));
        assert!(res.contains("console.log(svc.getUsername());"));
        assert!(!res.contains("private _username"));
        assert!(!res.contains(": void"));
    }

    #[test]
    fn test_encapsulate_field_python() {
        let py = r#"class Account:
    def __init__(self, balance: float):
        self.balance = balance

    def deposit(self, amount: float):
        self.balance += amount

def audit_account(acc: Account):
    acc.balance = 100.0
    print(acc.balance)
"#;
        let (owner, _ty, res, reads, writes, left) =
            encapsulate_field_py(py, Some("Account"), "balance").unwrap();
        assert_eq!(owner, "Account");
        assert_eq!(reads, 1);
        assert_eq!(writes, 1);
        assert_eq!(left, 2);
        assert!(res.contains("self._balance = balance"));
        assert!(res.contains("self._balance += amount"));
        assert!(res.contains("def get_balance(self)"));
        assert!(res.contains("return self._balance"));
        assert!(res.contains("def set_balance(self, balance"));
        assert!(res.contains("acc.set_balance(100.0)"));
        assert!(res.contains("print(acc.get_balance())"));
    }

    #[test]
    fn test_encapsulate_field_cpp() {
        let cpp = r#"class User {
public:
    std::string name;
    int age;
};

void update_user(User* u) {
    u->name = "Alice";
    std::cout << u->name << std::endl;
}
"#;
        let (owner, ty, res, reads, writes, _left) =
            encapsulate_field_cpp(cpp, Some("User"), "name", None).unwrap();
        assert_eq!(owner, "User");
        assert_eq!(ty, "std::string");
        assert_eq!(reads, 1);
        assert_eq!(writes, 1);
        assert!(res.contains("const std::string& get_name() const"));
        assert!(res.contains("void set_name(const std::string& name)"));
        assert!(res.contains("std::string name_;"));
        assert!(res.contains("u->set_name(\"Alice\");"));
        assert!(res.contains("std::cout << u->get_name() << std::endl;"));
    }

    #[test]
    fn test_encapsulate_field_swift() {
        let swift = r#"class User {
    var name: String
    var age: Int

    init(name: String, age: Int) {
        self.name = name
        self.age = age
    }
}

func checkUser(u: User) {
    u.name = "Alice"
    print(u.name)
}
"#;
        let (owner, ty, res, reads, writes, left) =
            encapsulate_field_swift(swift, Some("User"), "name").unwrap();
        assert_eq!(owner, "User");
        assert_eq!(ty, "String");
        assert_eq!(reads, 1);
        assert_eq!(writes, 1);
        assert_eq!(left, 1);
        assert!(res.contains("private var _name: String"));
        assert!(res.contains("func getName() -> String"));
        assert!(res.contains("func setName(_ name: String)"));
        assert!(res.contains("u.setName(\"Alice\")"));
        assert!(res.contains("print(u.getName())"));
    }

    #[test]
    fn test_encapsulate_field_go() {
        let go = r#"package user

type User struct {
	Name string
	Age  int
}

func ProcessUser(u *User) {
	u.Name = "Alice"
	println(u.Name)
}
"#;
        let err = encapsulate_field_go(go, Some("User"), "Name").unwrap_err();
        assert!(
            err.to_string().contains("exported Go field `Name`"),
            "{err:#}"
        );
    }
}
