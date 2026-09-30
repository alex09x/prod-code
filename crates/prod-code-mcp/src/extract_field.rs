//! Promoting an expression inside a method into a field of the type the method belongs to.
//!
//! The expression leaves the method, which reads `self.field` instead, and the field is
//! initialised where the value is built: every `Type { … }` and `Self { … }` in the workspace
//! gets `field: <expression>`, so every construction site does what the method used to do on
//! each call. What can go wrong is where that expression has to be spelled: a construction site
//! cannot see the method's locals or `self`, and a pattern that lists every field no longer
//! matches a struct with one more. Both are reported rather than written.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use crate::parameter_object::Language;

/// What the extraction did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtractedField {
    /// The type the field was added to.
    pub owner: String,
    /// The method the expression came from.
    pub method: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub name: String,
    pub ty: String,
    /// What each construction site now initialises the field with.
    pub init: String,
    /// How many places in the method now read the field.
    pub replaced: usize,
    /// Construction sites given the initialiser.
    pub constructors: usize,
    /// Uses that stop compiling with one more field and cannot be rewritten, with the reason.
    pub blocked: Vec<String>,
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl ExtractedField {
    /// The report: where the value lives now, and whether the result compiles.
    pub fn render(&self, diff_budget: usize) -> String {
        let recv_read = if self.file.ends_with(".ts")
            || self.file.ends_with(".tsx")
            || self.file.ends_with(".js")
            || self.file.ends_with(".jsx")
        {
            format!("this.{}", self.name)
        } else if self.file.ends_with(".cpp")
            || self.file.ends_with(".cc")
            || self.file.ends_with(".cxx")
            || self.file.ends_with(".h")
            || self.file.ends_with(".hpp")
        {
            format!("this->{}", self.name)
        } else if self.file.ends_with(".go") {
            format!("r.{}", self.name)
        } else {
            format!("self.{}", self.name)
        };
        let field_decl = if self.ty.is_empty() {
            self.name.clone()
        } else {
            format!("{}: {}", self.name, self.ty)
        };
        let mut out = format!(
            "`{}.{}` ({})\n\n- new field: `{}`\n- `{}` now reads `{}` in {} place(s)\n- \
             {} construction site(s) initialise it with `{}`\n\n",
            self.owner,
            self.name,
            self.file,
            field_decl,
            self.method,
            recv_read,
            self.replaced,
            self.constructors,
            self.init
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
                "\n{} use(s) of `{}` stop compiling with one more field and cannot be rewritten:\n",
                self.blocked.len(),
                self.owner
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) this could not read):\n",
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
            out.push_str(
                "\nthe expression is now written at every construction site: if it names a \
                 local or a parameter of the method, it cannot be spelled there. Pass `init` \
                 with what a new value should start as.\n",
            );
        }
        if !self.ty.is_empty()
            && self.file.ends_with(".rs")
            && !crate::encapsulate_field::returns_by_value(&self.ty)
        {
            out.push_str(&format!(
                "\n`{}` is not one of the primitive `Copy` types: if it is not `Copy` at all, a \
                 place where the method used the expression by value now moves out of `self`, \
                 and the analyzer does not check that. Ask for `verify: \"compile\"` to be \
                 sure.\n",
                self.ty
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

/// The type an `impl` header is for: `impl<T> Trait for Wrapper<T> where …` gives `Wrapper`.
pub fn self_type(header: &str) -> Option<String> {
    let mut rest = header.trim_start();
    if let Some(generics) = rest.strip_prefix('<') {
        let mut depth = 1i32;
        let mut end = generics.len();
        for (i, c) in generics.char_indices() {
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
        rest = &generics[end..];
    }
    let rest = rest.split(" where").next().unwrap_or(rest);
    let ty = match rest.find(" for ") {
        Some(at) => &rest[at + " for ".len()..],
        None => rest,
    };
    let ty = ty.trim().trim_start_matches('&').trim_start_matches("mut ");
    let path = ty.split('<').next().unwrap_or(ty).trim();
    let name = path.rsplit("::").next().unwrap_or(path).trim();
    (!name.is_empty() && name.chars().all(is_ident)).then(|| name.to_string())
}

/// Every `impl` block in `text`: the type it is for, where `impl` starts, and its braces.
pub fn impl_blocks(text: &str) -> Vec<(String, usize, usize, usize)> {
    let mut out = Vec::new();
    for (at, _) in text.match_indices("impl") {
        let before = text[..at].trim_end_matches([' ', '\t']);
        let starts_item = before.is_empty()
            || before.ends_with('\n')
            || before.ends_with("unsafe")
            || before.ends_with('}');
        let after = &text[at + "impl".len()..];
        if !starts_item || !(after.starts_with('<') || after.starts_with(char::is_whitespace)) {
            continue;
        }
        let Some(open) = text[at..].find(['{', ';']).map(|i| at + i) else {
            continue;
        };
        if text.as_bytes()[open] != b'{' {
            continue;
        }
        let Some(ty) = self_type(&text[at + "impl".len()..open]) else {
            continue;
        };
        if let Some(close) = crate::parameter_object::matching_bracket(text, open) {
            out.push((ty, at, open, close));
        }
    }
    out
}

/// The method whose body contains `offset`, inside the `impl` braces `open..close`: its name,
/// its parameter list, and its body's braces.
pub fn method_at(
    text: &str,
    open: usize,
    close: usize,
    offset: usize,
) -> Option<(String, String, usize, usize)> {
    let mut best = None;
    for (at, _) in text[open..close].match_indices("fn ") {
        let at = open + at;
        if text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let name_at = at + "fn ".len();
        let Some((name, params_start, params_end)) = crate::signature::param_span(text, name_at)
        else {
            continue;
        };
        let Some(body_open) = text[params_end..close]
            .find(['{', ';'])
            .map(|i| params_end + i)
        else {
            continue;
        };
        if text.as_bytes()[body_open] != b'{' {
            continue;
        }
        let Some(body_close) = crate::parameter_object::matching_bracket(text, body_open) else {
            continue;
        };
        if body_open < offset && offset < body_close {
            best = Some((
                name,
                text[params_start..params_end].to_string(),
                body_open,
                body_close,
            ));
        }
    }
    best
}

/// The braces of the struct whose name starts at `name_at`, or `None` for a tuple or unit
/// struct.
pub fn struct_braces(text: &str, name_at: usize) -> Option<(usize, usize)> {
    let open = text[name_at..].find(['{', ';', '(']).map(|i| name_at + i)?;
    if text.as_bytes()[open] != b'{' {
        return None;
    }
    Some((open, crate::parameter_object::matching_bracket(text, open)?))
}

/// The edit that declares `decl` as the last field of the struct whose braces are `open..close`.
pub fn field_insertion(
    text: &str,
    open: usize,
    close: usize,
    decl: &str,
) -> (usize, usize, String) {
    let content_end = text[..close].trim_end().len();
    let first_field = text[open + 1..close]
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("//"));
    let indent: String = first_field
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_else(|| "    ".to_string());
    let comma = if content_end > open + 1 && !text[..content_end].ends_with(',') {
        ","
    } else {
        ""
    };
    let line_start = text[..close].rfind('\n').map_or(0, |i| i + 1);
    let closing_indent = &text[line_start..close];
    let closing_indent = if closing_indent.trim().is_empty() {
        closing_indent
    } else {
        ""
    };
    (
        content_end,
        close - content_end,
        format!("{comma}\n{indent}{decl},\n{closing_indent}"),
    )
}

/// What the braces opening at `open` are, read from what follows them: a pattern is followed
/// by `=>`, a single `=`, `|` or a `:` type ascription; anything else builds a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Braces {
    Literal,
    /// A pattern; `rest` when it ends in `..` and so matches a struct with more fields.
    Pattern {
        rest: bool,
    },
}

pub fn braces_kind(text: &str, open: usize, close: usize) -> Braces {
    // A bare `..` before the closing brace is a rest pattern; a literal's update syntax always
    // names the value it copies from (`..base`). That settles `matches!(x, Store { a, .. })`,
    // where nothing around the braces says which it is.
    let inner = text[open + 1..close].trim_end().trim_end_matches(',');
    let rest = inner.trim_end().ends_with("..");
    if rest || followed_by_pattern_cue(text, close + 1, 0) {
        return Braces::Pattern { rest };
    }
    Braces::Literal
}

/// Whether what follows `from` marks the text before it as a pattern: `=>`, a single `=`, `|`,
/// a `:` type ascription, `in` (a `for` loop) or `if` (a match guard). Braces nested in a
/// tuple, a slice, a variant or another struct — `Some(Store { a }) =>` — are a pattern when
/// the brackets around them are, so a `)`, `]`, `}` or `,` sends the question outward.
fn followed_by_pattern_cue(text: &str, from: usize, depth: usize) -> bool {
    let after = text[from..].trim_start();
    let word = |w: &str| {
        after.starts_with(w)
            && !after[w.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
    };
    if after.starts_with("=>")
        || (after.starts_with('=') && !after.starts_with("=="))
        || (after.starts_with('|') && !after.starts_with("||"))
        || (after.starts_with(':') && !after.starts_with("::"))
        || word("in")
        || word("if")
    {
        return true;
    }
    if depth < 8 && after.starts_with([')', ']', '}', ',']) {
        return enclosing_close(text, text.len() - after.len())
            .is_some_and(|close| followed_by_pattern_cue(text, close + 1, depth + 1));
    }
    false
}

/// The bracket that closes the group `at` sits in, skipping any group opened after it.
fn enclosing_close(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = at;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => i = crate::parameter_object::matching_bracket(text, i)?,
            b')' | b']' | b'}' => return Some(i),
            b'"' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                i = j;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The edit that initialises `field` first in the literal whose braces open at `open`.
pub fn literal_insertion(text: &str, open: usize, field_init: &str) -> (usize, usize, String) {
    let rest = &text[open + 1..];
    let same_line = rest.split('\n').next().unwrap_or("");
    if same_line.trim().is_empty() {
        // One field per line: the new one goes on its own line, indented like the next.
        let next = rest
            .lines()
            .skip(1)
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let indent: String = next.chars().take_while(|c| c.is_whitespace()).collect();
        return (open + 1, 0, format!("\n{indent}{field_init},"));
    }
    if rest.trim_start().starts_with('}') {
        return (open + 1, 0, format!(" {field_init} "));
    }
    (open + 1, 0, format!(" {field_init},"))
}

/// Whether the name at `at` begins a value built with braces rather than a type named in an
/// `impl` header, a return type or a bound. Returns the opening brace.
pub fn constructor_brace(text: &str, at: usize, name: &str) -> Option<usize> {
    if !text[at..].starts_with(name) || text[at + name.len()..].starts_with(is_ident) {
        return None;
    }
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let before = text[line_start..at].trim();
    if before.starts_with("impl")
        || before.contains(" impl ")
        || before.ends_with("->")
        || before.ends_with("for")
        || before.ends_with("struct")
        || before.ends_with("enum")
    {
        return None;
    }
    let mut i = at + name.len();
    let rest = &text[i..];
    if let Some(generics) = rest.strip_prefix("::<") {
        let mut depth = 1i32;
        let mut end = None;
        for (j, c) in generics.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(j + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        i += "::<".len() + end?;
    }
    let skipped = text[i..].len() - text[i..].trim_start().len();
    let open = i + skipped;
    (text.as_bytes().get(open) == Some(&b'{')).then_some(open)
}

/// The braces of every `Self { … }` between `open` and `close`, leaving out `-> Self {`, where
/// the brace is a function body.
pub fn self_literals(text: &str, open: usize, close: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for (at, _) in text[open..close].match_indices("Self") {
        let at = open + at;
        if text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if let Some(brace) = constructor_brace(text, at, "Self")
            && brace < close
        {
            out.push(brace);
        }
    }
    out
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn source_line(text: &str, at: usize) -> String {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    text[start..]
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Promotes the expression selected in `file` into a field of the type its method belongs to.
#[allow(clippy::too_many_arguments)]
pub async fn extract(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    start: (u32, u32),
    end: (u32, u32),
    name: &str,
    ty: Option<&str>,
    init: Option<&str>,
    replace_all: bool,
    apply: bool,
    force: bool,
) -> Result<ExtractedField> {
    anyhow::ensure!(
        !name.is_empty() && name.chars().all(is_ident),
        "`{name}` is not an identifier"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let from = crate::signature::offset_of(&text, start.0, start.1)
        .context("the selection does not start inside the file")?;
    let to = crate::signature::offset_of(&text, end.0, end.1)
        .context("the selection does not end inside the file")?;
    anyhow::ensure!(to > from, "the selection is empty");
    let expression = text[from..to].trim().to_string();
    anyhow::ensure!(!expression.is_empty(), "the selection is only whitespace");

    // The method, the `impl` it is in, and the type that `impl` is for.
    let (owner, impl_at, impl_open, impl_close) = impl_blocks(&text)
        .into_iter()
        .filter(|(_, _, open, close)| *open < from && from < *close)
        .min_by_key(|(_, _, open, close)| close - open)
        .context("the selection is not inside an `impl` block")?;
    let (method, params, body_open, body_close) = method_at(&text, impl_open, impl_close, from)
        .context("the selection is not inside a method")?;
    anyhow::ensure!(
        mentions(&params, "self"),
        "`{method}` takes no `self`, so it has no field to read; extract a parameter instead"
    );
    anyhow::ensure!(
        to <= body_close,
        "the selection runs past the end of `{method}`"
    );
    let init = match init {
        Some(init) => init.trim().to_string(),
        None => {
            anyhow::ensure!(
                !mentions(&expression, "self"),
                "`{expression}` reads `self`, which does not exist yet where `{owner}` is built; \
                 pass `init` with what a new value should start as"
            );
            expression.clone()
        }
    };
    let ty = ty
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .context("pass the field's `type`: the analyzer gives no type for an arbitrary expression in a shape this can read")?;

    // Where the type is declared.
    let owner_in_header = text[impl_at..impl_open]
        .rfind(owner.as_str())
        .map(|i| impl_at + i)
        .context("the `impl` header does not name its type")?;
    let (hl, hc) = crate::signature::position_at(&text, owner_in_header)?;
    let definition = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/definition",
        serde_json::json!({
            "textDocument": { "uri": url::Url::from_file_path(file)
                .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?.to_string() },
            "position": { "line": hl - 1, "character": hc - 1 },
        }),
    )
    .await?;
    let location = match &definition {
        serde_json::Value::Array(items) => items.first().cloned(),
        other if other.is_object() => Some(other.clone()),
        _ => None,
    }
    .with_context(|| format!("the analyzer does not know where `{owner}` is declared"))?;
    let def_uri = location
        .get("uri")
        .or_else(|| location.get("targetUri"))
        .and_then(|u| u.as_str())
        .context("the definition has no file")?;
    let def_path = PathBuf::from(crate::remote_fs::uri_to_path(def_uri));
    let range = location
        .get("range")
        .or_else(|| location.get("targetSelectionRange"))
        .context("the definition has no position")?;
    let def_line = range
        .pointer("/start/line")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32
        + 1;
    let def_col = range
        .pointer("/start/character")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32
        + 1;

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let def_text = texts
        .entry(def_path.clone())
        .or_insert_with(|| std::fs::read_to_string(&def_path).unwrap_or_default())
        .clone();
    let def_at = crate::signature::offset_of(&def_text, def_line, def_col)
        .context("the declaration is not where the analyzer put it")?;
    anyhow::ensure!(
        def_text[def_at..].starts_with(owner.as_str()),
        "the analyzer places `{owner}` at {}:{def_line}:{def_col}, but the file says otherwise",
        display(root, &def_path)
    );
    let (struct_open, struct_close) = struct_braces(&def_text, def_at)
        .with_context(|| format!("`{owner}` is not a struct with named fields"))?;
    anyhow::ensure!(
        !def_text[struct_open + 1..struct_close].lines().any(|l| l
            .trim_start()
            .trim_start_matches("pub ")
            .starts_with(&format!("{name}:"))),
        "`{owner}` already has a field `{name}`"
    );

    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    // The method reads the field where it used to compute the value.
    let mut replaced = 0usize;
    if replace_all {
        let mut at = body_open;
        while let Some(i) = text[at..body_close].find(&expression) {
            let hit = at + i;
            edits.entry(file.to_path_buf()).or_default().push((
                hit,
                expression.len(),
                format!("self.{name}"),
            ));
            replaced += 1;
            at = hit + expression.len();
        }
    } else {
        let lead = text[from..to].len() - text[from..to].trim_start().len();
        edits.entry(file.to_path_buf()).or_default().push((
            from + lead,
            expression.len(),
            format!("self.{name}"),
        ));
        replaced = 1;
    }
    // The struct declares it.
    edits
        .entry(def_path.clone())
        .or_default()
        .push(field_insertion(
            &def_text,
            struct_open,
            struct_close,
            &format!("{name}: {ty}"),
        ));

    // Every construction site initialises it.
    let field_init = format!("{name}: {init}");
    let mut constructors = 0usize;
    let mut blocked = Vec::new();
    let mut unmatched = Vec::new();
    let mut impl_files: Vec<PathBuf> = vec![def_path.clone(), file.to_path_buf()];
    let mut braces: Vec<(PathBuf, usize)> = Vec::new();
    let refs = crate::signature::references(remote, root, &def_path, def_line, def_col)
        .await
        .with_context(|| {
            format!("cannot find the construction sites of `{owner}`; nothing was planned")
        })?;
    for (path, rl, rc) in refs {
        let body = crate::refactor::referenced_text(&mut texts, &path)?;
        let at_site = format!("{}:{rl}:{rc}", display(root, &path));
        let Some(at) = crate::signature::offset_of(body, rl, rc) else {
            unmatched.push(format!("{at_site} (the position is not in the file)"));
            continue;
        };
        // Inside its own `impl`, the analyzer reports `Self` as a reference to the type too.
        let spelled = [owner.as_str(), "Self"].into_iter().find(|spelling| {
            body[at..].starts_with(spelling) && !body[at + spelling.len()..].starts_with(is_ident)
        });
        let Some(spelled) = spelled else {
            unmatched.push(format!(
                "{at_site} (the analyzer places `{owner}` here, but the file says otherwise)"
            ));
            continue;
        };
        if !impl_files.contains(&path) {
            impl_files.push(path.clone());
        }
        if let Some(open) = constructor_brace(body, at, spelled) {
            braces.push((path.clone(), open));
        }
    }
    for path in &impl_files {
        let body = crate::refactor::referenced_text(&mut texts, path)?.clone();
        for (ty_name, _, open, close) in impl_blocks(&body) {
            if ty_name == owner {
                braces.extend(
                    self_literals(&body, open, close)
                        .into_iter()
                        .map(|b| (path.clone(), b)),
                );
            }
        }
    }
    braces.sort();
    braces.dedup();
    for (path, open) in braces {
        let body = &texts[&path];
        let (line, col) = crate::signature::position_at(body, open)?;
        let Some(close) = crate::parameter_object::matching_bracket(body, open) else {
            unmatched.push(format!(
                "{}:{line}:{col} (a construction whose braces do not close)",
                display(root, &path)
            ));
            continue;
        };
        match braces_kind(body, open, close) {
            Braces::Literal => {
                edits
                    .entry(path.clone())
                    .or_default()
                    .push(literal_insertion(body, open, &field_init));
                constructors += 1;
            }
            Braces::Pattern { rest: true } => {}
            Braces::Pattern { rest: false } => blocked.push(format!(
                "{}:{line}:{col} a pattern that lists every field no longer matches: `{}`",
                display(root, &path),
                source_line(body, open)
            )),
        }
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
            "{} use(s) of `{owner}` stop compiling with one more field; nothing was written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
        );
        // A construction site left as it was is in a file nothing here checks; `force`
        // overrides the analyzer, not a site this did not read (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{owner}` were not read, so a construction there may lack the \
             field; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `init`, or \
             `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(ExtractedField {
        owner,
        method,
        root: root.to_path_buf(),
        file: display(root, file),
        name: name.to_string(),
        ty,
        init,
        replaced,
        constructors,
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

fn language_matches_extension(lang: Language, path: &Path) -> bool {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    match lang {
        Language::Rust => ext == "rs",
        Language::TypeScript => matches!(ext, "ts" | "tsx"),
        Language::JavaScript => matches!(ext, "js" | "jsx" | "mjs" | "cjs"),
        Language::Python => ext == "py",
        Language::Go => ext == "go",
        Language::Swift => ext == "swift",
        Language::Cpp => matches!(ext, "cpp" | "cc" | "cxx" | "hpp" | "h"),
        Language::C => matches!(ext, "c" | "h"),
        Language::Java => ext == "java",
    }
}

fn parse_params_python(params_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in params_str.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() || trimmed == "*" || trimmed == "/" {
            continue;
        }
        let name = trimmed.split([':', '=']).next().unwrap_or("").trim();
        let name = name.strip_prefix('*').unwrap_or(name).trim();
        let name = name.strip_prefix('*').unwrap_or(name).trim();
        if !name.is_empty() && name.chars().all(is_ident) && name != "self" {
            out.push(name.to_string());
        }
    }
    out
}

fn parse_params_ts(params_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in params_str.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let head = trimmed.split([':', '=']).next().unwrap_or("").trim();
        let name = head
            .trim_start_matches("public ")
            .trim_start_matches("private ")
            .trim_start_matches("protected ")
            .trim_start_matches("readonly ")
            .trim();
        if !name.is_empty() && name.chars().all(is_ident) && name != "this" {
            out.push(name.to_string());
        }
    }
    out
}

fn parse_params_go(params_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in params_str.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let name = trimmed.split_whitespace().next().unwrap_or("").trim();
        if !name.is_empty() && name.chars().all(is_ident) {
            out.push(name.to_string());
        }
    }
    out
}

fn parse_params_swift(params_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in params_str.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let head = trimmed.split(':').next().unwrap_or("").trim();
        let words: Vec<&str> = head.split_whitespace().collect();
        let name = words.last().copied().unwrap_or("");
        if !name.is_empty() && name.chars().all(is_ident) && name != "_" && name != "self" {
            out.push(name.to_string());
        }
    }
    out
}

fn parse_params_cpp(params_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in params_str.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let before_default = trimmed.split('=').next().unwrap_or("").trim();
        let name = before_default
            .split(['*', '&', ' '])
            .rfind(|s| !s.is_empty())
            .unwrap_or("");
        if !name.is_empty() && name.chars().all(is_ident) && name != "this" {
            out.push(name.to_string());
        }
    }
    out
}

fn parse_locals(text: &str, lang: Language) -> Vec<String> {
    let mut locals = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        match lang {
            Language::TypeScript | Language::JavaScript => {
                for prefix in ["const ", "let ", "var "] {
                    if let Some(after) = trimmed.strip_prefix(prefix) {
                        let name = after.split([':', '=', ' ', ';']).next().unwrap_or("").trim();
                        if !name.is_empty() && name.chars().all(is_ident) {
                            locals.push(name.to_string());
                        }
                    }
                }
            }
            Language::Python => {
                if !trimmed.starts_with('#')
                    && trimmed.contains('=')
                    && !trimmed.contains("==")
                    && !trimmed.starts_with("self.")
                {
                    let name = trimmed.split(['=', ':']).next().unwrap_or("").trim();
                    if !name.is_empty() && name.chars().all(is_ident) && name != "def" && name != "class" {
                        locals.push(name.to_string());
                    }
                }
            }
            Language::Go => {
                if let Some((before, _)) = trimmed.split_once(":=") {
                    let name = before.trim();
                    if !name.is_empty() && name.chars().all(is_ident) {
                        locals.push(name.to_string());
                    }
                }
            }
            Language::Swift => {
                for prefix in ["let ", "var "] {
                    if let Some(after) = trimmed.strip_prefix(prefix) {
                        let name = after.split([':', '=', ' ']).next().unwrap_or("").trim();
                        if !name.is_empty() && name.chars().all(is_ident) {
                            locals.push(name.to_string());
                        }
                    }
                }
            }
            Language::Cpp | Language::C => {
                if let Some((before, _)) = trimmed.split_once('=') {
                    let words: Vec<&str> = before.split_whitespace().collect();
                    if words.len() >= 2 {
                        let name = words
                            .last()
                            .copied()
                            .unwrap_or("")
                            .trim_start_matches('*')
                            .trim_start_matches('&');
                        if !name.is_empty() && name.chars().all(is_ident) {
                            locals.push(name.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    locals
}

fn has_member_named(text: &str, lang: Language, owner: &str, name: &str) -> bool {
    match lang {
        Language::Go => {
            let needle = format!("type {owner} struct");
            let Some(struct_at) = text.find(&needle) else { return false; };
            let Some(s_open) = text[struct_at..].find('{').map(|i| struct_at + i) else { return false; };
            let Some(s_close) = crate::parameter_object::matching_bracket(text, s_open) else { return false; };
            text[s_open + 1..s_close].lines().any(|l| {
                l.split_whitespace().next() == Some(name)
            })
        }
        Language::Python => {
            let needle = format!("class {owner}");
            let Some(c_at) = text.find(&needle) else { return false; };
            let lines: Vec<&str> = text[c_at..].lines().collect();
            if lines.is_empty() { return false; }
            let c_indent = lines[0].len() - lines[0].trim_start().len();
            for line in lines.into_iter().skip(1) {
                let trimmed = line.trim_start();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let ind = line.len() - trimmed.len();
                if ind <= c_indent {
                    break;
                }
                let t = trimmed;
                if t.starts_with(&format!("self.{name} ="))
                    || t.starts_with(&format!("self.{name}:"))
                    || t.starts_with(&format!("{name} ="))
                    || t.starts_with(&format!("{name}:"))
                    || t.starts_with(&format!("def {name}("))
                {
                    return true;
                }
            }
            false
        }
        Language::TypeScript | Language::JavaScript => {
            let needle = format!("class {owner}");
            let Some(c_at) = text.find(&needle) else { return false; };
            let Some(c_open) = text[c_at..].find('{').map(|i| c_at + i) else { return false; };
            let Some(c_close) = crate::parameter_object::matching_bracket(text, c_open) else { return false; };
            text[c_open + 1..c_close].lines().any(|l| {
                let t = l
                    .trim()
                    .trim_start_matches("public ")
                    .trim_start_matches("private ")
                    .trim_start_matches("protected ")
                    .trim_start_matches("readonly ")
                    .trim_start_matches("static ")
                    .trim();
                t.starts_with(&format!("{name}:"))
                    || t.starts_with(&format!("{name} ="))
                    || t.starts_with(&format!("{name}("))
                    || t.starts_with(&format!("{name}?:"))
            })
        }
        Language::Swift => {
            let mut c_open_opt = None;
            for kind in ["class ", "struct ", "actor "] {
                let needle = format!("{kind}{owner}");
                if let Some(c_at) = text.find(&needle)
                    && let Some(c_open) = text[c_at..].find('{').map(|i| c_at + i)
                {
                    c_open_opt = Some(c_open);
                    break;
                }
            }
            let Some(c_open) = c_open_opt else { return false; };
            let Some(c_close) = crate::parameter_object::matching_bracket(text, c_open) else { return false; };
            text[c_open + 1..c_close].lines().any(|l| {
                let t = l.trim();
                t.starts_with(&format!("var {name}"))
                    || t.starts_with(&format!("let {name}"))
                    || t.starts_with(&format!("func {name}("))
            })
        }
        Language::Cpp | Language::C => {
            let mut c_open_opt = None;
            for kind in ["class ", "struct "] {
                let needle = format!("{kind}{owner}");
                if let Some(c_at) = text.find(&needle)
                    && let Some(c_open) = text[c_at..].find('{').map(|i| c_at + i)
                {
                    c_open_opt = Some(c_open);
                    break;
                }
            }
            let Some(c_open) = c_open_opt else { return false; };
            let Some(c_close) = crate::parameter_object::matching_bracket(text, c_open) else { return false; };
            text[c_open + 1..c_close].lines().any(|l| {
                let t = l.trim();
                t.contains(&format!(" {name};"))
                    || t.contains(&format!(" {name} ="))
                    || t.contains(&format!(" {name}("))
                    || t.starts_with(&format!("{name};"))
                    || t.starts_with(&format!("{name} ="))
                    || t.starts_with(&format!("{name}("))
            })
        }
        Language::Rust | Language::Java => false,
    }
}

/// Promotes the expression selected in `file` into a field of the type its method belongs to
/// across TypeScript/JavaScript, Python, Go, Swift, and C++.
#[allow(clippy::too_many_arguments)]
pub async fn extract_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    start: (u32, u32),
    end: (u32, u32),
    name: &str,
    ty: Option<&str>,
    init: Option<&str>,
    replace_all: bool,
    apply: bool,
    force: bool,
) -> Result<ExtractedField> {
    anyhow::ensure!(
        !name.is_empty() && name.chars().all(is_ident),
        "`{name}` is not an identifier"
    );
    let lang = Language::of(file).with_context(|| format!("unsupported language for {}", file.display()))?;
    if lang == Language::Rust {
        return extract(
            remote,
            root,
            file,
            start,
            end,
            name,
            ty,
            init,
            replace_all,
            apply,
            force,
        )
        .await;
    }
    if lang == Language::Java {
        anyhow::bail!("extract_field does not support Java yet; use LSP or IDE assists");
    }

    let text = std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let from = crate::signature::offset_of(&text, start.0, start.1)
        .context("the selection does not start inside the file")?;
    let to = crate::signature::offset_of(&text, end.0, end.1)
        .context("the selection does not end inside the file")?;
    anyhow::ensure!(to > from, "the selection is empty");
    let expression = text[from..to].trim().to_string();
    anyhow::ensure!(!expression.is_empty(), "the selection is only whitespace");

    let (owner, method, receiver_name, params, body_open, body_close, class_body_open, class_close_line_start, init_body) = match lang {
        Language::Go => {
            let mut found = None;
            for (pos, _) in text.match_indices("func ") {
                let after = &text[pos + 5..];
                if !after.starts_with('(') {
                    continue;
                }
                let Some(recv_close) = after.find(')') else { continue; };
                let recv_slice = after[1..recv_close].trim();
                let parts: Vec<&str> = recv_slice.split_whitespace().collect();
                if parts.len() < 2 {
                    continue;
                }
                let r_name = parts[0];
                let o_name = parts[1].trim_start_matches('*').trim_start_matches('&');
                let after_recv = &after[recv_close + 1..];
                let Some(m_paren) = after_recv.find('(') else { continue; };
                let m_name = after_recv[..m_paren].trim();
                let Some(p_close) = crate::parameter_object::matching_bracket(after_recv, m_paren) else { continue; };
                let p_slice = &after_recv[m_paren + 1..p_close];
                let Some(b_open_rel) = after_recv[p_close..].find('{') else { continue; };
                let b_open = pos + 5 + recv_close + 1 + p_close + b_open_rel;
                let Some(b_close) = crate::parameter_object::matching_bracket(&text, b_open) else { continue; };
                if b_open < from && to <= b_close {
                    found = Some((o_name.to_string(), m_name.to_string(), r_name.to_string(), parse_params_go(p_slice), b_open, b_close));
                    break;
                }
            }
            let (o_name, m_name, r_name, p_list, b_open, b_close) = found.context("selection is not inside a Go method with a receiver")?;

            let needle = format!("type {o_name} struct");
            let struct_at = text.find(&needle).with_context(|| format!("cannot find struct declaration for `{o_name}`"))?;
            let s_open = text[struct_at..].find('{').map(|i| struct_at + i).context("struct has no `{`")?;
            let s_close = crate::parameter_object::matching_bracket(&text, s_open).context("struct `{` does not close")?;
            let s_close_line_start = text[..s_close].rfind('\n').map_or(0, |i| i + 1);

            (o_name, m_name, r_name, p_list, b_open, b_close, s_open, s_close_line_start, None)
        }
        Language::Python => {
            let lines: Vec<&str> = text.lines().collect();
            let mut line_offsets = Vec::new();
            let mut off = 0;
            for l in &lines {
                line_offsets.push(off);
                off += l.len() + 1;
            }
            let sel_line_idx = lines
                .iter()
                .enumerate()
                .position(|(idx, _)| {
                    let start = line_offsets[idx];
                    let end = start + lines[idx].len();
                    start <= from && from <= end
                })
                .context("cannot find line of selection")?;

            let mut def_line_idx = None;
            for idx in (0..=sel_line_idx).rev() {
                let trimmed = lines[idx].trim_start();
                if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
                    def_line_idx = Some(idx);
                    break;
                }
            }
            let def_idx = def_line_idx.context("selection is not inside a Python function/method")?;
            let def_line = lines[def_idx];
            let def_indent = def_line.len() - def_line.trim_start().len();
            let def_head = def_line.trim_start().strip_prefix("async ").unwrap_or(def_line.trim_start());
            let m_name = def_head
                .strip_prefix("def ")
                .unwrap()
                .split('(')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            let p_start = def_line.find('(').context("method has no `(`")?;
            let p_end = def_line.rfind(')').context("method has no `)`")?;
            let p_slice = &def_line[p_start + 1..p_end];
            let p_list = parse_params_python(p_slice);
            anyhow::ensure!(
                def_line[p_start + 1..p_end].trim().starts_with("self"),
                "`{m_name}` takes no `self`, so it has no field to read; extract a parameter instead"
            );

            let b_open = line_offsets[def_idx] + def_line.find(':').context("def line has no `:`")? + 1;
            let mut b_close = text.len();
            for idx in def_idx + 1..lines.len() {
                let l = lines[idx];
                if l.trim().is_empty() || l.trim_start().starts_with('#') {
                    continue;
                }
                let ind = l.len() - l.trim_start().len();
                if ind <= def_indent {
                    b_close = line_offsets[idx];
                    break;
                }
            }
            anyhow::ensure!(b_open < from && to <= b_close, "selection runs past the end of `{m_name}`");

            let mut class_line_idx = None;
            for idx in (0..def_idx).rev() {
                let trimmed = lines[idx].trim_start();
                if trimmed.starts_with("class ") {
                    let ind = lines[idx].len() - trimmed.len();
                    if ind < def_indent {
                        class_line_idx = Some(idx);
                        break;
                    }
                }
            }
            let c_idx = class_line_idx.context("method is not inside a class")?;
            let c_line = lines[c_idx];
            let o_name = c_line
                .trim_start()
                .strip_prefix("class ")
                .unwrap()
                .split(['(', ':'])
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            let c_colon = line_offsets[c_idx] + c_line.find(':').context("class line has no `:`")? + 1;

            let mut init_info = None;
            for idx in c_idx + 1..lines.len() {
                let l = lines[idx];
                let trimmed = l.trim_start();
                let ind = l.len() - trimmed.len();
                if !trimmed.is_empty() && !trimmed.starts_with('#') && ind <= (c_line.len() - c_line.trim_start().len()) {
                    break;
                }
                if trimmed.starts_with("def __init__(") {
                    let init_indent = ind;
                    let init_start = line_offsets[idx] + l.find(':').unwrap_or(0) + 1;
                    let mut init_end = line_offsets[idx] + l.len();
                    for j in idx + 1..lines.len() {
                        let jl = lines[j];
                        if jl.trim().is_empty() || jl.trim_start().starts_with('#') {
                            continue;
                        }
                        let jind = jl.len() - jl.trim_start().len();
                        if jind <= init_indent {
                            init_end = line_offsets[j];
                            break;
                        }
                        init_end = line_offsets[j] + jl.len();
                    }
                    init_info = Some((init_start, init_end));
                    break;
                }
            }

            (o_name, m_name, "self".to_string(), p_list, b_open, b_close, c_colon, c_colon, init_info)
        }
        Language::TypeScript | Language::JavaScript => {
            let mut found_class = None;
            for (pos, _) in text.match_indices("class ") {
                if pos > 0 && is_ident(text[..pos].chars().next_back().unwrap()) {
                    continue;
                }
                let after = &text[pos + 6..];
                let o_name = after.split(['{', ' ', '\n', '<']).next().unwrap_or("").trim();
                let Some(c_open) = text[pos..].find('{').map(|i| pos + i) else { continue; };
                let Some(c_close) = crate::parameter_object::matching_bracket(&text, c_open) else { continue; };
                if c_open < from && to <= c_close {
                    found_class = Some((o_name.to_string(), c_open, c_close));
                    break;
                }
            }
            let (o_name, c_open, c_close) = found_class.context("selection is not inside a class")?;

            let mut found_method = None;
            let class_inner = &text[c_open + 1..c_close];
            let mut cur = 0;
            while let Some(rel_open) = class_inner[cur..].find('{') {
                let b_open = c_open + 1 + cur + rel_open;
                cur += rel_open + 1;
                let Some(b_close) = crate::parameter_object::matching_bracket(&text, b_open) else { continue; };
                if b_open < from && to <= b_close {
                    let before_body = text[c_open + 1..b_open].trim_end();
                    let last_paren = before_body.rfind(')').context("method has no `)`")?;
                    let first_paren = before_body[..last_paren].rfind('(').context("method has no `(`")?;
                    let p_slice = &before_body[first_paren + 1..last_paren];
                    let head = before_body[..first_paren].trim_end();
                    let m_name = head.split_whitespace().last().unwrap_or("").trim();
                    found_method = Some((m_name.to_string(), parse_params_ts(p_slice), b_open, b_close));
                    break;
                }
            }
            let (m_name, p_list, b_open, b_close) = found_method.context("selection is not inside a method")?;
            (o_name, m_name, "this".to_string(), p_list, b_open, b_close, c_open, c_close, None)
        }
        Language::Swift => {
            let mut found_type = None;
            for kind in ["class ", "struct ", "actor "] {
                for (pos, _) in text.match_indices(kind) {
                    if pos > 0 && is_ident(text[..pos].chars().next_back().unwrap()) {
                        continue;
                    }
                    let after = &text[pos + kind.len()..];
                    let o_name = after.split(['{', ' ', '\n', ':', '<']).next().unwrap_or("").trim();
                    let Some(t_open) = text[pos..].find('{').map(|i| pos + i) else { continue; };
                    let Some(t_close) = crate::parameter_object::matching_bracket(&text, t_open) else { continue; };
                    if t_open < from && to <= t_close {
                        found_type = Some((o_name.to_string(), t_open, t_close));
                        break;
                    }
                }
                if found_type.is_some() { break; }
            }
            let (o_name, t_open, t_close) = found_type.context("selection is not inside a class or struct")?;

            let mut found_func = None;
            let type_inner = &text[t_open + 1..t_close];
            for (pos, _) in type_inner.match_indices("func ") {
                let func_abs = t_open + 1 + pos;
                if func_abs > 0 && is_ident(text[..func_abs].chars().next_back().unwrap()) {
                    continue;
                }
                let after = &text[func_abs + 5..];
                let Some(p_open) = after.find('(') else { continue; };
                let m_name = after[..p_open].trim();
                let Some(p_close) = crate::parameter_object::matching_bracket(after, p_open) else { continue; };
                let p_slice = &after[p_open + 1..p_close];
                let Some(b_open_rel) = after[p_close..].find('{') else { continue; };
                let b_open = func_abs + 5 + p_close + b_open_rel;
                let Some(b_close) = crate::parameter_object::matching_bracket(&text, b_open) else { continue; };
                if b_open < from && to <= b_close {
                    found_func = Some((m_name.to_string(), parse_params_swift(p_slice), b_open, b_close));
                    break;
                }
            }
            let (m_name, p_list, b_open, b_close) = found_func.context("selection is not inside a Swift method")?;
            (o_name, m_name, "self".to_string(), p_list, b_open, b_close, t_open, t_close, None)
        }
        Language::Cpp | Language::C => {
            let mut found_type = None;
            for kind in ["class ", "struct "] {
                for (pos, _) in text.match_indices(kind) {
                    if pos > 0 && is_ident(text[..pos].chars().next_back().unwrap()) {
                        continue;
                    }
                    let after = &text[pos + kind.len()..];
                    let o_name = after.split(['{', ' ', '\n', ':', ';']).next().unwrap_or("").trim();
                    let Some(t_open) = text[pos..].find('{').map(|i| pos + i) else { continue; };
                    let Some(t_close) = crate::parameter_object::matching_bracket(&text, t_open) else { continue; };
                    if t_open < from && to <= t_close {
                        found_type = Some((o_name.to_string(), t_open, t_close));
                        break;
                    }
                }
                if found_type.is_some() { break; }
            }
            let (o_name, t_open, t_close) = found_type.context("selection is not inside a C++ class or struct")?;

            let mut found_method = None;
            let class_inner = &text[t_open + 1..t_close];
            let mut cur = 0;
            while let Some(rel_open) = class_inner[cur..].find('{') {
                let b_open = t_open + 1 + cur + rel_open;
                cur += rel_open + 1;
                let Some(b_close) = crate::parameter_object::matching_bracket(&text, b_open) else { continue; };
                if b_open < from && to <= b_close {
                    let before_body = text[t_open + 1..b_open].trim_end();
                    let last_paren = before_body.rfind(')').context("method has no `)`")?;
                    let first_paren = before_body[..last_paren].rfind('(').context("method has no `(`")?;
                    let p_slice = &before_body[first_paren + 1..last_paren];
                    let head = before_body[..first_paren].trim_end();
                    let m_name = head
                        .split_whitespace()
                        .last()
                        .unwrap_or("")
                        .trim()
                        .trim_start_matches('*')
                        .trim_start_matches('&');
                    found_method = Some((m_name.to_string(), parse_params_cpp(p_slice), b_open, b_close));
                    break;
                }
            }
            let (m_name, p_list, b_open, b_close) = found_method.context("selection is not inside a C++ method")?;
            let t_close_line_start = text[..t_close].rfind('\n').map_or(0, |i| i + 1);
            (o_name, m_name, "this".to_string(), p_list, b_open, b_close, t_open, t_close_line_start, None)
        }
        Language::Rust | Language::Java => unreachable!(),
    };

    let recv_kw = match lang {
        Language::TypeScript | Language::JavaScript | Language::Cpp | Language::C | Language::Java => "this",
        Language::Go => &receiver_name,
        _ => "self",
    };
    let init = match init {
        Some(init) => init.trim().to_string(),
        None => {
            anyhow::ensure!(
                !mentions(&expression, recv_kw),
                "`{expression}` reads `{recv_kw}`, which does not exist yet where `{owner}` is built; \
                 pass `init` with what a new value should start as"
            );
            expression.clone()
        }
    };

    for param in &params {
        anyhow::ensure!(
            !mentions(&init, param),
            "`{expression}` mentions parameter `{param}`, which does not exist where `{owner}` is built; \
             pass `init` with what a new value should start as"
        );
    }

    let locals = parse_locals(&text[body_open..from], lang);
    for local in &locals {
        anyhow::ensure!(
            !mentions(&init, local),
            "`{expression}` mentions local `{local}`, which does not exist where `{owner}` is built; \
             pass `init` with what a new value should start as"
        );
    }

    anyhow::ensure!(
        !has_member_named(&text, lang, &owner, name),
        "`{owner}` already has a field `{name}`"
    );

    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());

    let (insert_offset, insert_text) = match lang {
        Language::TypeScript | Language::JavaScript => {
            let line_after_open = text[class_body_open + 1..]
                .find('\n')
                .map(|i| class_body_open + 1 + i + 1)
                .unwrap_or(class_body_open + 1);
            let decl = match lang {
                Language::TypeScript => {
                    if let Some(t) = ty {
                        format!("    {name}: {t} = {init};\n")
                    } else {
                        format!("    {name} = {init};\n")
                    }
                }
                _ => format!("    {name} = {init};\n"),
            };
            (line_after_open, decl)
        }
        Language::Python => {
            if let Some((_, init_end)) = init_body {
                (init_end, format!("\n        self.{name} = {init}"))
            } else {
                let decl = if let Some(t) = ty {
                    format!("    {name}: {t} = {init}\n")
                } else {
                    format!("    {name} = {init}\n")
                };
                (class_body_open, format!("\n{decl}"))
            }
        }
        Language::Go => {
            let ty_str = ty.context("pass the field's `type`: Go requires a type for struct field declarations")?;
            (class_close_line_start, format!("    {name} {ty_str}\n"))
        }
        Language::Swift => {
            let line_after_open = text[class_body_open + 1..]
                .find('\n')
                .map(|i| class_body_open + 1 + i + 1)
                .unwrap_or(class_body_open + 1);
            let decl = if let Some(t) = ty {
                format!("    var {name}: {t} = {init}\n")
            } else {
                format!("    var {name} = {init}\n")
            };
            (line_after_open, decl)
        }
        Language::Cpp | Language::C => {
            let ty_str = ty.context("pass the field's `type`: C++ requires a type for member declarations")?;
            (class_close_line_start, format!("    {ty_str} {name} = {init};\n"))
        }
        Language::Rust | Language::Java => unreachable!(),
    };

    let own_edits = edits.entry(file.to_path_buf()).or_default();
    own_edits.push((insert_offset, 0, insert_text));

    let recv_expr = match lang {
        Language::TypeScript | Language::JavaScript | Language::Java => format!("this.{name}"),
        Language::Python | Language::Swift => format!("self.{name}"),
        Language::Cpp | Language::C => format!("this->{name}"),
        Language::Go => format!("{receiver_name}.{name}"),
        Language::Rust => format!("self.{name}"),
    };

    let mut replaced = 0usize;
    if replace_all {
        let mut at = body_open;
        while let Some(i) = text[at..body_close].find(&expression) {
            let hit = at + i;
            own_edits.push((hit, expression.len(), recv_expr.clone()));
            replaced += 1;
            at = hit + expression.len();
        }
    } else {
        let lead = text[from..to].len() - text[from..to].trim_start().len();
        own_edits.push((from + lead, expression.len(), recv_expr));
        replaced = 1;
    }

    let mut constructors = 0usize;
    if lang == Language::Go {
        for entry in ignore::WalkBuilder::new(root).build().flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("go") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            let needle = format!("{owner}{{");
            if !content.contains(&needle) {
                continue;
            }
            let mut file_edits = Vec::new();
            let mut cur = 0;
            while let Some(pos) = content[cur..].find(&needle) {
                let hit = cur + pos;
                cur = hit + needle.len();
                if hit > 0 && is_ident(content[..hit].chars().next_back().unwrap()) {
                    continue;
                }
                let open = hit + owner.len();
                let Some(close) = crate::parameter_object::matching_bracket(&content, open) else {
                    continue;
                };
                let inside = &content[open + 1..close];
                if mentions(inside, &format!("{name}:")) {
                    continue;
                }
                let is_multiline = inside.contains('\n');
                let insertion = if is_multiline {
                    let indent = inside
                        .lines()
                        .skip(1)
                        .find(|l| !l.trim().is_empty())
                        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect::<String>())
                        .unwrap_or_else(|| "        ".to_string());
                    format!("\n{indent}{name}: {init},")
                } else if inside.trim().is_empty() {
                    format!("{name}: {init}")
                } else {
                    format!("{name}: {init}, ")
                };
                file_edits.push((open + 1, 0, insertion));
                constructors += 1;
            }
            if !file_edits.is_empty() {
                texts.entry(path.to_path_buf()).or_insert_with(|| content.clone());
                edits.entry(path.to_path_buf()).or_default().extend(file_edits);
            }
        }
    } else {
        for entry in ignore::WalkBuilder::new(root).build().flatten() {
            let path = entry.path();
            if !path.is_file() || !language_matches_extension(lang, path) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            match lang {
                Language::TypeScript | Language::JavaScript => {
                    let needle = format!("new {owner}(");
                    constructors += content.matches(&needle).count();
                }
                Language::Python | Language::Swift => {
                    let needle = format!("{owner}(");
                    for (at, _) in content.match_indices(&needle) {
                        if at > 0 && is_ident(content[..at].chars().next_back().unwrap()) {
                            continue;
                        }
                        constructors += 1;
                    }
                }
                Language::Cpp | Language::C => {
                    let needle_call = format!("{owner}(");
                    let needle_brace = format!("{owner}{{");
                    for (at, _) in content.match_indices(&needle_call).chain(content.match_indices(&needle_brace)) {
                        if at > 0 && is_ident(content[..at].chars().next_back().unwrap()) {
                            continue;
                        }
                        constructors += 1;
                    }
                }
                _ => {}
            }
        }
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
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `init`, or \
             `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(ExtractedField {
        owner,
        method,
        root: root.to_path_buf(),
        file: display(root, file),
        name: name.to_string(),
        ty: ty.unwrap_or("").to_string(),
        init,
        replaced,
        constructors,
        blocked: Vec::new(),
        unmatched: Vec::new(),
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
    fn the_type_an_impl_is_for_is_the_last_segment_of_its_path() {
        assert_eq!(self_type(" Store ").as_deref(), Some("Store"));
        assert_eq!(
            self_type("<T> Page<T> where T: Clone ").as_deref(),
            Some("Page")
        );
        assert_eq!(
            self_type("<'a> fmt::Display for crate::store::Store<'a> ").as_deref(),
            Some("Store")
        );
        assert_eq!(
            self_type(" Trait for &mut Store ").as_deref(),
            Some("Store")
        );
        assert_eq!(self_type("  ").as_deref(), None);
    }

    const SOURCE: &str = "pub struct Store {\n    entries: Vec<u32>,\n}\n\nimpl Store {\n    pub fn new() -> Self {\n        Self { entries: Vec::new() }\n    }\n\n    // don't count this: fn fake() {}\n    pub fn limit(&self) -> usize {\n        let cap = 64 * 1024;\n        cap.min(self.entries.len())\n    }\n}\n\nimpl Default for Store {\n    fn default() -> Store {\n        Store::new()\n    }\n}\n";

    #[test]
    fn impl_blocks_and_the_method_around_a_position_are_found_by_their_braces() {
        let blocks = impl_blocks(SOURCE);
        assert_eq!(blocks.len(), 2);
        assert!(blocks.iter().all(|(ty, ..)| ty == "Store"));
        let (_, _, open, close) = blocks[0];
        let at = SOURCE.find("64 * 1024").unwrap();
        let (name, params, body_open, body_close) = method_at(SOURCE, open, close, at).unwrap();
        assert_eq!(name, "limit");
        assert_eq!(params, "&self");
        assert!(body_open < at && at < body_close);
        assert!(method_at(SOURCE, open, close, open + 1).is_none());
        assert!(impl_blocks("fn implement() {}\nimpl Missing;\n").is_empty());
    }

    #[test]
    fn a_new_field_goes_last_and_keeps_the_shape_of_the_list() {
        let open = SOURCE.find('{').unwrap();
        let close = crate::parameter_object::matching_bracket(SOURCE, open).unwrap();
        let (at, len, text) = field_insertion(SOURCE, open, close, "cap: usize");
        let mut out = SOURCE.to_string();
        out.replace_range(at..at + len, &text);
        assert!(
            out.starts_with("pub struct Store {\n    entries: Vec<u32>,\n    cap: usize,\n}\n"),
            "{out}"
        );

        let bare = "struct A {\n    a: u8\n}";
        let (at, len, text) = field_insertion(bare, 9, bare.len() - 1, "b: u8");
        let mut out = bare.to_string();
        out.replace_range(at..at + len, &text);
        assert_eq!(out, "struct A {\n    a: u8,\n    b: u8,\n}");

        let empty = "struct A {}";
        let (at, len, text) = field_insertion(empty, 9, 10, "b: u8");
        let mut out = empty.to_string();
        out.replace_range(at..at + len, &text);
        assert_eq!(out, "struct A {\n    b: u8,\n}");
    }

    fn kind(text: &str) -> Braces {
        let open = text.find('{').unwrap();
        let close = crate::parameter_object::matching_bracket(text, open).unwrap();
        braces_kind(text, open, close)
    }

    #[test]
    fn braces_are_a_pattern_when_something_is_matched_against_them() {
        assert_eq!(kind("let s = Store { entries };"), Braces::Literal);
        assert_eq!(kind("f(Store { entries }, 1)"), Braces::Literal);
        assert_eq!(kind("x == Store { entries }"), Braces::Literal);
        assert_eq!(
            kind("let Store { entries } = s;"),
            Braces::Pattern { rest: false }
        );
        assert_eq!(
            kind("Store { entries, .. } => 1,"),
            Braces::Pattern { rest: true }
        );
        assert_eq!(
            kind("Store { .. } | Other => 1,"),
            Braces::Pattern { rest: true }
        );
        assert_eq!(
            kind("fn f(Store { entries }: Store) {}"),
            Braces::Pattern { rest: false }
        );
        for (text, rest) in [
            ("for Store { entries } in all {}", false),
            ("Store { entries } if entries.is_empty() => 1,", false),
            ("Some(Store { entries }) => 1,", false),
            ("(Store { entries, .. }, 2) => 1,", true),
            ("let [Store { entries }] = all;", false),
            ("assert!(matches!(s, Store { entries, .. }));", true),
        ] {
            assert_eq!(kind(text), Braces::Pattern { rest }, "{text}");
        }
        for text in [
            "f(Store { entries }, g(1));",
            "let v = vec![Store { entries }];",
            "Outer { s: Store { entries }, n: 1 }",
            "let s = Store { entries }.into_inner();",
            "x => Store { entries },",
            "Store { entries, ..Store::new() }",
        ] {
            assert_eq!(kind(text), Braces::Literal, "{text}");
        }
    }

    #[test]
    fn a_literal_gets_the_field_first_in_its_own_shape() {
        let one_line = "Self { entries: Vec::new() }";
        let (at, len, text) = literal_insertion(one_line, 5, "cap: 64");
        let mut out = one_line.to_string();
        out.replace_range(at..at + len, &text);
        assert_eq!(out, "Self { cap: 64, entries: Vec::new() }");

        let lines = "Store {\n            entries,\n        }";
        let (at, len, text) = literal_insertion(lines, 6, "cap: 64");
        let mut out = lines.to_string();
        out.replace_range(at..at + len, &text);
        assert_eq!(
            out,
            "Store {\n            cap: 64,\n            entries,\n        }"
        );

        let empty = "Unit {}";
        let (at, len, text) = literal_insertion(empty, 5, "cap: 64");
        let mut out = empty.to_string();
        out.replace_range(at..at + len, &text);
        assert_eq!(out, "Unit { cap: 64 }");
    }

    #[test]
    fn only_a_name_followed_by_braces_that_build_a_value_is_a_constructor() {
        let at = |t: &str| t.find("Store").unwrap();
        let t = "let s = Store { entries };";
        assert_eq!(
            constructor_brace(t, at(t), "Store"),
            Some(t.find('{').unwrap())
        );
        let t = "let s = Store::<u8> { entries };";
        assert_eq!(
            constructor_brace(t, at(t), "Store"),
            Some(t.find('{').unwrap())
        );
        for t in [
            "impl Store {",
            "impl Default for Store {",
            "fn new() -> Store {",
            "pub struct Store {",
            "let s: Store = x;",
            "Store::new()",
            "StoreKey { a }",
        ] {
            assert_eq!(constructor_brace(t, at(t), "Store"), None, "{t}");
        }
        let blocks = impl_blocks(SOURCE);
        let (_, _, open, close) = blocks[0];
        let found = self_literals(SOURCE, open, close);
        assert_eq!(found, vec![SOURCE.find("Self { entries").unwrap() + 5]);
    }

    #[test]
    fn a_word_is_mentioned_only_whole() {
        assert!(mentions("&self", "self"));
        assert!(mentions("self.x + 1", "self"));
        assert!(!mentions("myself.x", "self"));
        assert!(!mentions("selfish", "self"));
    }

    fn report() -> ExtractedField {
        ExtractedField {
            owner: "Store".into(),
            method: "limit".into(),
            root: "/root".into(),
            file: "src/store.rs".into(),
            name: "cap".into(),
            ty: "Vec<u8>".into(),
            init: "64 * 1024".into(),
            replaced: 1,
            constructors: 2,
            blocked: vec!["src/app.rs:4:9 a pattern that lists every field no longer matches: `let Store { entries } = s;`".into()],
            unmatched: vec!["src/app.rs:1:5 (the analyzer places `Store` here, but the file says otherwise)".into()],
            rewritten: vec![("/root/src/store.rs".into(), "fn main() {}\n".into())],
            diagnostics: vec!["cannot find value `n` (src/app.rs:9:12)".into()],
            applied: false,
        }
    }

    #[test]
    fn the_report_says_where_the_value_went_and_what_was_left() {
        let text = report().render(10_000);
        assert!(text.contains("new field: `cap: Vec<u8>`"), "{text}");
        assert!(
            text.contains("`limit` now reads `self.cap` in 1 place(s)"),
            "{text}"
        );
        assert!(
            text.contains("2 construction site(s) initialise it with `64 * 1024`"),
            "{text}"
        );
        assert!(
            text.contains("stop compiling with one more field"),
            "{text}"
        );
        assert!(text.contains("this could not read"), "{text}");
        assert!(text.contains("Pass `init`"), "{text}");
        assert!(
            text.contains("not one of the primitive `Copy` types"),
            "{text}"
        );
        assert!(text.contains("nothing was written"), "{text}");

        let mut done = report();
        done.ty = "usize".into();
        done.blocked.clear();
        done.unmatched.clear();
        done.diagnostics.clear();
        done.applied = true;
        let text = done.render(10);
        assert!(text.contains("0 errors"), "{text}");
        assert!(!text.contains("Copy"), "{text}");
        assert!(text.contains("diff truncated"), "{text}");
        assert!(text.contains("[applied to 1 file(s)]"), "{text}");
    }
}
