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
}

impl ParameterObject {
    /// The report: the new type, what the declaration became, and whether it compiles.
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: ({})\n- now: ({})\n- {} call site(s) rewritten, {} use(s) in \
             the body\n\n```rust\n{}\n```\n\n",
            self.symbol,
            self.file,
            self.was,
            self.now,
            self.call_sites,
            self.body_uses,
            self.struct_text
        );
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
        if !self.imports.is_empty() {
            out.push_str("\nimports:\n");
            for note in &self.imports {
                out.push_str(&format!("  {note}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) that are not a call with the arity this \
                 declaration has — a function pointer, a macro, or a call already changed):\n",
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
    let start = i + 1;
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
                    return Some((start, i));
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
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

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
}
