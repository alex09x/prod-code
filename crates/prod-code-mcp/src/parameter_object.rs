//! Bundling a function's parameters into a struct, with the body and the call sites.
//!
//! Three edits that have to agree: a new type, a declaration whose parameters became fields and
//! a body that now reaches them through one name, and every call site passing a literal.
//!
//! The analyzer says where the function is declared and where it is used; the text says what
//! each of those places should become. It cannot be done with the structural engine
//! [`crate::signature`] uses, because that resolves the paths in its replacement and the type
//! this writes does not exist yet. All of it is type-checked in one overlay before a byte is
//! written.
//!
//! The same three edits are made in TypeScript, Python and Go, where the new type is an
//! `interface`, a `@dataclass` (or a plain class when the parameters carry no types) and a
//! `struct`. What differs between the languages is text — how a parameter list is written, how
//! a literal of the new type is spelled, where a string or a comment starts — and that is
//! chosen by the declaring file's language. Rust keeps its own path, which predates the others.
//!
//! Swift takes the same path, with a `struct` of `let` properties whose memberwise initialiser
//! is the literal, and argument labels that bind the arguments to the parameters. C and C++
//! take one of their own, because a function there is usually declared more than once — a
//! prototype in a header, the definition with the body — and every declaration has to change
//! together, with the new type in the header so that the definition and every caller see it
//! through an include they already have.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What bundling did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ParameterObject {
    pub symbol: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    /// The type that was generated, as it will be written.
    pub struct_text: String,
    pub was: String,
    pub now: String,
    /// How many call sites were rewritten.
    pub call_sites: usize,
    /// One line per import a call site in another module needed.
    pub imports: Vec<String>,
    /// How many uses of the bundled parameters the body had.
    pub body_uses: usize,
    pub rewritten: Vec<(String, String)>,
    /// References the rule did not match, named rather than guessed at.
    pub unmatched: Vec<String>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
    /// The language the declaration is written in, as the report's code block names it.
    #[serde(skip)]
    pub language: &'static str,
}

impl ParameterObject {
    /// The report: the new type, what the declaration became, and whether it compiles.
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: ({})\n- now: ({})\n- {} call site(s) rewritten, {} use(s) in \
             the body\n\n```{}\n{}\n```\n\n",
            self.symbol,
            self.file,
            self.was,
            self.now,
            self.call_sites,
            self.body_uses,
            self.language,
            self.struct_text
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
        if !self.imports.is_empty() {
            out.push_str("\nimports:\n");
            for note in &self.imports {
                out.push_str(&format!("  {note}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            // Only Rust has macros and function pointers; elsewhere the same thing is a function
            // passed around as a value.
            let usual = if self.language == "rust" {
                "a function pointer, a macro, or a call already changed"
            } else {
                "the function passed as a value, or a call already changed"
            };
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) that are not a call with the arity this \
                 declaration has — {usual}):\n",
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

/// The type of a parameter as the declaration writes it: everything after the first top-level
/// colon, trimmed.
pub fn type_of(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' | b')' | b']' => depth -= 1,
            b':' if depth == 0 => {
                // `::` is a path separator, not the end of the name.
                if bytes.get(i + 1) == Some(&b':') {
                    i += 2;
                    continue;
                }
                return Some(raw[i + 1..].trim());
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether a type borrows without naming a lifetime, so the struct that holds it needs one.
pub fn needs_lifetime(ty: &str) -> bool {
    for (i, c) in ty.char_indices() {
        if c != '&' {
            continue;
        }
        if !ty[i + 1..].trim_start().starts_with('\'') {
            return true;
        }
    }
    false
}

/// The same type with every anonymous borrow tied to `'a`.
pub fn with_lifetime(ty: &str) -> String {
    let mut out = String::with_capacity(ty.len() + 4);
    let mut rest = ty;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        out.push('&');
        let after = &rest[at + 1..];
        let trimmed = after.trim_start();
        if trimmed.starts_with('\'') {
            out.push_str(after);
            return out;
        }
        out.push_str("'a ");
        rest = trimmed;
    }
    out.push_str(rest);
    out
}

/// The struct that holds the bundled parameters, as it will be written.
///
/// One field per parameter, in the order the declaration had them, with the type the
/// declaration gave. A single lifetime is introduced when any of those types borrows.
pub fn struct_text(name: &str, fields: &[(String, String)], doc: &str) -> String {
    let borrows = fields.iter().any(|(_, ty)| needs_lifetime(ty));
    let generics = if borrows { "<'a>" } else { "" };
    let mut out = String::new();
    if !doc.is_empty() {
        out.push_str(&format!("/// {doc}\n"));
    }
    out.push_str(&format!("pub struct {name}{generics} {{\n"));
    for (field, ty) in fields {
        let ty = if borrows {
            with_lifetime(ty)
        } else {
            ty.clone()
        };
        out.push_str(&format!("    pub {field}: {ty},\n"));
    }
    out.push_str("}\n");
    out
}

/// How the bundled parameter is spelled in the new declaration.
pub fn parameter_text(binding: &str, name: &str, fields: &[(String, String)]) -> String {
    let borrows = fields.iter().any(|(_, ty)| needs_lifetime(ty));
    if borrows {
        format!("{binding}: {name}<'_>")
    } else {
        format!("{binding}: {name}")
    }
}

/// The arguments of the call whose callee name ends at `after_name`, as a byte range inside
/// the parentheses, or `None` when what follows the name is not a call.
///
/// Brackets nest and string and character literals are skipped, so an argument that is a
/// closure, a method chain or a string containing a comma survives intact.
pub fn call_args_span(text: &str, after_name: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = after_name;
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    if bytes.get(i) != Some(&b'(') {
        return None;
    }
    matching_bracket(text, i).map(|close| (i + 1, close))
}

/// The offset of the bracket that closes the one at `open`.
///
/// Brackets of every kind nest; string and character literals and comments are skipped, so a
/// `}` in a string or an apostrophe in a comment does not end the block early.
pub fn matching_bracket(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if !matches!(bytes.get(open), Some(b'(' | b'[' | b'{')) {
        return None;
    }
    let mut i = open;
    let mut depth = 0i32;
    let mut in_str: Option<u8> = None;
    let mut escaped = false;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(quote) = in_str {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == quote {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |n| i + 2 + n + 2);
                continue;
            }
            b'"' => in_str = Some(b'"'),
            // A lifetime is not the start of a character literal.
            b'\''
                if bytes
                    .get(i + 1)
                    .is_some_and(|n| *n != b'_' && !n.is_ascii_alphabetic())
                    || bytes.get(i + 2) == Some(&b'\'') =>
            {
                in_str = Some(b'\'')
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// One entry per top-level argument, with the text between the commas kept verbatim.
///
/// A closure's own parameter list is bounded by two `|`, and the comma inside it belongs to
/// the closure, not to the call — `f(a, |x, y| x + y)` passes two arguments, not three.
pub fn split_args(inner: &str) -> Vec<String> {
    let bytes = inner.as_bytes();
    let (mut depth, mut i, mut last) = (0i32, 0usize, 0usize);
    let mut in_str: Option<u8> = None;
    let mut escaped = false;
    let mut in_closure_params = false;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(quote) = in_str {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == quote {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => in_str = Some(b'"'),
            b'\''
                if bytes
                    .get(i + 1)
                    .is_some_and(|n| *n != b'_' && !n.is_ascii_alphabetic())
                    || bytes.get(i + 2) == Some(&b'\'') =>
            {
                in_str = Some(b'\'')
            }
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth -= 1,
            // `||` is either an empty closure list or a logical or; neither opens anything.
            b'|' if bytes.get(i + 1) != Some(&b'|') && (i == 0 || bytes[i - 1] != b'|') => {
                in_closure_params = !in_closure_params;
            }
            b'|' if bytes.get(i + 1) == Some(&b'|') => i += 1,
            b',' if depth == 0 && !in_closure_params => {
                out.push(inner[last..i].trim().to_string());
                last = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let tail = inner[last..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// The argument list a call should end up with: the bundled arguments collected into one
/// struct literal, in the position the first of them had, everything else where it was.
///
/// `spelling` is how the type is named *in this file* — its bare name where an import can
/// carry it, its full path where nothing can (a file under `tests/` is not a module of the
/// crate and cannot import from `crate::`).
pub fn rewritten_args(
    args: &[String],
    bundled: &[usize],
    spelling: &str,
    fields: &[(String, String)],
) -> String {
    let literal = {
        let inner: Vec<String> = bundled
            .iter()
            .enumerate()
            .map(|(n, arg)| format!("{}: {}", fields[n].0, args[*arg]))
            .collect();
        format!("{spelling} {{ {} }}", inner.join(", "))
    };
    let first = bundled.first().copied().unwrap_or(0);
    let mut out = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if i == first {
            out.push(literal.clone());
        } else if !bundled.contains(&i) {
            out.push(arg.clone());
        }
    }
    out.join(", ")
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Bundles `params` of the function at `file:line:col` into a struct called `name`.
#[allow(clippy::too_many_arguments)]
pub async fn introduce(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    params: &[String],
    name: &str,
    binding: &str,
    apply: bool,
    force: bool,
) -> Result<ParameterObject> {
    anyhow::ensure!(params.len() >= 2, "bundling one parameter is not a bundle");
    let language = Language::of(file).with_context(|| {
        format!(
            "bundling parameters works in Rust, TypeScript, Python, Go, C, C++ and Swift files; \
             {} is none of them",
            file.display()
        )
    })?;
    if matches!(language, Language::C | Language::Cpp) {
        return introduce_c(
            language, remote, root, file, line, col, params, name, binding, apply, force,
        )
        .await;
    }
    if language != Language::Rust {
        return introduce_in(
            language, remote, root, file, line, col, params, name, binding, apply, force,
        )
        .await;
    }
    anyhow::ensure!(
        name.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
        "`{name}` is not a type name; Rust types are UpperCamelCase"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset = crate::signature::offset_of(&text, line, col)
        .context("the declaration is not at the resolved position")?;
    let (callee, open, close) = crate::signature::param_span(&text, offset)
        .with_context(|| format!("no function declaration at {}:{line}:{col}", file.display()))?;
    let old_inner = text[open..close].to_string();
    let (receiver, declared) = crate::signature::parse_declared(&old_inner);

    for p in params {
        anyhow::ensure!(
            declared.iter().any(|d| &d.name == p),
            "`{p}` is not a parameter of `{callee}`; it declares ({})",
            declared
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // In declaration order rather than the order the request happens to name them: the
    // struct's field order is the reader's, not the caller's.
    let bundled: Vec<usize> = declared
        .iter()
        .enumerate()
        .filter(|(_, d)| params.iter().any(|p| p == &d.name))
        .map(|(i, _)| i)
        .collect();
    anyhow::ensure!(bundled.len() == params.len(), "a parameter was named twice");

    let fields: Vec<(String, String)> = bundled
        .iter()
        .map(|i| {
            let d = &declared[*i];
            (d.name.clone(), type_of(&d.raw).unwrap_or("()").to_string())
        })
        .collect();
    let struct_text = struct_text(
        name,
        &fields,
        &format!("The parameters `{callee}` takes together."),
    );

    // Every edit is computed against the file as it is now and applied from the last offset
    // backwards, so no edit has to know what the ones before it did to the offsets.
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    let mut imports = Vec::new();
    let mut call_sites = 0usize;
    let home = crate::move_item::module_of(file).ok().map(|(_, m)| m);

    // How the new type is named in a given file: bare where the file is a module that can
    // import it, the full path where it is not — a file under `tests/` is a crate of its own
    // and `crate::` there means something else.
    let spelling_in = |path: &Path| -> (String, Option<String>) {
        let Some(home) = home.as_ref() else {
            return (name.to_string(), None);
        };
        if path == file {
            return (name.to_string(), None);
        }
        match crate::move_item::module_of(path) {
            Ok((_, theirs)) if theirs == *home => (name.to_string(), None),
            Ok((_, theirs)) => (
                name.to_string(),
                Some(format!("use {}::{name};", home.spelled_from(&theirs.krate))),
            ),
            Err(_) => (format!("{}::{name}", home.absolute()), None),
        }
    };

    let texts = |path: &Path| -> String {
        if path == file {
            text.clone()
        } else {
            std::fs::read_to_string(path).unwrap_or_default()
        }
    };

    for (path, rl, rc) in crate::signature::references(remote, root, file, line, col)
        .await
        .unwrap_or_default()
    {
        let body = texts(&path);
        let Some(at) = crate::signature::offset_of(&body, rl, rc) else {
            continue;
        };
        // The analyzer's position is trusted only when the name is actually there. If the file
        // changed since it was analysed, the position points at something else, and appending
        // an argument to whatever call follows it is the one mistake this must never make (#75).
        if !body[at..].starts_with(callee.as_str()) {
            unmatched.push(format!(
                "{}:{rl}:{rc} (the analyzer places `{callee}` here, but the file says otherwise)",
                display(root, &path)
            ));
            continue;
        }
        let after_name = at + callee.len();
        let Some((args_start, args_end)) = call_args_span(&body, after_name) else {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        };
        let args = split_args(&body[args_start..args_end]);
        if args.len() != declared.len() {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        }
        let (spelling, _) = spelling_in(&path);
        edits.entry(path).or_default().push((
            args_start,
            args_end - args_start,
            rewritten_args(&args, &bundled, &spelling, &fields),
        ));
        call_sites += 1;
    }

    // The body reaches the bundled parameters through one name now. The analyzer says where
    // each of them is used; a text search would also find them in a string and in a comment.
    let mut body_uses = 0usize;
    for i in &bundled {
        let d = &declared[*i];
        let Some(at) = text[open..close].find(&d.raw).map(|o| open + o) else {
            continue;
        };
        let (l, c) = crate::signature::line_col_at(&text, at);
        for (path, rl, rc) in crate::signature::references(remote, root, file, l, c)
            .await
            .unwrap_or_default()
        {
            if path != file || rl == l {
                continue;
            }
            let Some(o) = crate::signature::offset_of(&text, rl, rc) else {
                continue;
            };
            if !text[o..].starts_with(&d.name) {
                continue;
            }
            edits.entry(file.to_path_buf()).or_default().push((
                o,
                d.name.len(),
                format!("{binding}.{}", d.name),
            ));
            body_uses += 1;
        }
    }

    // The declaration itself, and the type above it.
    let now = {
        let mut out: Vec<String> = Vec::new();
        if let Some(r) = &receiver {
            out.push(r.trim().to_string());
        }
        let first = bundled.first().copied().unwrap_or(0);
        for (i, d) in declared.iter().enumerate() {
            if i == first {
                out.push(parameter_text(binding, name, &fields));
            } else if !bundled.contains(&i) {
                out.push(d.raw.trim().to_string());
            }
        }
        out.join(", ")
    };
    let declaring = edits.entry(file.to_path_buf()).or_default();
    declaring.push((open, close - open, now.clone()));
    let item_line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let item_start = text[..item_line_start]
        .rfind("\n\n")
        .map(|i| i + 2)
        .unwrap_or(item_line_start);
    declaring.push((item_start, 0, format!("{struct_text}\n")));

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts(&path);
        file_edits.sort_by_key(|(at, _, _)| *at);
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }

    // A call site in another module of the same crate names the type bare, so it has to import
    // it. The analyzer does not report an unresolved struct literal, so this is not something
    // the type check would catch afterwards.
    let paths: Vec<PathBuf> = rewritten.keys().cloned().collect();
    for path in paths {
        let (_, use_line) = spelling_in(&path);
        let Some(use_line) = use_line else { continue };
        let body = rewritten.get(&path).cloned().unwrap_or_default();
        let with_import = crate::move_item::add_import(&body, &use_line);
        if with_import != body {
            imports.push(format!("{}: added `{use_line}`", display(root, &path)));
        }
        rewritten.insert(path, with_import);
    }

    let (diagnostics, applied) = check_and_apply(remote, root, &rewritten, apply, force).await?;

    Ok(ParameterObject {
        symbol: callee,
        root: root.to_path_buf(),
        file: display(root, file),
        struct_text,
        was: old_inner.split_whitespace().collect::<Vec<_>>().join(" "),
        now,
        call_sites,
        imports,
        body_uses,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        unmatched,
        diagnostics,
        applied,
        language: language.fence(),
    })
}

/// Type-checks the rewritten files together in one overlay, and writes them when that was asked
/// for and the analyzer accepts them (or `force` says to write them regardless). Returns the
/// errors and whether anything was written.
async fn check_and_apply(
    remote: SocketAddr,
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
    apply: bool,
    force: bool,
) -> Result<(Vec<String>, bool)> {
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
            "the change does not compile ({} error(s)); nothing was written. Fix the request, \
             or pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }
    Ok((diagnostics, applied))
}

/// The language of the declaring file, which decides every piece of text this writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    TypeScript,
    Python,
    Go,
    C,
    Cpp,
    Swift,
}

impl Language {
    /// The language a file is written in, or `None` for one parameters cannot be bundled in.
    /// JavaScript is not TypeScript here: it has no interfaces to declare the new type with.
    /// A `.h` header is C by its name alone; [`introduce_c`] treats it as C++ when the
    /// definition or a caller is.
    pub fn of(path: &Path) -> Option<Language> {
        match crate::lang::language_id_for_path(path) {
            "rust" => Some(Language::Rust),
            "typescript" | "typescriptreact" => Some(Language::TypeScript),
            "python" => Some(Language::Python),
            "go" => Some(Language::Go),
            "c" => Some(Language::C),
            "cpp" => Some(Language::Cpp),
            "swift" => Some(Language::Swift),
            _ => None,
        }
    }

    /// The tag of the code block the report shows the new type in.
    pub fn fence(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::TypeScript => "typescript",
            Language::Python => "python",
            Language::Go => "go",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Swift => "swift",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::TypeScript => "TypeScript",
            Language::Python => "Python",
            Language::Go => "Go",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Swift => "Swift",
        }
    }
}

/// What the new parameter is called when the request does not say: the type's name in the casing
/// the language gives a parameter — `render_options` in Rust, Python, C and C++,
/// `renderOptions` in TypeScript, Go and Swift.
pub fn default_binding(file: &Path, name: &str) -> String {
    match Language::of(file) {
        Some(Language::TypeScript | Language::Go | Language::Swift) => lower_camel(name),
        _ => crate::fixture::snake_case(name),
    }
}

/// `HTTPOptions` becomes `httpOptions` and `Size` becomes `size`: the leading capitals are
/// lowered, except the one that starts the next word.
fn lower_camel(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut upper = chars.iter().take_while(|c| c.is_uppercase()).count();
    if upper > 1 && upper < chars.len() {
        upper -= 1;
    }
    chars[..upper]
        .iter()
        .flat_map(|c| c.to_lowercase())
        .chain(chars[upper..].iter().copied())
        .collect()
}

/// What kind of entry of a parameter list a parameter is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An ordinary named parameter.
    Plain,
    /// One that takes the rest of the positional arguments: `*args`, `...rest`, Go's `...T`.
    Variadic,
    /// Python's `**kwargs`.
    Keywords,
    /// Python's bare `*` and `/`, which separate parameters and are not one.
    Marker,
}

/// One entry of a parameter list in TypeScript, Python or Go.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// The entry as written, without the comments around it.
    pub raw: String,
    pub name: String,
    /// Where the name starts, as a byte offset into the parameter list.
    pub name_at: usize,
    /// The declared type; in Go, for a name in a group like `a, b int`, the group's type. In C
    /// and C++ it is the whole declaration of the name (`const char *name`, `int (*cb)(int)`),
    /// because a declarator wraps the name rather than following it.
    pub ty: Option<String>,
    pub default: Option<String>,
    /// TypeScript's `x?: T`.
    pub optional: bool,
    /// Go's `a, b int` writes the type once, after the last name. A name before it has no type
    /// of its own in the text, so it has to be written with one when the group is split.
    pub shares_type: bool,
    pub kind: Kind,
    /// Swift's argument label, the name a call passes the argument under: the parameter's own
    /// name, the one written before it (`with name: String`), or `None` for `_`. Unused in the
    /// other languages, where a call passes arguments by position or by the parameter's name.
    pub label: Option<String>,
}

/// A field of the new type.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    /// The declared type, or the one hover gives; `None` when neither knows it.
    pub ty: Option<String>,
    pub default: Option<String>,
    pub optional: bool,
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(bytes.len(), |n| from + n)
}

/// Walks `text` from `from`, handing `visit` the offset and byte of everything that is code.
/// Comments are skipped; a string literal is reported by its two quotes only, so a bracket or a
/// comma inside it is never seen. `visit` returns `false` to stop.
///
/// A quote is a string in every one of these languages, and so is a backtick outside Python —
/// unlike Rust, where `'a` is a lifetime. Python's comment is `#`, and its `//` is division.
fn walk_code(
    text: &str,
    from: usize,
    language: Language,
    mut visit: impl FnMut(usize, u8) -> bool,
) {
    let bytes = text.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        let c = bytes[i];
        let rest = &bytes[i..];
        if language == Language::Python && c == b'#' {
            i = line_end(bytes, i);
            continue;
        }
        if language != Language::Python && rest.starts_with(b"//") {
            i = line_end(bytes, i);
            continue;
        }
        if language != Language::Python && rest.starts_with(b"/*") {
            i = find_bytes(&bytes[i + 2..], b"*/").map_or(bytes.len(), |n| i + 2 + n + 2);
            continue;
        }
        if language == Language::Python && (rest.starts_with(b"\"\"\"") || rest.starts_with(b"'''"))
        {
            if !visit(i, c) {
                return;
            }
            let Some(n) = find_bytes(&bytes[i + 3..], &rest[..3]) else {
                return;
            };
            let last = i + 3 + n + 2;
            if !visit(last, c) {
                return;
            }
            i = last + 1;
            continue;
        }
        if c == b'"' || c == b'\'' || (c == b'`' && language != Language::Python) {
            if !visit(i, c) {
                return;
            }
            // A Go raw string has no escapes; every other literal here does.
            let escapes = !(c == b'`' && language == Language::Go);
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != c {
                j += if escapes && bytes[j] == b'\\' { 2 } else { 1 };
            }
            if j >= bytes.len() || !visit(j, c) {
                return;
            }
            i = j + 1;
            continue;
        }
        if !visit(i, c) {
            return;
        }
        i += 1;
    }
}

/// The offset of the bracket that closes the one at `open`, with strings and comments skipped
/// the way the language writes them.
fn close_in(text: &str, open: usize, language: Language) -> Option<usize> {
    if !matches!(text.as_bytes().get(open), Some(b'(' | b'[' | b'{')) {
        return None;
    }
    let mut depth = 0i32;
    let mut found = None;
    walk_code(text, open, language, |i, c| {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    found = Some(i);
                    return false;
                }
            }
            _ => {}
        }
        true
    });
    found
}

/// The top-level entries of a comma-separated list, each as the offset of its first character
/// and its text from there to its last character of code — so a comment before or after an
/// entry is not part of it. Type arguments nest in TypeScript (`Map<string, number>`), C++
/// (`std::map<int, int>`) and Swift (`Dictionary<String, Int>`); a `<` is taken as one only
/// straight after a name, since a comparison is written with spaces.
fn entries(list: &str, language: Language) -> Vec<(usize, &str)> {
    let bytes = list.as_bytes();
    let (mut depth, mut angle) = (0i32, 0i32);
    let mut out = Vec::new();
    let mut current: Option<(usize, usize)> = None;
    let generic = matches!(
        language,
        Language::TypeScript | Language::Cpp | Language::Swift
    );
    walk_code(list, 0, language, |i, c| {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'<' if generic && i > 0 && is_ident_byte(bytes[i - 1]) => angle += 1,
            // `=>` and Swift's `->` are arrows, not the end of a type argument list.
            b'>' if angle > 0 && !matches!(bytes[i - 1], b'=' | b'-') => angle -= 1,
            b',' if depth == 0 && angle == 0 => {
                if let Some((start, end)) = current.take() {
                    out.push((start, &list[start..=end]));
                }
                return true;
            }
            _ => {}
        }
        if !c.is_ascii_whitespace() {
            current = Some((current.map_or(i, |(start, _)| start), i));
        }
        true
    });
    if let Some((start, end)) = current {
        out.push((start, &list[start..=end]));
    }
    out
}

/// Splits an entry at its first top-level `=` that is an assignment — not `==`, `!=`, `<=`,
/// `>=` or TypeScript's `=>`.
fn split_default(entry: &str, language: Language) -> (&str, Option<&str>) {
    let bytes = entry.as_bytes();
    let mut depth = 0i32;
    let mut at = None;
    walk_code(entry, 0, language, |i, c| {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'=' if depth == 0 => {
                let next = bytes.get(i + 1).copied().unwrap_or(b' ');
                let prev = if i > 0 { bytes[i - 1] } else { b' ' };
                if next != b'=' && next != b'>' && !matches!(prev, b'=' | b'!' | b'<' | b'>') {
                    at = Some(i);
                    return false;
                }
            }
            _ => {}
        }
        true
    });
    match at {
        Some(i) => (entry[..i].trim_end(), Some(entry[i + 1..].trim())),
        None => (entry, None),
    }
}

fn leading_ident(text: &str) -> &str {
    let n = text.bytes().take_while(|b| is_ident_byte(*b)).count();
    &text[..n]
}

/// One entry of a parameter list, starting at `at` in the list.
fn parse_param(entry: &str, at: usize, language: Language) -> Param {
    let mut param = Param {
        raw: entry.to_string(),
        name: String::new(),
        name_at: at,
        ty: None,
        default: None,
        optional: false,
        shares_type: false,
        kind: Kind::Plain,
        label: None,
    };
    match language {
        Language::C | Language::Cpp => return parse_c_param(param, language),
        Language::Swift => return parse_swift_param(param),
        _ => {}
    }
    let mut head = 0usize;
    match language {
        Language::Python => {
            if entry == "*" || entry == "/" {
                param.name = entry.to_string();
                param.kind = Kind::Marker;
                return param;
            }
            if entry.starts_with("**") {
                param.kind = Kind::Keywords;
                head = 2;
            } else if entry.starts_with('*') {
                param.kind = Kind::Variadic;
                head = 1;
            }
        }
        Language::TypeScript => {
            // A constructor's parameter properties carry modifiers before the name.
            loop {
                let rest = &entry[head..];
                let Some(m) = [
                    "public ",
                    "private ",
                    "protected ",
                    "readonly ",
                    "override ",
                ]
                .iter()
                .find(|m| rest.starts_with(**m)) else {
                    break;
                };
                head += m.len();
                head += entry[head..].len() - entry[head..].trim_start().len();
            }
            if entry[head..].starts_with("...") {
                param.kind = Kind::Variadic;
                head += 3;
            }
        }
        _ => {}
    }
    param.name = leading_ident(&entry[head..]).to_string();
    param.name_at = at + head;
    let mut rest = entry[head + param.name.len()..].trim_start();
    if language == Language::Go {
        if !rest.is_empty() {
            if rest.starts_with("...") {
                param.kind = Kind::Variadic;
            }
            param.ty = Some(rest.to_string());
        }
        return param;
    }
    if language == Language::TypeScript
        && let Some(after) = rest.strip_prefix('?')
    {
        param.optional = true;
        rest = after.trim_start();
    }
    let (typed, default) = split_default(rest, language);
    param.default = default.map(str::to_string);
    param.ty = typed
        .strip_prefix(':')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    param
}

/// A C or C++ parameter: a declaration whose name is its last identifier (`const char *name`,
/// `int xs[]`) or sits inside the declarator (`int (*cb)(int)`), with a C++ default after it.
/// An array parameter is a pointer, which is what its field has to be: an array field would be
/// a different type, and one without a size would not compile at all.
fn parse_c_param(mut param: Param, language: Language) -> Param {
    let entry = param.raw.clone();
    // C's `...` and a C++ parameter pack (`Args&&... args`) both take any number of arguments.
    if entry.contains("...") {
        param.kind = Kind::Variadic;
        param.name = entry.rsplit("...").next().unwrap_or("").trim().to_string();
        return param;
    }
    let (declaration, default) = split_default(&entry, language);
    param.default = default.map(str::to_string);
    let bytes = declaration.as_bytes();
    // A function pointer or reference names itself inside the first parentheses.
    if let Some(p) = declaration.find("(*").or_else(|| declaration.find("(&")) {
        let start = p + 2 + declaration[p + 2..].len() - declaration[p + 2..].trim_start().len();
        let name = leading_ident(&declaration[start..]);
        param.name = name.to_string();
        param.name_at += start;
        param.ty = Some(declaration.to_string());
        return param;
    }
    let mut end = declaration.len();
    let mut suffixes: Vec<&str> = Vec::new();
    while end > 0 && bytes[end - 1] == b']' {
        let Some(open) = declaration[..end].rfind('[') else {
            break;
        };
        suffixes.insert(0, &declaration[open..end]);
        end = declaration[..open].trim_end().len();
    }
    let start = declaration[..end]
        .bytes()
        .rposition(|b| !is_ident_byte(b))
        .map_or(0, |i| i + 1);
    let head = declaration[..start].trim_end();
    // A lone type such as `int` or `const char *` names nothing.
    if head.is_empty() || head.ends_with("::") || start == end {
        param.ty = Some(declaration.to_string());
        return param;
    }
    let name = &declaration[start..end];
    param.name = name.to_string();
    param.name_at += start;
    let joined = |head: &str| {
        if head.ends_with(['*', '&']) {
            format!("{head}{name}")
        } else {
            format!("{head} {name}")
        }
    };
    param.ty = Some(match suffixes.split_first() {
        None => joined(head),
        Some((_, [])) => joined(&format!("{head} *")),
        Some((_, rest)) => format!("{head} (*{name}){}", rest.concat()),
    });
    param
}

/// A Swift parameter: an optional argument label before the name (`_ label`, `with name`), a
/// type after the colon, and a default after that; `Int...` takes several arguments.
fn parse_swift_param(mut param: Param) -> Param {
    let entry = param.raw.clone();
    let Some(colon) = entry.find(':') else {
        param.name = leading_ident(&entry).to_string();
        return param;
    };
    let names: Vec<(usize, &str)> = entry[..colon]
        .split_whitespace()
        .filter(|w| !w.starts_with('@'))
        .map(|w| (w.as_ptr() as usize - entry.as_ptr() as usize, w))
        .collect();
    let (label, (name_at, name)) = match names.as_slice() {
        [(_, label), name] => ((*label != "_").then(|| label.to_string()), *name),
        [name] => (Some(name.1.to_string()), *name),
        _ => (None, (0, "")),
    };
    param.label = label;
    param.name = name.to_string();
    param.name_at += name_at;
    let (typed, default) = split_default(entry[colon + 1..].trim(), Language::Swift);
    param.default = default.map(str::to_string);
    let ty = typed.trim();
    if ty.ends_with("...") {
        param.kind = Kind::Variadic;
    }
    param.ty = (!ty.is_empty()).then(|| ty.to_string());
    param
}

/// The receiver and the parameters of a parameter list in any language but Rust.
///
/// Python's receiver is a method's first parameter, `self` or `cls`, and TypeScript's is a
/// `this` parameter; both are kept verbatim and never bundled. Go's receiver is written before
/// the method's name, so it is not in this list at all, and C++ and Swift pass theirs
/// implicitly.
pub fn parse_params(list: &str, language: Language) -> (Option<String>, Vec<Param>) {
    let mut receiver = None;
    let mut out: Vec<Param> = Vec::new();
    // C writes an empty list as `(void)`.
    if matches!(language, Language::C | Language::Cpp) && list.trim() == "void" {
        return (None, out);
    }
    for (at, entry) in entries(list, language) {
        let param = parse_param(entry, at, language);
        let first = receiver.is_none() && out.is_empty();
        let is_receiver = first
            && param.kind == Kind::Plain
            && match language {
                Language::Python => param.name == "self" || param.name == "cls",
                Language::TypeScript => param.name == "this",
                _ => false,
            };
        if is_receiver {
            receiver = Some(entry.to_string());
            continue;
        }
        out.push(param);
    }
    if language == Language::Go {
        let mut group_type: Option<String> = None;
        for p in out.iter_mut().rev() {
            match &p.ty {
                Some(ty) => group_type = Some(ty.clone()),
                None => {
                    p.ty = group_type.clone();
                    p.shares_type = true;
                }
            }
        }
    }
    (receiver, out)
}

/// The type a hover gives a parameter: `number` from TypeScript's `(parameter) width: number`,
/// `int` from basedpyright's `(parameter) width: int`. `None` when the server does not know it,
/// which basedpyright says as `Unknown`.
pub fn hover_parameter_type(hover: &str, name: &str) -> Option<String> {
    let prefix = format!("(parameter) {name}");
    let rest = hover
        .lines()
        .find_map(|l| l.trim().strip_prefix(prefix.as_str()))?;
    let ty = rest
        .trim_start_matches('?')
        .trim_start()
        .strip_prefix(':')?
        .trim();
    (!ty.is_empty() && ty != "Unknown").then(|| ty.to_string())
}

async fn hover_type(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    at: usize,
    name: &str,
) -> Option<String> {
    let (line, col) = crate::signature::line_col_at(text, at);
    let uri = url::Url::from_file_path(file).ok()?.to_string();
    let res = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        }),
    )
    .await
    .ok()?;
    hover_parameter_type(res.pointer("/contents/value")?.as_str()?, name)
}

/// One level of indentation as the file writes it: its first indented line that is not the
/// inside of a block comment, or the language's usual one. Go is always a tab.
fn indent_unit(text: &str, language: Language) -> String {
    if language == Language::Go {
        return "\t".to_string();
    }
    if let Some(unit) = detected_indent(text) {
        return unit;
    }
    match language {
        Language::TypeScript => "  ".to_string(),
        _ => "    ".to_string(),
    }
}

/// The indentation of the file's first indented line that is not the inside of a block comment,
/// or `None` when nothing in it is indented — a header of prototypes, typically.
fn detected_indent(text: &str) -> Option<String> {
    for line in text.lines() {
        let code = line.trim_start();
        if code.is_empty() || code.len() == line.len() || code.starts_with('*') {
            continue;
        }
        let ws = &line[..line.len() - code.len()];
        return Some(if ws.starts_with('\t') {
            "\t".to_string()
        } else {
            ws.to_string()
        });
    }
    None
}

/// The type that holds the bundled parameters in TypeScript, Python or Go, as it will be written.
///
/// TypeScript gets an `interface` (exported when the declaration it stands above is), with a
/// parameter that had no type and no hover typed `any`, which is what it was. Python gets a
/// `@dataclass` when every field has a type, since a dataclass field is an annotation; a
/// parameter the analyzer cannot type makes it a plain class with an `__init__`, rather than a
/// type being invented. Go gets a `struct` whose fields keep the parameters' names, so a field
/// is exported exactly when the parameter's name was capitalised, which it rarely is.
///
/// C and C++ get a `struct` whose members are declared the way the parameters were, with a C++
/// default as the member's initialiser. Swift gets a `struct` of `let` properties (public when
/// the function is), a defaulted one a `var`, so that its memberwise initialiser is the literal.
pub fn type_text(
    language: Language,
    name: &str,
    callee: &str,
    fields: &[Field],
    indent: &str,
    export: bool,
) -> String {
    let mut out = String::new();
    match language {
        Language::TypeScript => {
            out.push_str(&format!(
                "/** The parameters `{callee}` takes together. */\n"
            ));
            let export = if export { "export " } else { "" };
            out.push_str(&format!("{export}interface {name} {{\n"));
            for f in fields {
                let optional = if f.optional { "?" } else { "" };
                let ty = f.ty.as_deref().unwrap_or("any");
                out.push_str(&format!("{indent}{}{optional}: {ty};\n", f.name));
            }
            out.push_str("}\n");
        }
        Language::Python => {
            // A field without a default after one with a default is an error in a dataclass and
            // in an `__init__`, unless the fields are keyword-only — which every call site this
            // writes passes them as anyway.
            let mut seen_default = false;
            let keyword_only = fields.iter().any(|f| {
                seen_default |= f.default.is_some();
                seen_default && f.default.is_none()
            });
            let doc = format!("{indent}\"\"\"The parameters `{callee}` takes together.\"\"\"\n\n");
            if fields.iter().all(|f| f.ty.is_some()) {
                out.push_str(if keyword_only {
                    "@dataclass(kw_only=True)\n"
                } else {
                    "@dataclass\n"
                });
                out.push_str(&format!("class {name}:\n{doc}"));
                for f in fields {
                    let ty = f.ty.as_deref().unwrap_or_default();
                    match &f.default {
                        Some(d) => out.push_str(&format!("{indent}{}: {ty} = {d}\n", f.name)),
                        None => out.push_str(&format!("{indent}{}: {ty}\n", f.name)),
                    }
                }
            } else {
                out.push_str(&format!("class {name}:\n{doc}"));
                let mut params = vec!["self".to_string()];
                if keyword_only {
                    params.push("*".to_string());
                }
                for f in fields {
                    let mut p = f.name.clone();
                    if let Some(ty) = &f.ty {
                        p.push_str(&format!(": {ty}"));
                    }
                    if let Some(d) = &f.default {
                        p.push_str(if f.ty.is_some() { " = " } else { "=" });
                        p.push_str(d);
                    }
                    params.push(p);
                }
                out.push_str(&format!("{indent}def __init__({}):\n", params.join(", ")));
                for f in fields {
                    out.push_str(&format!("{indent}{indent}self.{0} = {0}\n", f.name));
                }
            }
        }
        Language::C | Language::Cpp => {
            // A C header may be read by a C89 compiler, where `//` is not a comment.
            if language == Language::C {
                out.push_str(&format!(
                    "/* The parameters `{callee}` takes together. */\n"
                ));
            } else {
                out.push_str(&format!("// The parameters `{callee}` takes together.\n"));
            }
            out.push_str(&format!("struct {name} {{\n"));
            for f in fields {
                let declaration = f.ty.as_deref().unwrap_or(&f.name);
                match &f.default {
                    Some(d) => out.push_str(&format!("{indent}{declaration} = {d};\n")),
                    None => out.push_str(&format!("{indent}{declaration};\n")),
                }
            }
            out.push_str("};\n");
        }
        Language::Swift => {
            out.push_str(&format!("/// The parameters `{callee}` takes together.\n"));
            // A public function cannot take an internal type.
            let public = if export { "public " } else { "" };
            out.push_str(&format!("{public}struct {name} {{\n"));
            for f in fields {
                let ty = f.ty.as_deref().unwrap_or("Any");
                // A `let` with a value is a constant the memberwise initialiser cannot set; a
                // `var` with one is a parameter of it with that default.
                match &f.default {
                    Some(d) => out.push_str(&format!("{indent}var {}: {ty} = {d}\n", f.name)),
                    None => out.push_str(&format!("{indent}let {}: {ty}\n", f.name)),
                }
            }
            out.push_str("}\n");
        }
        _ => {
            out.push_str(&format!(
                "// {name} holds the parameters {callee} takes together.\n"
            ));
            out.push_str(&format!("type {name} struct {{\n"));
            // gofmt aligns the types of consecutive fields; writing them aligned keeps the file
            // as gofmt would leave it.
            let width = fields.iter().map(|f| f.name.len()).max().unwrap_or(0);
            for f in fields {
                let ty = f.ty.as_deref().unwrap_or("any");
                out.push_str(&format!("\t{:width$} {ty}\n", f.name));
            }
            out.push_str("}\n");
        }
    }
    out
}

/// How the bundled parameter is declared: `opts: Opts` in TypeScript, Python and Swift,
/// `opts Opts` in Go, `Opts opts` in C++, and `struct Opts opts` in C, where a struct's name is
/// only a type together with the keyword.
fn parameter_in(language: Language, binding: &str, name: &str) -> String {
    match language {
        Language::Go => format!("{binding} {name}"),
        Language::Cpp => format!("{name} {binding}"),
        Language::C => format!("struct {name} {binding}"),
        _ => format!("{binding}: {name}"),
    }
}

/// A literal of the new type from (field, value) pairs: `{ a: x, b: y }` in TypeScript, whose
/// interfaces are structural and need no name; `Opts(a=x, b=y)` in Python; `Opts{a: x, b: y}`
/// in Go; `Opts(a: x, b: y)`, the memberwise initialiser, in Swift. C needs a compound literal,
/// `(struct Opts){.a = x, .b = y}`, since a braced list alone is not an expression there; C++
/// converts a braced list to the parameter's type, and its designators are C++20's — see
/// [`aggregate_text`] for the standards before it.
pub fn literal_text(language: Language, spelling: &str, pairs: &[(String, String)]) -> String {
    let join = |sep: &str| {
        pairs
            .iter()
            .map(|(f, v)| format!("{f}{sep}{v}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let designated = || {
        pairs
            .iter()
            .map(|(f, v)| format!(".{f} = {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match language {
        Language::TypeScript => format!("{{ {} }}", join(": ")),
        Language::Python => format!("{spelling}({})", join("=")),
        Language::Go => format!("{spelling}{{{}}}", join(": ")),
        Language::Rust => format!("{spelling} {{ {} }}", join(": ")),
        Language::C => format!("(struct {spelling}){{{}}}", designated()),
        Language::Cpp => format!("{{{}}}", designated()),
        Language::Swift => format!("{spelling}({})", join(": ")),
    }
}

/// A C++ literal without designators, `{x, y}`, for a project that compiles to a standard before
/// C++20. The values are in field order, which is what an aggregate is initialised by; a field
/// the call left to its default can only be a trailing one, and it is left out.
pub fn aggregate_text(pairs: &[(String, String)]) -> String {
    let values: Vec<&str> = pairs.iter().map(|(_, v)| v.as_str()).collect();
    format!("{{{}}}", values.join(", "))
}

/// The arguments of a call whose callee name ends at `after_name`, as a byte range inside the
/// parentheses; `None` when what follows the name is not a call. Explicit type arguments in
/// TypeScript and C++ (`build<T>(…)`) come between the name and the list.
fn call_args_in(text: &str, after_name: usize, language: Language) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = after_name;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if matches!(language, Language::TypeScript | Language::Cpp) && bytes.get(i) == Some(&b'<') {
        let mut depth = 0i32;
        while i < bytes.len() {
            match bytes[i] {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                b';' | b'\n' => return None,
                _ => {}
            }
            i += 1;
        }
        i += 1;
    }
    if bytes.get(i) != Some(&b'(') {
        return None;
    }
    close_in(text, i, language).map(|close| (i + 1, close))
}

/// A Python keyword argument, as its name and its value: `height=2`, but not `a == b`.
fn keyword_arg(arg: &str) -> Option<(&str, &str)> {
    let name = leading_ident(arg);
    if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let value = arg[name.len()..].trim_start().strip_prefix('=')?;
    if value.starts_with('=') {
        return None;
    }
    Some((name, value.trim()))
}

/// Which parameter each argument of a call binds to, or `None` when the call is not one of this
/// declaration with the right arity. An argument bound to no parameter (`None` in the list) is
/// one Python's `*args` or `**kwargs` takes, and it stays as it was.
///
/// TypeScript and Go pass every argument by position, so the count has to match, as in Rust.
/// Python binds positional arguments in order and keyword arguments by name; a parameter no
/// argument binds has to have a default. A call that spreads (`*xs`, `**kw`) cannot be mapped
/// without running it.
///
/// C and C++ pass by position too, but a C++ call may leave off trailing parameters that have
/// defaults, and a C `...` takes whatever is left. Swift binds by argument label, in declaration
/// order, and a parameter with a default may be skipped.
pub fn bind_arguments(
    args: &[String],
    params: &[Param],
    language: Language,
) -> Option<Vec<Option<usize>>> {
    match language {
        Language::Python => {}
        Language::Swift => return bind_swift_arguments(args, params),
        Language::C | Language::Cpp => {
            let variadic = params.last().is_some_and(|p| p.kind == Kind::Variadic);
            let fixed = params.len() - usize::from(variadic);
            if args.len() > fixed && !variadic {
                return None;
            }
            let left_off = &params[args.len().min(fixed)..fixed];
            if left_off.iter().any(|p| p.default.is_none()) {
                return None;
            }
            return Some((0..args.len()).map(|a| Some(a.min(fixed))).collect());
        }
        _ => {
            return (args.len() == params.len()).then(|| (0..args.len()).map(Some).collect());
        }
    }
    let mut positional = Vec::new();
    for (i, p) in params.iter().enumerate() {
        match p.kind {
            Kind::Plain => positional.push(i),
            Kind::Marker if p.name == "*" => break,
            Kind::Variadic | Kind::Keywords => break,
            Kind::Marker => {}
        }
    }
    let takes_rest = params.iter().any(|p| p.kind == Kind::Variadic);
    let takes_keywords = params.iter().any(|p| p.kind == Kind::Keywords);
    let mut taken = vec![false; params.len()];
    let mut bound = Vec::with_capacity(args.len());
    let mut next = 0usize;
    for arg in args {
        if arg.starts_with('*') {
            return None;
        }
        if let Some((key, _)) = keyword_arg(arg) {
            match params
                .iter()
                .position(|p| p.kind == Kind::Plain && p.name == key)
            {
                Some(i) if !taken[i] => {
                    taken[i] = true;
                    bound.push(Some(i));
                }
                None if takes_keywords => bound.push(None),
                _ => return None,
            }
        } else {
            match positional.get(next) {
                Some(&i) => {
                    taken[i] = true;
                    bound.push(Some(i));
                    next += 1;
                }
                None if takes_rest => bound.push(None),
                None => return None,
            }
        }
    }
    let missing = params
        .iter()
        .enumerate()
        .any(|(i, p)| p.kind == Kind::Plain && !taken[i] && p.default.is_none());
    (!missing).then_some(bound)
}

/// A Swift argument label and the value after it: `width: 3`, but not `a ? b : c`.
fn swift_label(arg: &str) -> Option<(&str, &str)> {
    let name = leading_ident(arg);
    if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let value = arg[name.len()..].trim_start().strip_prefix(':')?;
    Some((name, value.trim()))
}

/// Swift's binding: each argument goes to the next parameter whose label it carries, and a
/// parameter passed over has to have a default. The arguments after the first one of a variadic
/// parameter carry no label and belong to it too.
fn bind_swift_arguments(args: &[String], params: &[Param]) -> Option<Vec<Option<usize>>> {
    let mut bound: Vec<Option<usize>> = Vec::with_capacity(args.len());
    let mut next = 0usize;
    for arg in args {
        let label = swift_label(arg).map(|(l, _)| l);
        loop {
            let p = params.get(next)?;
            let fed = bound.last() == Some(&Some(next));
            if p.kind == Kind::Variadic && fed && label.is_none() {
                bound.push(Some(next));
                break;
            }
            if !fed && p.label.as_deref() == label {
                bound.push(Some(next));
                if p.kind != Kind::Variadic {
                    next += 1;
                }
                break;
            }
            // A variadic parameter may be given nothing at all.
            if p.default.is_none() && p.kind != Kind::Variadic && !fed {
                return None;
            }
            next += 1;
        }
    }
    let missing = params
        .iter()
        .enumerate()
        .any(|(i, p)| p.default.is_none() && p.kind != Kind::Variadic && !bound.contains(&Some(i)));
    (!missing).then_some(bound)
}

/// The argument list a call ends up with: the bundled arguments collected into one literal where
/// the first of them was, every other argument where it was.
///
/// In Python the literal is passed the way that first argument was: by keyword when it was a
/// keyword argument, since a positional argument cannot follow one. A bundled parameter the call
/// left to its default is left out of the literal, where the field's default stands in for it;
/// when the call passed none of them, the literal goes last, by keyword.
pub fn rewritten_call(
    args: &[String],
    bound: &[Option<usize>],
    bundled: &[usize],
    params: &[Param],
    language: Language,
    spelling: &str,
    binding: &str,
) -> String {
    rewritten_call_with(args, bound, bundled, params, language, binding, |pairs| {
        literal_text(language, spelling, pairs)
    })
}

/// [`rewritten_call`] with the literal written by `literal`, for a language where it depends on
/// more than the language — a C++ project before C++20 has no designators.
///
/// A Swift argument is passed under the new parameter's label, which it has unless the first
/// bundled parameter had none. When a Swift or C++ call passed none of the bundled arguments,
/// every one of them had a default, and so does the new parameter; the call stays as it was.
fn rewritten_call_with(
    args: &[String],
    bound: &[Option<usize>],
    bundled: &[usize],
    params: &[Param],
    language: Language,
    binding: &str,
    literal: impl Fn(&[(String, String)]) -> String,
) -> String {
    let value_of = |arg: &str| -> String {
        match language {
            Language::Python => keyword_arg(arg).map_or(arg, |(_, v)| v).to_string(),
            Language::Swift => swift_label(arg).map_or(arg, |(_, v)| v).to_string(),
            _ => arg.to_string(),
        }
    };
    let pairs: Vec<(String, String)> = bundled
        .iter()
        .filter_map(|p| {
            let a = bound.iter().position(|b| *b == Some(*p))?;
            Some((params[*p].name.clone(), value_of(&args[a])))
        })
        .collect();
    let literal = literal(&pairs);
    let first = bound
        .iter()
        .position(|b| b.is_some_and(|p| bundled.contains(&p)));
    let labelled =
        language == Language::Swift && bundled.first().is_some_and(|p| params[*p].label.is_some());
    let mut out = Vec::new();
    for (a, arg) in args.iter().enumerate() {
        if Some(a) == first {
            if language == Language::Python && keyword_arg(arg).is_some() {
                out.push(format!("{binding}={literal}"));
            } else if labelled {
                out.push(format!("{binding}: {literal}"));
            } else {
                out.push(literal.clone());
            }
        } else if !bound[a].is_some_and(|p| bundled.contains(&p)) {
            out.push(arg.clone());
        }
    }
    if first.is_none() && language == Language::Python {
        out.push(format!("{binding}={literal}"));
    }
    out.join(", ")
}

/// The byte range of a function's body, from the end of its parameter list to the end of the
/// function. A reference outside it is not a use in the body: basedpyright lists a keyword
/// argument at a call site (`height=2`) among the references to the parameter `height`.
fn body_span(text: &str, decl: usize, close: usize, language: Language) -> (usize, usize) {
    if language != Language::Python {
        return match body_open(text, close, language).and_then(|o| close_in(text, o, language)) {
            Some(end) => (close, end),
            None => (close, close),
        };
    }
    // The signature ends at the first top-level colon after the parameter list; the return
    // annotation before it has none.
    let mut depth = 0i32;
    let mut colon = None;
    walk_code(text, close + 1, language, |i, c| {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b':' if depth == 0 => {
                colon = Some(i);
                return false;
            }
            _ => {}
        }
        true
    });
    let Some(colon) = colon else {
        return (close, close);
    };
    let eol = line_end(text.as_bytes(), colon);
    let same_line = text[colon + 1..eol].trim();
    if !same_line.is_empty() && !same_line.starts_with('#') {
        return (close, eol);
    }
    let def_line = text[..decl].rfind('\n').map_or(0, |i| i + 1);
    let indent = text[def_line..].len() - text[def_line..].trim_start_matches([' ', '\t']).len();
    let mut at = eol + 1;
    while at < text.len() {
        let end = line_end(text.as_bytes(), at);
        let line = &text[at..end];
        let code = line.trim_start();
        if !code.is_empty() && !code.starts_with('#') && line.len() - code.len() <= indent {
            break;
        }
        at = end + 1;
    }
    (close, at.min(text.len()))
}

/// The brace that opens a TypeScript or Go function's body, past its return type — which can
/// be an object type in braces itself (`): { a: number } {`), told apart by what precedes it.
/// `None` for a declaration without a body, such as a TypeScript overload.
fn body_open(text: &str, close: usize, language: Language) -> Option<usize> {
    let mut prev = b')';
    let mut skip_until = 0usize;
    let mut found = None;
    walk_code(text, close + 1, language, |i, c| {
        if i < skip_until || c.is_ascii_whitespace() {
            return true;
        }
        match c {
            b'{' if !matches!(prev, b':' | b'|' | b'&' | b'<' | b',') => {
                found = Some(i);
                return false;
            }
            b'{' | b'(' | b'[' => match close_in(text, i, language) {
                Some(end) => {
                    skip_until = end + 1;
                    prev = b')';
                }
                None => return false,
            },
            b';' => return false,
            _ => prev = c,
        }
        true
    });
    found
}

/// The start of the line of the top-level declaration the function belongs to: the class of a
/// TypeScript or Python method, the function itself otherwise (a Go method is top-level).
fn top_level_line(text: &str, decl: usize) -> usize {
    let mut start = text[..decl].rfind('\n').map_or(0, |i| i + 1);
    while start > 0 {
        let line = &text[start..line_end(text.as_bytes(), start)];
        if !line.trim().is_empty() && !line.starts_with([' ', '\t']) {
            break;
        }
        start = text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
    }
    start
}

/// Where the new type goes: above the top-level declaration, and above the comments and
/// decorators that belong to it, so it does not come between a declaration and its doc.
fn item_start_in(text: &str, top: usize, language: Language) -> usize {
    let mut start = top;
    while start > 0 {
        let prev_start = text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
        let prev = text[prev_start..start - 1].trim_start();
        let belongs = match language {
            Language::Python => prev.starts_with('#') || prev.starts_with('@'),
            // A C++ template header and an attribute on a line of its own are part of the
            // declaration below them.
            Language::C | Language::Cpp => ["//", "/*", "*", "template", "[["]
                .iter()
                .any(|p| prev.starts_with(p)),
            _ => ["//", "/*", "*", "@"].iter().any(|p| prev.starts_with(p)),
        };
        if !belongs {
            break;
        }
        start = prev_start;
    }
    start
}

/// The qualifier a call spells the function with — `home.` in `home.build(…)` — which the new
/// type needs as well, since it is declared next to the function.
fn qualifier_before(text: &str, at: usize) -> &str {
    let bytes = text.as_bytes();
    if at == 0 || bytes[at - 1] != b'.' {
        return "";
    }
    let mut start = at;
    while start > 0 && (bytes[start - 1] == b'.' || is_ident_byte(bytes[start - 1])) {
        start -= 1;
    }
    &text[start..at]
}

/// Whether a reference is a name in an import statement. basedpyright lists the `build` of
/// `from app.home import build` among the references to `build`; it is neither a call to
/// rewrite nor a use to report.
fn in_import(text: &str, at: usize, language: Language) -> bool {
    let starts = |l: &str| match language {
        Language::Python => l.starts_with("from ") || l.starts_with("import "),
        _ => {
            l.starts_with("import ") || l.starts_with("export {") || l.starts_with("export type {")
        }
    };
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    if starts(text[line_start..].trim_start()) {
        return true;
    }
    // A name on a line of its own inside an import's parenthesised or braced list: the list
    // opens right after the `import` keyword and has not closed yet.
    let Some(keyword) = text[..line_start].rfind("import") else {
        return false;
    };
    let statement = text[..keyword].rfind('\n').map_or(0, |i| i + 1);
    if !starts(text[statement..].trim_start()) {
        return false;
    }
    let (open, close) = if language == Language::Python {
        ('(', ')')
    } else {
        ('{', '}')
    };
    let list = text[keyword + "import".len()..at].trim_start();
    let list = list.strip_prefix("type ").unwrap_or(list).trim_start();
    list.starts_with(open) && !list.contains(close)
}

/// The edit that adds `name` to the import a Python file already has from the declaring module
/// — the one whose last component is `stem` — as (offset, text to insert). `Some` with an empty
/// text when the name is imported already; `None` when there is no such import to extend.
fn python_import_edit(text: &str, stem: &str, name: &str) -> Option<(usize, String)> {
    let mut line_start = 0usize;
    for line in text.split_inclusive('\n') {
        let at = line_start;
        line_start += line.len();
        let Some(rest) = line.strip_prefix("from ") else {
            continue;
        };
        let Some((module, names)) = rest.split_once(" import ") else {
            continue;
        };
        if module.trim().rsplit('.').next() != Some(stem) {
            continue;
        }
        let names_at = at + line.len() - names.len();
        if names.trim_start().starts_with('(') {
            let open = names_at + names.len() - names.trim_start().len();
            let close = open + 1 + text[open + 1..].find(')')?;
            if text[open + 1..close].split(',').any(|n| n.trim() == name) {
                return Some((close, String::new()));
            }
            // A list that ends in a trailing comma keeps one.
            let before = text[..close].trim_end();
            return Some(if before.ends_with(',') {
                (before.len(), format!(" {name},"))
            } else {
                (before.len(), format!(", {name}"))
            });
        }
        let list = names.split('#').next().unwrap_or("");
        if list.split(',').any(|n| n.trim() == name) {
            return Some((at, String::new()));
        }
        return Some((names_at + list.trim_end().len(), format!(", {name}")));
    }
    None
}

/// Where `from dataclasses import dataclass` goes, and the text to insert there: after the last
/// top-level import, or after the module's docstring, or at the very top. `None` when the file
/// imports `dataclass` already.
fn dataclass_import(text: &str) -> Option<(usize, String)> {
    const LINE: &str = "from dataclasses import dataclass";
    let mut after_imports = None;
    let mut offset = 0usize;
    let mut open_paren = false;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        if open_paren {
            if line.contains(')') {
                open_paren = false;
                after_imports = Some(offset);
            }
            continue;
        }
        if let Some(names) = line.strip_prefix("from dataclasses import ")
            && names
                .split([',', '(', ')', ' ', '\n'])
                .any(|n| n.trim() == "dataclass")
        {
            return None;
        }
        if line.starts_with("import ") || line.starts_with("from ") {
            open_paren = line.contains('(') && !line.contains(')');
            if !open_paren {
                after_imports = Some(offset);
            }
        }
    }
    if let Some(at) = after_imports {
        let tail = if text[..at].ends_with('\n') { "" } else { "\n" };
        return Some((at, format!("{tail}{LINE}\n")));
    }
    for quote in ["\"\"\"", "'''"] {
        if let Some(rest) = text.strip_prefix(quote)
            && let Some(n) = rest.find(quote)
        {
            let end = line_end(text.as_bytes(), quote.len() + n + quote.len());
            let at = (end + 1).min(text.len());
            return Some((at, format!("\n{LINE}\n")));
        }
    }
    Some((0, format!("{LINE}\n\n")))
}

/// Bundling in a TypeScript, Python, Go or Swift file: the same three edits as in Rust, with the
/// text each language writes them in.
#[allow(clippy::too_many_arguments)]
async fn introduce_in(
    language: Language,
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    params: &[String],
    name: &str,
    binding: &str,
    apply: bool,
    force: bool,
) -> Result<ParameterObject> {
    anyhow::ensure!(
        !name.is_empty() && name.bytes().all(is_ident_byte) && !name.as_bytes()[0].is_ascii_digit(),
        "`{name}` is not a type name"
    );
    // A Go type in lower case is an unexported one, which is a choice, not a mistake.
    anyhow::ensure!(
        language == Language::Go || name.starts_with(|c: char| c.is_ascii_uppercase()),
        "`{name}` is not a type name; {} types are UpperCamelCase",
        language.label()
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset = crate::signature::offset_of(&text, line, col)
        .context("the declaration is not at the resolved position")?;
    let (callee, open, close) = crate::signature::param_span(&text, offset)
        .with_context(|| format!("no function declaration at {}:{line}:{col}", file.display()))?;
    let old_inner = text[open..close].to_string();
    let (receiver, declared) = parse_params(&old_inner, language);

    for p in params {
        anyhow::ensure!(
            declared
                .iter()
                .any(|d| &d.name == p && d.kind != Kind::Marker),
            "`{p}` is not a parameter of `{callee}`; it declares ({})",
            declared
                .iter()
                .filter(|d| d.kind != Kind::Marker)
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let bundled: Vec<usize> = declared
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind != Kind::Marker && params.iter().any(|p| p == &d.name))
        .map(|(i, _)| i)
        .collect();
    anyhow::ensure!(bundled.len() == params.len(), "a parameter was named twice");
    for i in &bundled {
        anyhow::ensure!(
            declared[*i].kind == Kind::Plain,
            "`{}` takes a variable number of arguments, and a field holds one value",
            declared[*i].name
        );
        // What the body writes to an `inout` parameter reaches the caller; written to a field
        // of a copy, it would not.
        anyhow::ensure!(
            !(language == Language::Swift
                && declared[*i]
                    .ty
                    .as_deref()
                    .is_some_and(|t| t.starts_with("inout "))),
            "`{}` is `inout`, and a field of the new type would be a copy of it",
            declared[*i].name
        );
    }

    let mut fields = Vec::with_capacity(bundled.len());
    for i in &bundled {
        let p = &declared[*i];
        let ty = match &p.ty {
            Some(ty) => Some(ty.clone()),
            None => hover_type(remote, root, file, &text, open + p.name_at, &p.name).await,
        };
        fields.push(Field {
            name: p.name.clone(),
            ty,
            // A TypeScript interface has no defaults; the field keeps the parameter's type.
            default: p
                .default
                .clone()
                .filter(|_| matches!(language, Language::Python | Language::Swift)),
            optional: p.optional,
        });
    }

    let decl_line = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let top = top_level_line(&text, offset);
    let item_start = item_start_in(&text, top, language);
    let export = match language {
        Language::TypeScript => text[top..].starts_with("export "),
        Language::Swift => {
            let modifiers = &text[decl_line..offset];
            modifiers.contains("public ") || modifiers.contains("open ")
        }
        _ => false,
    };
    let is_method = match language {
        Language::Go => text[decl_line..offset].trim_start().starts_with("func ("),
        _ => receiver.is_some() || top != decl_line,
    };
    let type_decl = type_text(
        language,
        name,
        &callee,
        &fields,
        &indent_unit(&text, language),
        export,
    );

    // The uses in the body, at the positions the analyzer reports.
    let body = body_span(&text, offset, close, language);
    let mut uses: Vec<(usize, usize, String)> = Vec::new();
    for i in &bundled {
        let p = &declared[*i];
        let (l, c) = crate::signature::line_col_at(&text, open + p.name_at);
        for (path, rl, rc) in crate::signature::references(remote, root, file, l, c)
            .await
            .unwrap_or_default()
        {
            if path != file {
                continue;
            }
            let Some(o) = crate::signature::offset_of(&text, rl, rc) else {
                continue;
            };
            if o <= body.0 || o >= body.1 || !text[o..].starts_with(&p.name) {
                continue;
            }
            // basedpyright counts the name of a keyword argument (`height=…`) as a reference;
            // in a call in the body that is the callee's parameter, not a use of this one.
            let after = text[o + p.name.len()..].trim_start();
            if language == Language::Python && after.starts_with('=') && !after.starts_with("==") {
                continue;
            }
            uses.push((o, p.name.len(), format!("{binding}.{}", p.name)));
        }
    }
    uses.sort();
    uses.dedup();
    let body_uses = uses.len();

    let texts = |path: &Path| -> String {
        if path == file {
            text.clone()
        } else {
            std::fs::read_to_string(path).unwrap_or_default()
        }
    };
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    let mut imports = Vec::new();
    let mut call_sites = 0usize;
    let mut consumed = vec![false; uses.len()];
    let mut bare_callers: Vec<PathBuf> = Vec::new();
    for (path, rl, rc) in crate::signature::references(remote, root, file, line, col)
        .await
        .unwrap_or_default()
    {
        let source = texts(&path);
        let Some(at) = crate::signature::offset_of(&source, rl, rc) else {
            continue;
        };
        // As in Rust (#75): the position is trusted only when the name is there.
        if !source[at..].starts_with(callee.as_str()) {
            unmatched.push(format!(
                "{}:{rl}:{rc} (the analyzer places `{callee}` here, but the file says otherwise)",
                display(root, &path)
            ));
            continue;
        }
        let Some((args_start, args_end)) = call_args_in(&source, at + callee.len(), language)
        else {
            if !in_import(&source, at, language) {
                unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            }
            continue;
        };
        // A call in the function's own body passes arguments that are uses of the bundled
        // parameters themselves. One edit cannot sit inside another, so those uses are
        // rewritten inside the argument text.
        let mut inner = source[args_start..args_end].to_string();
        let mut inside = Vec::new();
        if path == file {
            for (n, (o, len, replacement)) in uses.iter().enumerate().rev() {
                if *o >= args_start && o + len <= args_end {
                    inner.replace_range(o - args_start..o - args_start + len, replacement);
                    inside.push(n);
                }
            }
        }
        let args: Vec<String> = entries(&inner, language)
            .into_iter()
            .map(|(_, a)| a.to_string())
            .collect();
        let Some(bound) = bind_arguments(&args, &declared, language) else {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        };
        // A method's qualifier is the object it is called on, not where the type lives, and a
        // TypeScript literal names no type at all.
        let qualifier = if is_method || language == Language::TypeScript {
            ""
        } else {
            qualifier_before(&source, at)
        };
        let spelling = format!("{qualifier}{name}");
        let new_args = rewritten_call(
            &args, &bound, &bundled, &declared, language, &spelling, binding,
        );
        for n in inside {
            consumed[n] = true;
        }
        if language == Language::Python
            && qualifier.is_empty()
            && path != file
            && !bare_callers.contains(&path)
        {
            bare_callers.push(path.clone());
        }
        edits
            .entry(path)
            .or_default()
            .push((args_start, args_end - args_start, new_args));
        call_sites += 1;
    }
    for (n, used) in uses.into_iter().enumerate() {
        if !consumed[n] {
            edits.entry(file.to_path_buf()).or_default().push(used);
        }
    }

    // The declaration itself. A Swift parameter keeps the first bundled one's lack of a label,
    // and a default when every bundled one had a default, so that a call that passed none of
    // them still compiles unchanged.
    let mut parameter = parameter_in(language, binding, name);
    if language == Language::Swift {
        if bundled
            .first()
            .is_some_and(|p| declared[*p].label.is_none())
        {
            parameter = format!("_ {parameter}");
        }
        if bundled.iter().all(|p| declared[*p].default.is_some()) {
            parameter.push_str(&format!(" = {name}()"));
        }
    }
    let now = {
        let mut out: Vec<String> = Vec::new();
        if let Some(r) = &receiver {
            out.push(r.trim().to_string());
        }
        let first = bundled.first().copied().unwrap_or(0);
        for (i, p) in declared.iter().enumerate() {
            if i == first {
                out.push(parameter.clone());
            } else if bundled.contains(&i) {
                continue;
            } else if p.shares_type && bundled.contains(&(i + 1)) {
                // The name the type was written after is leaving the group.
                out.push(format!(
                    "{} {}",
                    p.name,
                    p.ty.as_deref().unwrap_or_default()
                ));
            } else {
                out.push(p.raw.clone());
            }
        }
        out.join(", ")
    };
    let declaring = edits.entry(file.to_path_buf()).or_default();
    declaring.push((open, close - open, now.clone()));
    // The import is pushed before the type: at the same offset, the one pushed first ends up
    // first in the file.
    if type_decl.starts_with("@dataclass")
        && let Some((at, line)) = dataclass_import(&text)
    {
        declaring.push((at, 0, line));
        imports.push(format!(
            "{}: added `from dataclasses import dataclass`",
            display(root, file)
        ));
    }
    // Python separates top-level definitions with two blank lines, the others with one.
    let gap = if language == Language::Python {
        "\n\n"
    } else {
        "\n"
    };
    declaring.push((item_start, 0, format!("{type_decl}{gap}")));

    // A Python caller in another module names the type bare, so it has to import it, from the
    // module it already imports the function (or the class) from.
    let stem = match file.file_stem().and_then(|s| s.to_str()) {
        Some("__init__") => file
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or(""),
        Some(stem) => stem,
        None => "",
    };
    for path in bare_callers {
        let source = texts(&path);
        match python_import_edit(&source, stem, name) {
            Some((_, insert)) if insert.is_empty() => {}
            Some((at, insert)) => {
                edits.entry(path.clone()).or_default().push((at, 0, insert));
                imports.push(format!(
                    "{}: added `{name}` to the import from `{stem}`",
                    display(root, &path)
                ));
            }
            None => imports.push(format!(
                "{}: needs `{name}` imported; it has no `from … import` of `{stem}` to add it to",
                display(root, &path)
            )),
        }
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut source = texts(&path);
        file_edits.sort_by_key(|(at, _, _)| *at);
        for (at, len, replacement) in file_edits.into_iter().rev() {
            source.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, source);
    }

    let (diagnostics, applied) = check_and_apply(remote, root, &rewritten, apply, force).await?;

    Ok(ParameterObject {
        symbol: callee,
        root: root.to_path_buf(),
        file: display(root, file),
        struct_text: type_decl,
        was: old_inner.split_whitespace().collect::<Vec<_>>().join(" "),
        now,
        call_sites,
        imports,
        body_uses,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        unmatched,
        diagnostics,
        applied,
        language: language.fence(),
    })
}

/// Every reference to the symbol at `file:line:col`, its declarations included. clangd lists a
/// header's prototype and the definition only when it is asked for declarations, so what this
/// has and [`crate::signature::references`] does not are the declarations.
async fn references_with_declarations(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<Vec<(PathBuf, u32, u32)>> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    let res = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/references",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            "context": { "includeDeclaration": true },
        }),
    )
    .await?;
    Ok(res
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|loc| {
            let uri = loc.get("uri")?.as_str()?;
            let at = |p: &str| loc.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
            Some((
                PathBuf::from(crate::remote_fs::uri_to_path(uri)),
                at("/range/start/line"),
                at("/range/start/character"),
            ))
        })
        .collect())
}

/// The first line of the outermost declaration around a 0-based line that is not a namespace,
/// in a documentSymbol answer: the class of a method, the function itself otherwise. Lines are
/// enough to tell which declaration holds a name: two declarations do not share one.
fn outermost_container(symbols: &[serde_json::Value], line: u64) -> Option<u64> {
    for s in symbols {
        let Some(range) = s.get("range").or_else(|| s.pointer("/location/range")) else {
            continue;
        };
        let at = |p: &str| range.pointer(p).and_then(|v| v.as_u64());
        let (Some(start), Some(end)) = (at("/start/line"), at("/end/line")) else {
            continue;
        };
        if line < start || line > end {
            continue;
        }
        // Module, namespace and package: the new type belongs inside them, next to the function.
        if matches!(s.get("kind").and_then(|k| k.as_u64()), Some(2..=4)) {
            return s
                .get("children")
                .and_then(|c| c.as_array())
                .and_then(|c| outermost_container(c, line));
        }
        return Some(start);
    }
    None
}

/// The start of the line the new type goes above in a C or C++ file, for the declaration whose
/// name is at `at`: the outermost declaration around it that is not a namespace, as the
/// server's documentSymbol nests them.
///
/// The text's indentation is not enough here, as it is in the other languages: a class's
/// `public:` is written at the class's own indentation often enough, and would be taken for
/// the line the class starts on. Without an answer from the server, the lines that are only an
/// access specifier or a preprocessor directive are passed over.
async fn container_line(
    remote: SocketAddr,
    root: &Path,
    path: &Path,
    text: &str,
    at: usize,
) -> usize {
    let (line, _) = crate::signature::line_col_at(text, at);
    let answer = match url::Url::from_file_path(path) {
        Ok(uri) => crate::tools::execute_lsp_query(
            remote,
            root,
            path,
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": uri.to_string() } }),
        )
        .await
        .ok(),
        Err(_) => None,
    };
    let found = answer
        .as_ref()
        .and_then(|a| a.as_array())
        .and_then(|symbols| outermost_container(symbols, u64::from(line - 1)))
        .and_then(|l| crate::signature::offset_of(text, l as u32 + 1, 1));
    if let Some(start) = found {
        return start;
    }
    let mut start = top_level_line(text, at);
    while start > 0 {
        let code = text[start..line_end(text.as_bytes(), start)].trim();
        let access = matches!(code, "public:" | "private:" | "protected:");
        if !access && !code.starts_with('#') {
            break;
        }
        start = top_level_line(text, start - 1);
    }
    start
}

/// The C++ standard the project builds with, as a year (2017, 2020), from what its build
/// declares: a compilation database or `compile_flags.txt` at the root or in `build/`, else a
/// `CMakeLists.txt` from the file's directory up to the root. `None` when none of them says, and
/// then the compiler's default applies, which is C++17 for current GCC and clang.
fn cpp_standard(root: &Path, file: &Path) -> Option<u32> {
    for name in [
        "compile_commands.json",
        "build/compile_commands.json",
        "compile_flags.txt",
    ] {
        if let Some(year) = std::fs::read_to_string(root.join(name))
            .ok()
            .and_then(|t| std_in_flags(&t))
        {
            return Some(year);
        }
    }
    let mut dir = file.parent();
    while let Some(d) = dir {
        if let Some(year) = std::fs::read_to_string(d.join("CMakeLists.txt"))
            .ok()
            .and_then(|t| std_in_cmake(&t))
        {
            return Some(year);
        }
        if d == root || !d.starts_with(root) {
            break;
        }
        dir = d.parent();
    }
    None
}

/// The year of the first `-std=c++NN` (or `gnu++NN`, or MSVC's `/std:c++NN`) in compiler flags.
fn std_in_flags(text: &str) -> Option<u32> {
    ["std=c++", "std=gnu++", "std:c++"]
        .iter()
        .find_map(|marker| {
            text.match_indices(marker)
                .find_map(|(at, m)| std_year(&text[at + m.len()..]))
        })
}

/// The year a `CMakeLists.txt` asks for: `set(CMAKE_CXX_STANDARD 20)`, `cxx_std_20`, or a flag.
fn std_in_cmake(text: &str) -> Option<u32> {
    text.match_indices("CMAKE_CXX_STANDARD")
        .find_map(|(at, m)| std_year(text[at + m.len()..].trim_start()))
        .or_else(|| {
            text.match_indices("cxx_std_")
                .find_map(|(at, m)| std_year(&text[at + m.len()..]))
        })
        .or_else(|| std_in_flags(text))
}

/// A standard's version as a year: `17` and `1z` are 2017, `20` and `2a` are 2020, `98` is 1998.
fn std_year(version: &str) -> Option<u32> {
    let token: String = version
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    Some(match token.as_str() {
        "98" => 1998,
        "03" => 2003,
        "0x" => 2011,
        "1y" => 2014,
        "1z" => 2017,
        "2a" => 2020,
        "2b" => 2023,
        "2c" => 2026,
        t if t.len() == 2 && t.bytes().all(|b| b.is_ascii_digit()) => {
            2000 + t.parse::<u32>().ok()?
        }
        _ => return None,
    })
}

/// One declaration of the function being bundled: a prototype, or the definition.
struct CDeclaration {
    path: PathBuf,
    text: String,
    /// Where the function's name starts.
    name_at: usize,
    /// The parameter list, between its parentheses.
    open: usize,
    close: usize,
    params: Vec<Param>,
    /// Whether this is the definition, the one with the body.
    body: bool,
}

/// Bundling in C and C++, where the new type is a `struct` and every declaration of the
/// function changes together: the prototype a header gives callers, and the definition.
///
/// The declarations are the references clangd lists only when asked for declarations too. The
/// parameters are named as the definition names them — a prototype may name them differently,
/// or not at all — and matched to a prototype's by position. The type goes into the header
/// when there is one, above the prototype (above the class, for a method), where the
/// definition and every caller already see it.
///
/// A call passes a C99 compound literal in C, `(struct Opts){.a = x, .b = y}`. In C++ a braced
/// list converts to the parameter's type: `{.a = x, .b = y}` when the project builds as C++20
/// or later, whose designated initialisers these are, and `{x, y}` otherwise. GCC and clang
/// accept designators before C++20 as an extension, so the analyzer's check cannot tell the two
/// apart; the standard the build declares does (see [`cpp_standard`]).
#[allow(clippy::too_many_arguments)]
async fn introduce_c(
    language: Language,
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    params: &[String],
    name: &str,
    binding: &str,
    apply: bool,
    force: bool,
) -> Result<ParameterObject> {
    // A C struct is named in lower case as often as not, which is a choice, not a mistake.
    anyhow::ensure!(
        !name.is_empty()
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !name.as_bytes()[0].is_ascii_digit(),
        "`{name}` is not a type name"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset = crate::signature::offset_of(&text, line, col)
        .context("the declaration is not at the resolved position")?;
    let (callee, _, _) = crate::signature::param_span(&text, offset)
        .with_context(|| format!("no function declaration at {}:{line}:{col}", file.display()))?;

    let calls = crate::signature::references(remote, root, file, line, col)
        .await
        .unwrap_or_default();
    let everything = references_with_declarations(remote, root, file, line, col)
        .await
        .unwrap_or_default();
    let mut spots = vec![(file.to_path_buf(), line, col)];
    spots.extend(everything.into_iter().filter(|r| !calls.contains(r)));

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let mut decls: Vec<CDeclaration> = Vec::new();
    for (path, l, c) in spots {
        if !texts.contains_key(&path) {
            let Ok(t) = std::fs::read_to_string(&path) else {
                continue;
            };
            texts.insert(path.clone(), t);
        }
        let t = &texts[&path];
        let Some(at) = crate::signature::offset_of(t, l, c) else {
            continue;
        };
        if !t[at..].starts_with(callee.as_str()) {
            continue;
        }
        let Some((n, open, close)) = crate::signature::param_span(t, at) else {
            continue;
        };
        if n != callee || decls.iter().any(|d| d.path == path && d.open == open) {
            continue;
        }
        decls.push(CDeclaration {
            path,
            text: t.clone(),
            name_at: at,
            open,
            close,
            params: Vec::new(),
            body: false,
        });
    }

    // A `.h` header is C by its name; the function is C++ when anything around it is.
    let language = if language == Language::Cpp
        || decls
            .iter()
            .map(|d| &d.path)
            .chain(calls.iter().map(|(p, _, _)| p))
            .any(|p| Language::of(p) == Some(Language::Cpp))
    {
        Language::Cpp
    } else {
        Language::C
    };
    for d in &mut decls {
        d.body = body_open(&d.text, d.close, language).is_some();
        d.params = parse_params(&d.text[d.open..d.close], language).1;
    }
    let main = decls.iter().position(|d| d.body).unwrap_or(0);
    let count = decls[main].params.len();
    for d in &decls {
        anyhow::ensure!(
            d.params.len() == count,
            "{} declares `{callee}` with {} parameter(s) and {} with {count}; bundling needs \
             every declaration to list the same ones",
            display(root, &d.path),
            d.params.len(),
            display(root, &decls[main].path)
        );
    }
    let mut declared = decls[main].params.clone();
    // C++ gives a default on one declaration only, the header's as a rule.
    for (i, p) in declared.iter_mut().enumerate() {
        if p.default.is_none() {
            p.default = decls.iter().find_map(|d| d.params[i].default.clone());
        }
    }

    for p in params {
        anyhow::ensure!(
            declared.iter().any(|d| &d.name == p),
            "`{p}` is not a parameter of `{callee}`; its definition declares ({})",
            declared
                .iter()
                .filter(|d| !d.name.is_empty())
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let bundled: Vec<usize> = declared
        .iter()
        .enumerate()
        .filter(|(_, d)| !d.name.is_empty() && params.iter().any(|p| p == &d.name))
        .map(|(i, _)| i)
        .collect();
    anyhow::ensure!(bundled.len() == params.len(), "a parameter was named twice");
    for i in &bundled {
        anyhow::ensure!(
            declared[*i].kind == Kind::Plain,
            "`{}` takes a variable number of arguments, and a field holds one value",
            declared[*i].name
        );
    }
    let fields: Vec<Field> = bundled
        .iter()
        .map(|i| {
            let p = &declared[*i];
            Field {
                name: p.name.clone(),
                ty: p.ty.clone(),
                default: p.default.clone().filter(|_| language == Language::Cpp),
                optional: false,
            }
        })
        .collect();

    let home = decls
        .iter()
        .position(|d| crate::lang::is_header(&d.path))
        .unwrap_or(main);
    let home_path = decls[home].path.clone();
    let top = container_line(
        remote,
        root,
        &home_path,
        &decls[home].text,
        decls[home].name_at,
    )
    .await;
    let item_start = item_start_in(&decls[home].text, top, language);
    let indent = detected_indent(&decls[home].text)
        .or_else(|| detected_indent(&decls[main].text))
        .unwrap_or_else(|| "    ".to_string());
    let type_decl = type_text(language, name, &callee, &fields, &indent, false);

    // The uses in the definition's body, at the positions the analyzer reports.
    let def_path = decls[main].path.clone();
    let mut uses: Vec<(usize, usize, String)> = Vec::new();
    if decls[main].body {
        let def = &decls[main];
        let body = body_span(&def.text, def.name_at, def.close, language);
        for i in &bundled {
            let p = &declared[*i];
            let (l, c) = crate::signature::line_col_at(&def.text, def.open + p.name_at);
            for (path, rl, rc) in crate::signature::references(remote, root, &def.path, l, c)
                .await
                .unwrap_or_default()
            {
                if path != def.path {
                    continue;
                }
                let Some(o) = crate::signature::offset_of(&def.text, rl, rc) else {
                    continue;
                };
                if o <= body.0 || o >= body.1 || !def.text[o..].starts_with(&p.name) {
                    continue;
                }
                uses.push((o, p.name.len(), format!("{binding}.{}", p.name)));
            }
        }
    }
    uses.sort();
    uses.dedup();
    let body_uses = uses.len();

    let designated =
        language == Language::C || cpp_standard(root, &def_path).is_some_and(|year| year >= 2020);
    let literal = |pairs: &[(String, String)]| {
        if designated {
            literal_text(language, name, pairs)
        } else {
            aggregate_text(pairs)
        }
    };
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    let mut call_sites = 0usize;
    let mut consumed = vec![false; uses.len()];
    for (path, rl, rc) in calls {
        if !texts.contains_key(&path) {
            let t = std::fs::read_to_string(&path).unwrap_or_default();
            texts.insert(path.clone(), t);
        }
        let source = &texts[&path];
        let Some(at) = crate::signature::offset_of(source, rl, rc) else {
            continue;
        };
        // As in Rust (#75): the position is trusted only when the name is there.
        if !source[at..].starts_with(callee.as_str()) {
            unmatched.push(format!(
                "{}:{rl}:{rc} (the analyzer places `{callee}` here, but the file says otherwise)",
                display(root, &path)
            ));
            continue;
        }
        let Some((args_start, args_end)) = call_args_in(source, at + callee.len(), language) else {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        };
        // A recursive call passes uses of the bundled parameters; see `introduce_in`.
        let mut inner = source[args_start..args_end].to_string();
        let mut inside = Vec::new();
        if path == def_path {
            for (n, (o, len, replacement)) in uses.iter().enumerate().rev() {
                if *o >= args_start && o + len <= args_end {
                    inner.replace_range(o - args_start..o - args_start + len, replacement);
                    inside.push(n);
                }
            }
        }
        let args: Vec<String> = entries(&inner, language)
            .into_iter()
            .map(|(_, a)| a.to_string())
            .collect();
        let Some(bound) = bind_arguments(&args, &declared, language) else {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        };
        let new_args = rewritten_call_with(
            &args, &bound, &bundled, &declared, language, binding, literal,
        );
        for n in inside {
            consumed[n] = true;
        }
        edits
            .entry(path)
            .or_default()
            .push((args_start, args_end - args_start, new_args));
        call_sites += 1;
    }
    for (n, used) in uses.into_iter().enumerate() {
        if !consumed[n] {
            edits.entry(def_path.clone()).or_default().push(used);
        }
    }

    // Every declaration, each keeping the parameters it wrote as it wrote them. A C++ default
    // stays on the declaration that gave it; the new parameter gets one, `{}`, which the
    // fields' own defaults fill in, where every bundled parameter had one.
    let first = bundled.first().copied().unwrap_or(0);
    let mut now = String::new();
    for (k, d) in decls.iter().enumerate() {
        let defaulted =
            language == Language::Cpp && bundled.iter().all(|i| d.params[*i].default.is_some());
        let mut out: Vec<String> = Vec::new();
        for (i, p) in d.params.iter().enumerate() {
            if i == first {
                let mut parameter = parameter_in(language, binding, name);
                if defaulted {
                    parameter.push_str(" = {}");
                }
                out.push(parameter);
            } else if !bundled.contains(&i) {
                out.push(p.raw.clone());
            }
        }
        let list = out.join(", ");
        if k == main {
            now = list.clone();
        }
        edits
            .entry(d.path.clone())
            .or_default()
            .push((d.open, d.close - d.open, list));
    }
    edits
        .entry(home_path)
        .or_default()
        .push((item_start, 0, format!("{type_decl}\n")));

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut source = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, _, _)| *at);
        for (at, len, replacement) in file_edits.into_iter().rev() {
            source.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, source);
    }

    let (diagnostics, applied) = check_and_apply(remote, root, &rewritten, apply, force).await?;
    let was = &decls[main].text[decls[main].open..decls[main].close];
    Ok(ParameterObject {
        symbol: callee,
        root: root.to_path_buf(),
        file: display(root, file),
        struct_text: type_decl,
        was: was.split_whitespace().collect::<Vec<_>>().join(" "),
        now,
        call_sites,
        imports: Vec::new(),
        body_uses,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        unmatched,
        diagnostics,
        applied,
        language: language.fence(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parameters_type_is_what_follows_its_first_top_level_colon() {
        assert_eq!(type_of("name: &str"), Some("&str"));
        assert_eq!(
            type_of("map: BTreeMap<String, Vec<u8>>"),
            Some("BTreeMap<String, Vec<u8>>")
        );
        assert_eq!(type_of("p: std::path::PathBuf"), Some("std::path::PathBuf"));
        assert_eq!(type_of("self"), None);
    }

    #[test]
    fn a_borrow_without_a_lifetime_makes_the_struct_take_one() {
        assert!(needs_lifetime("&str"));
        assert!(needs_lifetime("&[u8]"));
        assert!(!needs_lifetime("String"));
        assert!(!needs_lifetime("&'static str"));
        assert_eq!(with_lifetime("&str"), "&'a str");
        assert_eq!(with_lifetime("&[u8]"), "&'a [u8]");
        assert_eq!(with_lifetime("String"), "String");
    }

    #[test]
    fn the_struct_keeps_the_declared_types_and_the_field_order() {
        let fields = vec![
            ("text".to_string(), "&str".to_string()),
            ("count".to_string(), "usize".to_string()),
        ];
        let text = struct_text("Opts", &fields, "The parameters `f` takes together.");
        assert_eq!(
            text,
            "/// The parameters `f` takes together.\npub struct Opts<'a> {\n    pub text: &'a str,\n    pub count: usize,\n}\n"
        );
        assert_eq!(parameter_text("opts", "Opts", &fields), "opts: Opts<'_>");

        let owned = vec![("count".to_string(), "usize".to_string())];
        assert_eq!(
            struct_text("Opts", &owned, ""),
            "pub struct Opts {\n    pub count: usize,\n}\n"
        );
        assert_eq!(parameter_text("opts", "Opts", &owned), "opts: Opts");
    }

    #[test]
    fn a_bracket_in_a_comment_or_a_literal_does_not_close_the_block() {
        let block = "impl A {\n    // don't stop at } here\n    /* nor } here */\n    fn f() -> &'static str { \"}\" }\n    fn g() -> char { '}' }\n}\ntail";
        let close = matching_bracket(block, block.find('{').unwrap()).expect("it closes");
        assert_eq!(&block[close..], "}\ntail");
        assert_eq!(
            matching_bracket("(a, [b)", 0),
            None,
            "an unclosed list has no end"
        );
        assert_eq!(
            matching_bracket("x", 0),
            None,
            "only a bracket opens a block"
        );
    }

    #[test]
    fn an_argument_list_survives_closures_strings_and_chains() {
        let call = "f(a, |x, y| x + y, \"one, two\", b.iter().map(|v| v).collect())";
        let (start, end) = call_args_span(call, 1).expect("a call");
        let args = split_args(&call[start..end]);
        assert_eq!(
            args,
            vec![
                "a",
                "|x, y| x + y",
                "\"one, two\"",
                "b.iter().map(|v| v).collect()"
            ]
        );
        assert!(
            call_args_span("let g = f;", 8).is_none(),
            "a use that is not a call has no argument list"
        );
    }

    #[test]
    fn the_bundled_arguments_become_one_literal_where_the_first_of_them_was() {
        let fields = vec![
            ("b".to_string(), "u8".to_string()),
            ("c".to_string(), "u8".to_string()),
        ];
        let args: Vec<String> = ["w", "x", "y", "z"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            rewritten_args(&args, &[1, 2], "Opts", &fields),
            "w, Opts { b: x, c: y }, z"
        );
        assert_eq!(
            rewritten_args(&args, &[0, 3], "Opts", &fields),
            "Opts { b: w, c: z }, x, y"
        );
        // A file that cannot import it names it in full.
        assert_eq!(
            rewritten_args(&args, &[1, 2], "the_crate::home::Opts", &fields),
            "w, the_crate::home::Opts { b: x, c: y }, z"
        );
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_language_is_the_files_and_javascript_is_not_typescript() {
        assert_eq!(Language::of(Path::new("a/b.rs")), Some(Language::Rust));
        assert_eq!(
            Language::of(Path::new("src/home.ts")),
            Some(Language::TypeScript)
        );
        assert_eq!(
            Language::of(Path::new("src/view.tsx")),
            Some(Language::TypeScript)
        );
        assert_eq!(
            Language::of(Path::new("app/home.py")),
            Some(Language::Python)
        );
        assert_eq!(
            Language::of(Path::new("shapes/home.go")),
            Some(Language::Go)
        );
        assert_eq!(Language::of(Path::new("src/home.js")), None);
        assert_eq!(
            default_binding(Path::new("a.rs"), "SyncRequest"),
            "sync_request"
        );
        assert_eq!(
            default_binding(Path::new("a.py"), "SyncRequest"),
            "sync_request"
        );
        assert_eq!(
            default_binding(Path::new("a.ts"), "SyncRequest"),
            "syncRequest"
        );
        assert_eq!(
            default_binding(Path::new("a.go"), "HTTPOptions"),
            "httpOptions"
        );
        assert_eq!(default_binding(Path::new("a.go"), "URL"), "url");
    }

    #[test]
    fn a_typescript_list_keeps_optional_marks_defaults_and_nested_type_arguments() {
        let list = "this: Canvas, label: string, x?: number, m: Map<string, number> = new Map(), cb: (a: number, b: number) => void, public readonly id: string, ...rest: number[]";
        let (receiver, params) = parse_params(list, Language::TypeScript);
        assert_eq!(receiver.as_deref(), Some("this: Canvas"));
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["label", "x", "m", "cb", "id", "rest"]);
        assert!(params[1].optional);
        assert_eq!(params[1].ty.as_deref(), Some("number"));
        assert_eq!(params[2].ty.as_deref(), Some("Map<string, number>"));
        assert_eq!(params[2].default.as_deref(), Some("new Map()"));
        assert_eq!(
            params[3].ty.as_deref(),
            Some("(a: number, b: number) => void")
        );
        assert_eq!(params[3].default, None, "`=>` is not a default");
        assert_eq!(&list[params[4].name_at..params[4].name_at + 2], "id");
        assert_eq!(params[5].kind, Kind::Variadic);
    }

    #[test]
    fn a_python_list_has_a_receiver_markers_and_defaults_with_commas_in_them() {
        let list =
            "self, name: str, width: int, height: int = 2, *, key=\"a, b\",  # the key\n    **kw";
        let (receiver, params) = parse_params(list, Language::Python);
        assert_eq!(receiver.as_deref(), Some("self"));
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["name", "width", "height", "*", "key", "kw"]);
        assert_eq!(params[2].ty.as_deref(), Some("int"));
        assert_eq!(params[2].default.as_deref(), Some("2"));
        assert_eq!(params[3].kind, Kind::Marker);
        assert_eq!(params[4].ty, None);
        assert_eq!(params[4].default.as_deref(), Some("\"a, b\""));
        assert_eq!(params[5].kind, Kind::Keywords);
        assert_eq!(
            params[5].raw, "**kw",
            "the comment before it is not part of it"
        );
    }

    #[test]
    fn a_go_group_gives_every_name_its_type() {
        let (receiver, params) = parse_params(
            "name string, width, height int, rest ...string",
            Language::Go,
        );
        assert_eq!(receiver, None);
        let typed: Vec<(&str, &str, bool)> = params
            .iter()
            .map(|p| {
                (
                    p.name.as_str(),
                    p.ty.as_deref().unwrap_or(""),
                    p.shares_type,
                )
            })
            .collect();
        assert_eq!(
            typed,
            [
                ("name", "string", false),
                ("width", "int", true),
                ("height", "int", false),
                ("rest", "...string", false)
            ]
        );
        assert_eq!(params[3].kind, Kind::Variadic);
    }

    /// The hovers here are what the TypeScript server and basedpyright answer on a parameter.
    #[test]
    fn a_hover_gives_a_parameters_type_unless_the_server_does_not_know_it() {
        assert_eq!(
            hover_parameter_type("```typescript\n(parameter) width: number\n```\n", "width")
                .as_deref(),
            Some("number")
        );
        assert_eq!(
            hover_parameter_type("```typescript\n(parameter) text: any\n```\n", "text").as_deref(),
            Some("any")
        );
        assert_eq!(
            hover_parameter_type("```python\n(parameter) width: int\n```", "width").as_deref(),
            Some("int")
        );
        assert_eq!(
            hover_parameter_type("```python\n(parameter) a: Unknown\n```", "a"),
            None
        );
        assert_eq!(
            hover_parameter_type("```go\nvar width int\n```", "width"),
            None
        );
    }

    fn field(name: &str, ty: Option<&str>, default: Option<&str>) -> Field {
        Field {
            name: name.to_string(),
            ty: ty.map(str::to_string),
            default: default.map(str::to_string),
            optional: false,
        }
    }

    #[test]
    fn each_language_declares_the_type_its_own_way() {
        let fields = [
            field("width", Some("int"), None),
            field("height", Some("int"), Some("2")),
        ];
        assert_eq!(
            type_text(Language::Python, "Size", "build", &fields, "    ", false),
            "@dataclass\nclass Size:\n    \"\"\"The parameters `build` takes together.\"\"\"\n\n    width: int\n    height: int = 2\n"
        );
        let untyped = [field("a", None, None), field("b", None, Some("4"))];
        assert_eq!(
            type_text(Language::Python, "Pair", "loose", &untyped, "    ", false),
            "class Pair:\n    \"\"\"The parameters `loose` takes together.\"\"\"\n\n    def __init__(self, a, b=4):\n        self.a = a\n        self.b = b\n"
        );
        let out_of_order = [
            field("a", Some("int"), Some("1")),
            field("b", Some("int"), None),
        ];
        assert!(
            type_text(Language::Python, "P", "f", &out_of_order, "    ", false)
                .starts_with("@dataclass(kw_only=True)\n")
        );
        let mut optional = field("y", Some("number"), None);
        optional.optional = true;
        assert_eq!(
            type_text(
                Language::TypeScript,
                "Point",
                "draw",
                &[field("x", Some("number"), None), optional],
                "  ",
                true
            ),
            "/** The parameters `draw` takes together. */\nexport interface Point {\n  x: number;\n  y?: number;\n}\n"
        );
        assert_eq!(
            type_text(
                Language::Go,
                "Size",
                "Build",
                &[
                    field("width", Some("int"), None),
                    field("h", Some("int"), None)
                ],
                "\t",
                false
            ),
            "// Size holds the parameters Build takes together.\ntype Size struct {\n\twidth int\n\th     int\n}\n"
        );
    }

    #[test]
    fn a_python_call_binds_by_position_and_by_keyword() {
        let (_, params) = parse_params("name: str, width: int, height: int = 2", Language::Python);
        let bundled = [1, 2];
        let rewrite = |args: &[&str]| {
            let args = strings(args);
            bind_arguments(&args, &params, Language::Python).map(|bound| {
                rewritten_call(
                    &args,
                    &bound,
                    &bundled,
                    &params,
                    Language::Python,
                    "Size",
                    "size",
                )
            })
        };
        assert_eq!(
            rewrite(&["name", "1", "height=2"]).as_deref(),
            Some("name, Size(width=1, height=2)")
        );
        assert_eq!(
            rewrite(&["name", "1"]).as_deref(),
            Some("name, Size(width=1)"),
            "a default the call relied on is the field's default"
        );
        assert_eq!(
            rewrite(&["name", "height=3", "width=1"]).as_deref(),
            Some("name, size=Size(width=1, height=3)"),
            "after a keyword argument the literal is passed by keyword"
        );
        assert_eq!(
            rewrite(&["width=1", "name=n"]).as_deref(),
            Some("size=Size(width=1), name=n")
        );
        assert_eq!(rewrite(&["name"]), None, "`width` has no default");
        assert_eq!(rewrite(&["*xs"]), None, "a spread cannot be mapped");
        assert_eq!(rewrite(&["a", "b", "c", "d"]), None, "too many");
        assert_eq!(
            rewrite(&["a == b", "1", "2"]).as_deref(),
            Some("a == b, Size(width=1, height=2)"),
            "a comparison is not a keyword argument"
        );
    }

    #[test]
    fn typescript_and_go_calls_bind_by_position_with_the_declared_arity() {
        let (_, params) = parse_params("label: string, x: number, y: number", Language::TypeScript);
        let args = strings(&["\"b\"", "5", "6"]);
        let bound = bind_arguments(&args, &params, Language::TypeScript).expect("same arity");
        assert_eq!(
            rewritten_call(
                &args,
                &bound,
                &[1, 2],
                &params,
                Language::TypeScript,
                "Point",
                "point"
            ),
            "\"b\", { x: 5, y: 6 }"
        );
        assert!(bind_arguments(&strings(&["\"b\"", "5"]), &params, Language::TypeScript).is_none());

        let (_, params) = parse_params("name string, width, height int", Language::Go);
        let args = strings(&["\"a\"", "3", "4"]);
        let bound = bind_arguments(&args, &params, Language::Go).expect("same arity");
        assert_eq!(
            rewritten_call(
                &args,
                &bound,
                &[1, 2],
                &params,
                Language::Go,
                "shapes.Size",
                "size"
            ),
            "\"a\", shapes.Size{width: 3, height: 4}"
        );
    }

    #[test]
    fn strings_and_comments_are_skipped_the_way_each_language_writes_them() {
        // An apostrophe opens a string in these languages, not a lifetime.
        let ts = "f('a, (b', `c, ${d}`, /* e, ) */ g) // h, )\n";
        let close = close_in(ts, 1, Language::TypeScript).expect("it closes");
        assert_eq!(&ts[close..close + 1], ")");
        let args: Vec<&str> = entries(&ts[2..close], Language::TypeScript)
            .into_iter()
            .map(|(_, a)| a)
            .collect();
        assert_eq!(args, ["'a, (b'", "`c, ${d}`", "g"]);
        // `//` divides in Python; `#` comments.
        let py = "f(a // 2, \"\"\"x, )\"\"\", b)  # c, )\n";
        let close = close_in(py, 1, Language::Python).expect("it closes");
        let args: Vec<&str> = entries(&py[2..close], Language::Python)
            .into_iter()
            .map(|(_, a)| a)
            .collect();
        assert_eq!(args, ["a // 2", "\"\"\"x, )\"\"\"", "b"]);
        let go = "f('(', `a, \\`, b)";
        let close = close_in(go, 1, Language::Go).expect("it closes");
        assert_eq!(close, go.len() - 1, "a Go raw string has no escapes");
    }

    #[test]
    fn a_python_import_from_the_declaring_module_gains_the_type() {
        let one_line = "from app.home import build, Canvas\n\nx = 1\n";
        let (at, insert) = python_import_edit(one_line, "home", "Size").expect("an import");
        let mut out = one_line.to_string();
        out.insert_str(at, &insert);
        assert_eq!(out, "from app.home import build, Canvas, Size\n\nx = 1\n");

        let wrapped = "from .home import (\n    build,\n)\n";
        let (at, insert) = python_import_edit(wrapped, "home", "Size").expect("an import");
        let mut out = wrapped.to_string();
        out.insert_str(at, &insert);
        assert_eq!(out, "from .home import (\n    build, Size,\n)\n");

        let (_, insert) =
            python_import_edit("from app.home import Size, build\n", "home", "Size").unwrap();
        assert!(insert.is_empty(), "already imported");
        assert!(python_import_edit("import app.home\n", "home", "Size").is_none());

        assert_eq!(
            dataclass_import("\"\"\"Shapes.\"\"\"\n\nimport math\n\n\ndef f():\n    pass\n"),
            Some((27, "from dataclasses import dataclass\n".to_string()))
        );
        assert_eq!(
            dataclass_import("from dataclasses import dataclass, field\n"),
            None
        );
        assert_eq!(
            dataclass_import("def f():\n    pass\n"),
            Some((0, "from dataclasses import dataclass\n\n".to_string()))
        );
    }

    #[test]
    fn a_c_parameter_is_named_by_its_declarator() {
        let list = "const char *name, int xs[], char grid[][4], int (*cb)(int), const std::map<int, int> &m, int height = 2, unsigned, ...";
        let (receiver, params) = parse_params(list, Language::Cpp);
        assert_eq!(receiver, None);
        let named: Vec<(&str, &str)> = params
            .iter()
            .map(|p| (p.name.as_str(), p.ty.as_deref().unwrap_or("")))
            .collect();
        assert_eq!(
            named,
            [
                ("name", "const char *name"),
                ("xs", "int *xs"),
                ("grid", "char (*grid)[4]"),
                ("cb", "int (*cb)(int)"),
                ("m", "const std::map<int, int> &m"),
                ("height", "int height"),
                ("", "unsigned"),
                ("", "")
            ]
        );
        assert_eq!(&list[params[3].name_at..params[3].name_at + 2], "cb");
        assert_eq!(params[5].default.as_deref(), Some("2"));
        assert_eq!(params[7].kind, Kind::Variadic);
        assert!(parse_params(" void ", Language::C).1.is_empty());
    }

    #[test]
    fn a_swift_parameter_has_a_label_a_type_and_a_default() {
        let list = "_ label: String, with name: String, x: Int, y: Int = 0, cb: @escaping (Int) -> Void, d: [String: Int], xs: Int...";
        let (_, params) = parse_params(list, Language::Swift);
        let seen: Vec<(Option<&str>, &str, &str)> = params
            .iter()
            .map(|p| {
                (
                    p.label.as_deref(),
                    p.name.as_str(),
                    p.ty.as_deref().unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                (None, "label", "String"),
                (Some("with"), "name", "String"),
                (Some("x"), "x", "Int"),
                (Some("y"), "y", "Int"),
                (Some("cb"), "cb", "@escaping (Int) -> Void"),
                (Some("d"), "d", "[String: Int]"),
                (Some("xs"), "xs", "Int...")
            ]
        );
        assert_eq!(&list[params[1].name_at..params[1].name_at + 4], "name");
        assert_eq!(params[3].default.as_deref(), Some("0"));
        assert_eq!(params[6].kind, Kind::Variadic);
    }

    #[test]
    fn a_swift_call_binds_by_label_and_passes_the_literal_under_the_new_one() {
        let (_, params) = parse_params("_ label: String, x: Int, y: Int = 0", Language::Swift);
        let rewrite = |args: &[&str], bundled: &[usize]| {
            let args = strings(args);
            bind_arguments(&args, &params, Language::Swift).map(|bound| {
                rewritten_call(
                    &args,
                    &bound,
                    bundled,
                    &params,
                    Language::Swift,
                    "Point",
                    "point",
                )
            })
        };
        assert_eq!(
            rewrite(&["\"b\"", "x: 5", "y: 6"], &[1, 2]).as_deref(),
            Some("\"b\", point: Point(x: 5, y: 6)")
        );
        assert_eq!(
            rewrite(&["\"c\"", "x: 9"], &[1, 2]).as_deref(),
            Some("\"c\", point: Point(x: 9)"),
            "a default the call relied on is the field's default"
        );
        assert_eq!(
            rewrite(&["\"b\"", "x: 5", "y: 6"], &[0, 1]).as_deref(),
            Some("Point(label: \"b\", x: 5), y: 6"),
            "an unlabelled first parameter makes an unlabelled new one"
        );
        assert_eq!(rewrite(&["x: 5"], &[1, 2]), None, "`label` has no default");
        assert_eq!(
            rewrite(&["\"b\"", "y: 6", "x: 5"], &[1, 2]),
            None,
            "labels come in declaration order"
        );
    }

    #[test]
    fn a_cpp_call_may_leave_off_defaulted_trailing_arguments() {
        let (_, mut params) =
            parse_params("const char *name, int width, int height", Language::Cpp);
        params[2].default = Some("2".to_string());
        let args = strings(&["\"m\"", "7"]);
        let bound = bind_arguments(&args, &params, Language::Cpp).expect("height has a default");
        assert_eq!(
            rewritten_call(
                &args,
                &bound,
                &[1, 2],
                &params,
                Language::Cpp,
                "Size",
                "size"
            ),
            "\"m\", {.width = 7}"
        );
        assert_eq!(
            rewritten_call_with(
                &args,
                &bound,
                &[1, 2],
                &params,
                Language::Cpp,
                "size",
                aggregate_text
            ),
            "\"m\", {7}"
        );
        assert!(bind_arguments(&strings(&["\"m\""]), &params, Language::Cpp).is_none());
        let (_, params) = parse_params("const char *name, int width, int height", Language::C);
        let args = strings(&["\"a\"", "3", "4"]);
        let bound = bind_arguments(&args, &params, Language::C).expect("same arity");
        assert_eq!(
            rewritten_call(&args, &bound, &[1, 2], &params, Language::C, "Size", "size"),
            "\"a\", (struct Size){.width = 3, .height = 4}"
        );
        let (_, printf) = parse_params("const char *fmt, ...", Language::C);
        assert_eq!(
            bind_arguments(&strings(&["f", "1", "2"]), &printf, Language::C),
            Some(vec![Some(0), Some(1), Some(1)])
        );
    }

    #[test]
    fn c_cpp_and_swift_declare_a_struct_their_own_way() {
        let fields = [
            field("width", Some("int width"), None),
            field("height", Some("int height"), Some("2")),
        ];
        assert_eq!(
            type_text(Language::Cpp, "Size", "build", &fields, "    ", false),
            "// The parameters `build` takes together.\nstruct Size {\n    int width;\n    int height = 2;\n};\n"
        );
        assert_eq!(
            type_text(
                Language::C,
                "Size",
                "build",
                &[field("name", Some("const char *name"), None)],
                "\t",
                false
            ),
            "/* The parameters `build` takes together. */\nstruct Size {\n\tconst char *name;\n};\n"
        );
        assert_eq!(
            type_text(
                Language::Swift,
                "Point",
                "draw",
                &[
                    field("x", Some("Int"), None),
                    field("y", Some("Int"), Some("0"))
                ],
                "    ",
                true
            ),
            "/// The parameters `draw` takes together.\npublic struct Point {\n    let x: Int\n    var y: Int = 0\n}\n"
        );
    }

    #[test]
    fn the_cpp_standard_is_what_the_build_declares() {
        assert_eq!(
            std_in_cmake("set(CMAKE_CXX_STANDARD_REQUIRED ON)\nset(CMAKE_CXX_STANDARD 20)\n"),
            Some(2020)
        );
        assert_eq!(
            std_in_cmake("target_compile_features(po PRIVATE cxx_std_17)\n"),
            Some(2017)
        );
        assert_eq!(
            std_in_cmake("add_compile_options(-std=gnu++2a)\n"),
            Some(2020)
        );
        assert_eq!(std_in_cmake("project(po C)\n"), None);
        assert_eq!(
            std_in_flags("[{\"command\": \"c++ -std=c++1z -o a.o -c a.cpp\"}]"),
            Some(2017)
        );
        assert_eq!(std_in_flags("/std:c++latest"), None);
        assert_eq!(std_year("98"), Some(1998));
        assert_eq!(std_year("23 "), Some(2023));
    }

    /// clangd's answer for a header with a class in a namespace, cut to what is looked at.
    #[test]
    fn the_new_type_goes_above_the_outermost_declaration_that_is_not_a_namespace() {
        let symbols = serde_json::json!([{
            "kind": 3, "name": "shapes",
            "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 17, "character": 1 } },
            "children": [
                { "kind": 12, "name": "build",
                  "range": { "start": { "line": 7, "character": 0 }, "end": { "line": 7, "character": 69 } } },
                { "kind": 5, "name": "Canvas",
                  "range": { "start": { "line": 9, "character": 0 }, "end": { "line": 15, "character": 1 } },
                  "children": [ { "kind": 6, "name": "draw",
                    "range": { "start": { "line": 11, "character": 4 }, "end": { "line": 11, "character": 66 } } } ] }
            ]
        }]);
        let symbols = symbols.as_array().unwrap();
        assert_eq!(outermost_container(symbols, 11), Some(9));
        assert_eq!(outermost_container(symbols, 7), Some(7));
        assert_eq!(outermost_container(symbols, 2), None);
    }
}
