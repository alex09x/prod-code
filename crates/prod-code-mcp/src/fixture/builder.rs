//! Typed builders (roadmap 8.5, #459): the source of a builder for a struct with named fields,
//! read from the declaration in the file and checked by the analyzer where it would go.
//!
//! A builder is only worth having when it is exactly the struct: one setter per field taking the
//! field's type as written, every field required, and `build` naming the first one that was never
//! set. So nothing here guesses. The declaration is located with the analyzer's document symbols
//! and read from the file (hover elides fields past the tenth), the fields are read from its
//! tokens, and every shape the generator cannot be sure of — tuple and unit structs, enums,
//! unions, `cfg`-dependent fields, attribute macros that may rewrite the struct, unsupported const
//! expressions, and `Self` where it would change meaning — is refused with the reason. A name the
//! builder would introduce that is already taken in the file, in the workspace index or (when
//! verifying) in the scope the builder would be inserted into is refused as well.
//!
//! This is a preview: nothing is written. Verification places the builder right after the
//! declaration in an in-memory overlay, next to a deliberate error. The analyzer's silence about
//! the builder counts only when it reported the deliberate error: a scope it does not check (an
//! inactive `cfg`, a file outside the crate, an engine still loading) is reported as unverified,
//! never as clean.

use anyhow::{Context, Result, bail};
use std::net::SocketAddr;
use std::path::Path;

/// The method the verification canary calls, which no builder has.
const CANARY_METHOD: &str = "__prod_code_missing_method";
/// Methods of the builder itself; a field of the same name would need a second one.
const RESERVED_METHODS: [&str; 2] = ["new", "build"];
/// Attributes that never change a struct's fields.
const INERT_ATTRIBUTES: [&str; 12] = [
    "derive",
    "doc",
    "repr",
    "allow",
    "warn",
    "deny",
    "forbid",
    "expect",
    "must_use",
    "non_exhaustive",
    "deprecated",
    "automatically_derived",
];
/// Strict and reserved keywords, which a builder name cannot be.
const KEYWORDS: [&str; 52] = [
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

/// What to generate a builder for.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuilderRequest<'a> {
    /// The struct's name, as the workspace symbol index knows it.
    pub symbol: &'a str,
    /// A file inside the project, which also picks one of several declarations of that name.
    pub hint: Option<&'a Path>,
    /// The builder's name; `<Type>Builder` when absent. The error type is `<builder>Error`.
    pub builder_name: Option<&'a str>,
    /// Check the builder with the analyzer in the scope it would be inserted into.
    pub verify: bool,
}

/// One field of the struct, and the setter the builder has for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuilderField {
    /// The field's name as declared, raw identifiers included (`r#type`).
    pub name: String,
    /// The field's type as spelled in the declaration, whitespace collapsed.
    pub ty: String,
    /// The setter's name: the field's own.
    pub setter: String,
}

/// The builder for one declaration, computed from the file's text alone.
#[derive(Debug, Clone)]
pub struct BuilderPlan {
    /// The struct's name as declared.
    pub type_name: String,
    pub builder_name: String,
    /// The error `build` returns for a field that was never set.
    pub error_name: String,
    /// The struct's visibility as declared, given to the builder, its error and their methods.
    pub visibility: String,
    pub fields: Vec<BuilderField>,
    /// The complete generated source, indented like the declaration.
    pub code: String,
    /// The whole file as it would be with the builder inserted.
    pub file_text: String,
    /// The declaration's first line (its first attribute) and last line, 1-based.
    pub declaration_lines: (u32, u32),
    /// The line the builder would be inserted after, 1-based: the declaration's last.
    pub insert_after_line: u32,
    /// Where `code` sits in `file_text`, 1-based and inclusive.
    pub code_lines: (u32, u32),
    /// Things worth knowing that did not stop the generation.
    pub notes: Vec<String>,
    indent: String,
    newline: &'static str,
}

/// What the analyzer made of the generated builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// The analyzer checked the builder where it would be inserted and reported no error; it
    /// also reported the deliberate error placed next to it, so it was really looking.
    Clean,
    /// The analyzer checked the builder and reported these errors.
    Rejected { diagnostics: Vec<String> },
    /// The builder was not checked, for this reason. Not a pass.
    Unverified { reason: String },
}

/// A generated builder, where it would go, and whether the analyzer accepted it there.
#[derive(Debug, Clone)]
pub struct BuilderPreview {
    /// The file that declares the struct, relative to the workspace root.
    pub file: String,
    pub plan: BuilderPlan,
    pub verification: Verification,
}

impl BuilderPreview {
    /// True only when the analyzer checked the builder in place and found nothing.
    pub fn verified(&self) -> bool {
        self.verification == Verification::Clean
    }

    /// The analyzer's errors, empty unless the builder was rejected.
    pub fn diagnostics(&self) -> &[String] {
        match &self.verification {
            Verification::Rejected { diagnostics } => diagnostics,
            _ => &[],
        }
    }

    pub fn render(&self) -> String {
        let plan = &self.plan;
        let mut out = format!(
            "builder `{}` for `{}` ({} field{}), declared in {}:{}-{}; it would be inserted after line {}\n\n```rust\n{}\n```\n",
            plan.builder_name,
            plan.type_name,
            plan.fields.len(),
            if plan.fields.len() == 1 { "" } else { "s" },
            self.file,
            plan.declaration_lines.0,
            plan.declaration_lines.1,
            plan.insert_after_line,
            plan.code,
        );
        match &self.verification {
            Verification::Clean => out.push_str(
                "\nverified: the analyzer checked it in the scope of the declaration: 0 errors\n",
            ),
            Verification::Rejected { diagnostics } => {
                out.push_str("\nrejected: the analyzer reports errors in it:\n");
                for d in diagnostics {
                    out.push_str(&format!("  {d}\n"));
                }
            }
            Verification::Unverified { reason } => {
                out.push_str(&format!("\nnot verified: {reason}\n"));
            }
        }
        for note in &plan.notes {
            out.push_str(&format!("note: {note}\n"));
        }
        out.push_str("nothing was written\n");
        out
    }
}

/// Generates the builder for `request.symbol` and, when asked, checks it with the analyzer.
/// Nothing is written: the result carries the code and the file as it would be.
pub async fn preview(
    remote: SocketAddr,
    root: &Path,
    request: &BuilderRequest<'_>,
) -> Result<BuilderPreview> {
    let hit = super::resolve_type(remote, root, request.symbol, request.hint).await?;
    let file = hit
        .path
        .strip_prefix(root)
        .unwrap_or(&hit.path)
        .to_string_lossy()
        .into_owned();
    let uri = url::Url::from_file_path(&hit.path)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", hit.path))?
        .to_string();
    let outline = crate::tools::execute_lsp_query(
        remote,
        root,
        &hit.path,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await?;
    let node = outline_node(&outline, request.symbol, hit.line)
        .with_context(|| {
            format!(
                "the analyzer's outline of {file} is malformed; refusing rather than guessing where `{}` is and which fields it has",
                request.symbol
            )
        })?
        .with_context(|| {
            format!(
                "the analyzer's outline of {file} has no declaration of `{}` at line {}",
                request.symbol, hit.line
            )
        })?;
    let text = std::fs::read_to_string(&hit.path)
        .with_context(|| format!("cannot read {}", hit.path.display()))?;
    let mut plan = plan(
        &text,
        request.symbol,
        (node.start, node.end),
        request.builder_name,
    )
    .with_context(|| format!("no builder for `{}` in {file}", request.symbol))?;
    match &node.fields {
        Some(listed) => {
            let read: Vec<&str> = plan.fields.iter().map(|f| bare(&f.name)).collect();
            let listed: Vec<&str> = listed.iter().map(|f| bare(f)).collect();
            if read != listed {
                bail!(
                    "the analyzer's outline of `{}` lists the fields [{}] but the declaration in {file} spells [{}]; refusing rather than generating a builder for part of the struct",
                    request.symbol,
                    listed.join(", "),
                    read.join(", ")
                );
            }
        }
        None if !plan.fields.is_empty() => plan.notes.push(
            "the analyzer's outline lists no fields for the struct; they were read from its declaration"
                .to_string(),
        ),
        None => {}
    }
    for name in [plan.builder_name.clone(), plan.error_name.clone()] {
        let hits =
            crate::tools::workspace_symbol_search(remote, root, &name, Some(&hit.path), 64).await?;
        if let Some(taken) = hits.iter().find(|h| bare(&h.name) == name) {
            bail!(
                "`{name}` is already declared in this workspace ({} at {}:{}); the builder would collide with it or change what an import of it names. Pass another `builder_name`",
                taken.kind,
                taken
                    .path
                    .strip_prefix(root)
                    .unwrap_or(&taken.path)
                    .to_string_lossy(),
                taken.line
            );
        }
    }
    let verification = if request.verify {
        verify(remote, root, &hit.path, &file, &text, &plan).await?
    } else {
        Verification::Unverified {
            reason: "verification was not requested; the names were checked against the declaring file and the workspace index only".to_string(),
        }
    };
    Ok(BuilderPreview {
        file,
        plan,
        verification,
    })
}

/// A declaration in the analyzer's document symbols: its lines, 1-based, and the fields it
/// lists, when it lists any.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OutlineNode {
    start: u32,
    end: u32,
    fields: Option<Vec<String>>,
}

/// The smallest struct-like node called `name` whose lines hold `line` (1-based); `None` when
/// the outline has none. An outline that cannot be read where it speaks of `name` (a range that
/// is not one, a field without a name, members that are not a list) is an error: dropping that
/// part would pass a partial or misplaced struct for the declaration.
fn outline_node(symbols: &serde_json::Value, name: &str, line: u32) -> Result<Option<OutlineNode>> {
    fn walk(
        nodes: &[serde_json::Value],
        name: &str,
        line: u32,
        best: &mut Option<OutlineNode>,
    ) -> Result<()> {
        for node in nodes {
            let named = node
                .get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| bare(n) == bare(name));
            // Struct, enum and class: a union is listed as a struct.
            let kind = node.get("kind").and_then(|k| k.as_u64());
            if named && matches!(kind, Some(5) | Some(10) | Some(23)) {
                let (s, e) = symbol_lines(node).with_context(|| {
                    format!("it gives `{name}` a range that is not one: {}", shown(node))
                })?;
                if s <= line && line <= e && best.as_ref().is_none_or(|b| e - s < b.end - b.start) {
                    let mut fields = Vec::new();
                    for child in children(node)? {
                        match child.get("kind").and_then(|k| k.as_u64()) {
                            Some(8) => match child.get("name").and_then(|n| n.as_str()) {
                                Some(field) => fields.push(field.to_string()),
                                None => {
                                    bail!("it lists a field of `{name}` without a name: {child}")
                                }
                            },
                            Some(_) => {}
                            None => bail!("it lists a member of `{name}` without a kind: {child}"),
                        }
                    }
                    *best = Some(OutlineNode {
                        start: s,
                        end: e,
                        fields: (!fields.is_empty()).then_some(fields),
                    });
                }
            }
            walk(children(node)?, name, line, best)?;
        }
        Ok(())
    }
    /// Fields listed flat, as the Rust engine answers: a `Field` whose container path ends in
    /// the struct's name and whose line is inside the struct's.
    fn flat_fields(
        nodes: &[serde_json::Value],
        owner: &str,
        lines: (u32, u32),
        out: &mut Vec<String>,
    ) -> Result<()> {
        for node in nodes {
            let contained = node
                .get("containerName")
                .and_then(|c| c.as_str())
                .and_then(|c| c.rsplit(" > ").next())
                .is_some_and(|c| bare(c.trim()) == bare(owner));
            if node.get("kind").and_then(|k| k.as_u64()) == Some(8) && contained {
                let field = node.get("name").and_then(|n| n.as_str()).with_context(|| {
                    format!("it lists a field of `{owner}` without a name: {node}")
                })?;
                let (start, _) = symbol_lines(node).with_context(|| {
                    format!(
                        "it gives field `{field}` of `{owner}` a range that is not one: {}",
                        shown(node)
                    )
                })?;
                if lines.0 <= start && start <= lines.1 {
                    out.push(field.to_string());
                }
            }
            flat_fields(children(node)?, owner, lines, out)?;
        }
        Ok(())
    }
    /// A node's `DocumentSymbol` or `SymbolInformation` range, as 1-based lines.
    fn symbol_lines(node: &serde_json::Value) -> Option<(u32, u32)> {
        node.get("range")
            .or_else(|| node.pointer("/location/range"))
            .and_then(range_lines)
    }
    /// The range a node was given, for an error.
    fn shown(node: &serde_json::Value) -> String {
        node.get("range")
            .or_else(|| node.pointer("/location/range"))
            .map_or_else(|| "none".to_string(), |r| r.to_string())
    }
    /// A node's members: none when it has no `children` (or `null`), an error when they are
    /// not a list.
    fn children(node: &serde_json::Value) -> Result<&[serde_json::Value]> {
        match node.get("children") {
            None | Some(serde_json::Value::Null) => Ok(&[][..]),
            Some(serde_json::Value::Array(children)) => Ok(children.as_slice()),
            Some(other) => bail!("it lists members that are not a list: {other}"),
        }
    }
    let nodes = match symbols {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Array(nodes) => nodes,
        other => bail!("it is not a list of symbols: {other}"),
    };
    let mut best: Option<OutlineNode> = None;
    walk(nodes, name, line, &mut best)?;
    let Some(mut best) = best else {
        return Ok(None);
    };
    if best.fields.is_none() {
        let mut fields = Vec::new();
        flat_fields(nodes, name, (best.start, best.end), &mut fields)?;
        best.fields = (!fields.is_empty()).then_some(fields);
    }
    Ok(Some(best))
}

/// A 0-based LSP line or character as a 1-based `u32`. `None` for anything else, including a
/// number a cast would truncate or `+ 1` would overflow.
fn one_based(value: Option<&serde_json::Value>) -> Option<u32> {
    u32::try_from(value?.as_u64()?).ok()?.checked_add(1)
}

/// The first and last lines of an LSP range, 1-based. `None` unless both ends have a line and a
/// character that fit and the range does not end on a line before it starts. Only the lines are
/// ordered: the Rust engine's flat outline ends a field at character 0 of its own line.
fn range_lines(range: &serde_json::Value) -> Option<(u32, u32)> {
    let line = |at: &str| {
        one_based(range.pointer(&format!("/{at}/character")))?;
        one_based(range.pointer(&format!("/{at}/line")))
    };
    let (start, end) = (line("start")?, line("end")?);
    (start <= end).then_some((start, end))
}

/// Plans the builder for the declaration of `type_name` that starts within `within` (1-based
/// lines, as the analyzer's outline gives them) in `file_text`. Pure: no analyzer, no disk.
pub fn plan(
    file_text: &str,
    type_name: &str,
    within: (u32, u32),
    builder_name: Option<&str>,
) -> Result<BuilderPlan> {
    let src = Source::new(file_text)?;
    let wanted = bare(type_name);
    let candidates: Vec<usize> = (1..src.tokens.len())
        .filter(|&i| {
            src.tokens[i].kind == Kind::Ident
                && bare(src.t(i)) == wanted
                && src.tokens[i - 1].kind == Kind::Ident
                && matches!(src.t(i - 1), "struct" | "enum" | "union")
                && (within.0..=within.1).contains(&src.line(i))
        })
        .collect();
    let name_at = match candidates.as_slice() {
        [one] => *one,
        [] => bail!("no `struct {type_name}` in lines {}-{}", within.0, within.1),
        _ => bail!(
            "more than one declaration of `{type_name}` in lines {}-{}",
            within.0,
            within.1
        ),
    };
    let decl = read_declaration(&src, name_at)?;
    let builder = match builder_name {
        Some(name) => {
            if !is_plain_identifier(name) {
                bail!("`{name}` is not an identifier a builder can be named");
            }
            name.to_string()
        }
        None => format!("{}Builder", bare(&decl.name)),
    };
    let error = format!("{builder}Error");
    if builder == bare(&decl.name) {
        bail!("the builder cannot have the struct's own name `{builder}`");
    }
    for name in [&builder, &error] {
        if let Some(i) = (0..src.tokens.len())
            .find(|&i| src.tokens[i].kind == Kind::Ident && bare(src.t(i)) == name.as_str())
        {
            bail!(
                "`{name}` already appears in this file at line {}; the builder would collide with it or change what it names. Pass another `builder_name`",
                src.line(i)
            );
        }
    }
    for field in &decl.fields {
        if RESERVED_METHODS.contains(&bare(&field.name)) {
            bail!(
                "field `{}` would need a setter called `{}`, which the builder already has for itself; rename the field before generating a builder",
                field.name,
                bare(&field.name)
            );
        }
    }
    let newline = if file_text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let code = render_code(&decl, &builder, &error, newline);
    let (file_text_after, first) = insert_after(file_text, decl.last_line, &code, newline);
    let code_len = code.split('\n').count() as u32;
    Ok(BuilderPlan {
        type_name: decl.name.clone(),
        builder_name: builder,
        error_name: error,
        visibility: decl.visibility.clone(),
        fields: decl.fields.clone(),
        code,
        file_text: file_text_after,
        declaration_lines: (decl.first_line, decl.last_line),
        insert_after_line: decl.last_line,
        code_lines: (first, first + code_len - 1),
        notes: decl.notes.clone(),
        indent: decl.indent.clone(),
        newline,
    })
}

/// A struct declaration as read from its tokens.
#[derive(Debug, Clone)]
struct Declaration {
    name: String,
    visibility: String,
    generics: Generics,
    where_clause: String,
    fields: Vec<BuilderField>,
    first_line: u32,
    last_line: u32,
    indent: String,
    notes: Vec<String>,
}

/// Generic syntax copied to the builder declaration and adapted for its impl and type uses.
#[derive(Debug, Clone, Default)]
struct Generics {
    /// The declaration's `<...>`, including legal type and const defaults.
    declaration: String,
    /// The impl's `<...>`, with type and const defaults removed as Rust requires.
    impl_declaration: String,
    /// The original type's arguments, containing parameter names only.
    arguments: String,
}

/// Reads the declaration whose name is token `name_at`, refusing every shape it cannot be sure of.
fn read_declaration(src: &Source<'_>, name_at: usize) -> Result<Declaration> {
    let name = src.t(name_at).to_string();
    match src.t(name_at - 1) {
        "struct" => {}
        "enum" => {
            bail!("`{name}` is an enum; a builder is generated only for a struct with named fields")
        }
        _ => {
            bail!("`{name}` is a union; a builder is generated only for a struct with named fields")
        }
    }
    // Backwards over the visibility and the outer attributes.
    let mut begin = name_at - 1;
    if begin > 0 && src.is(begin - 1, ")") {
        let open = src
            .open_of(begin - 1)
            .context("unbalanced parentheses before the declaration")?;
        if open == 0 || !src.is(open - 1, "pub") {
            bail!("cannot read what precedes `struct {name}`");
        }
        begin = open - 1;
    } else if begin > 0 && src.is(begin - 1, "pub") {
        begin -= 1;
    }
    let visibility = src.spell(begin, name_at - 1);
    let mut attributes = Vec::new();
    while begin >= 2 && src.is(begin - 1, "]") {
        let Some(open) = src.open_of(begin - 1) else {
            bail!("unbalanced brackets before `struct {name}`");
        };
        // `#![...]` belongs to the enclosing module.
        if open == 0 || !src.is(open - 1, "#") {
            break;
        }
        attributes.push((src.attribute_path(open + 1, begin - 1), src.line(open)));
        begin = open - 1;
    }
    if begin > 0 && !matches!(src.t(begin - 1), ";" | "{" | "}" | "]") {
        bail!(
            "cannot tell where the declaration of `{name}` starts: `{}` precedes it on line {}",
            src.t(begin - 1),
            src.line(begin - 1)
        );
    }
    let has_derive = attributes.iter().any(|(p, _)| p == "derive");
    let mut notes = Vec::new();
    for (path, line) in &attributes {
        if path == "cfg" || path == "cfg_attr" {
            bail!(
                "`{name}` carries `#[{path}(…)]` (line {line}): whether and how it is declared depends on the build configuration, which a generated builder cannot follow. Generate it for a declaration without `{path}`"
            );
        }
        let tool = ["rustfmt::", "clippy::", "diagnostic::"]
            .iter()
            .any(|t| path.starts_with(t));
        if INERT_ATTRIBUTES.contains(&path.as_str()) || tool {
            continue;
        }
        if !has_derive {
            bail!(
                "`#[{path}]` on `{name}` (line {line}) is not a built-in attribute and no derive declares it as a helper: it may be an attribute macro that rewrites the struct, and a builder read from the source could miss fields"
            );
        }
        notes.push(format!(
            "`#[{path}]` is taken for a derive helper; if it is an attribute macro that changes the fields, only verification can show it"
        ));
    }
    let mut next = name_at + 1;
    let generics = if src.is(next, "<") {
        let (generics, close) = read_generics(src, next, &name)?;
        next = close + 1;
        generics
    } else {
        Generics::default()
    };
    let (where_clause, open) = if src.is(next, "where") {
        let open = struct_body_after_where(src, next, &name)?;
        validate_rebound_syntax(src, next, open, &name, "where clause")?;
        (src.spell(next, open), open)
    } else {
        (String::new(), next)
    };
    if !src.is(open, "{") {
        match src.tokens.get(open).map(|_| src.t(open)) {
            Some("(") => bail!(
                "`{name}` is a tuple struct; a builder is generated only for a struct with named fields"
            ),
            Some(";") => bail!("`{name}` is a unit struct; there is nothing for a builder to set"),
            _ => bail!("cannot read the declaration of `{name}`"),
        }
    }
    let close = src
        .close_of(open)
        .with_context(|| format!("the body of `{name}` is not closed"))?;
    let mut fields: Vec<BuilderField> = Vec::new();
    let (mut from, mut depth, mut angle) = (open + 1, 0i32, 0i32);
    for i in open + 1..=close {
        let split = if i == close {
            true
        } else if src.tokens[i].kind == Kind::Punct {
            match src.t(i) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                "<" if depth == 0 => angle += 1,
                ">" if depth == 0 && angle > 0 => angle -= 1,
                _ => {}
            }
            src.is(i, ",") && depth == 0 && angle == 0
        } else {
            false
        };
        if split {
            if from < i {
                let field = read_field(src, from, i, &name)?;
                if fields.iter().any(|f| bare(&f.name) == bare(&field.name)) {
                    bail!("`{name}` declares `{}` twice", field.name);
                }
                fields.push(field);
            }
            from = i + 1;
        }
    }
    // The builder goes after the line the declaration ends on, so nothing else may be on it.
    let end = src.tokens[close].end;
    let line_end = src.text[end..]
        .find('\n')
        .map_or(src.text.len(), |p| end + p);
    let rest = src.text[end..line_end].trim();
    if !rest.is_empty() && !rest.starts_with("//") {
        bail!(
            "the declaration of `{name}` shares its last line ({}) with other code; put it on a line of its own first",
            src.line(close)
        );
    }
    let start = src.tokens[begin].start;
    let line_start = src.text[..start].rfind('\n').map_or(0, |p| p + 1);
    let indent: String = src.text[line_start..start]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    Ok(Declaration {
        name,
        visibility,
        generics,
        where_clause,
        fields,
        first_line: src.line(begin),
        last_line: src.line(close),
        indent,
        notes,
    })
}

/// Reads `<...>` and derives each spelling Rust needs without changing bounds or defaults.
fn read_generics(src: &Source<'_>, open: usize, owner: &str) -> Result<(Generics, usize)> {
    let close = close_angle(src, open)
        .with_context(|| format!("the generic parameters of `{owner}` are not closed"))?;
    validate_rebound_syntax(src, open + 1, close, owner, "generic parameters")?;
    let mut parameters = Vec::new();
    let mut from = open + 1;
    let mut brackets = 0i32;
    let mut angles = 0i32;
    for i in open + 1..=close {
        let split = if i == close {
            true
        } else {
            match src.t(i) {
                "(" | "[" | "{" if src.tokens[i].kind == Kind::Punct => brackets += 1,
                ")" | "]" | "}" if src.tokens[i].kind == Kind::Punct => brackets -= 1,
                "<" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles += 1,
                ">" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles -= 1,
                _ => {}
            }
            src.is(i, ",") && brackets == 0 && angles == 0
        };
        if split {
            if from == i {
                if i != close {
                    bail!(
                        "`{owner}` has an empty generic parameter on line {}",
                        src.line(i)
                    );
                }
            } else {
                parameters.push(read_generic_parameter(src, from, i, owner)?);
            }
            from = i + 1;
        }
    }
    if parameters.is_empty() {
        bail!("`{owner}` has an empty generic parameter list");
    }
    Ok((
        Generics {
            declaration: src.spell(open, close + 1),
            impl_declaration: format!(
                "<{}>",
                parameters
                    .iter()
                    .map(|p| p.impl_parameter.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            arguments: format!(
                "<{}>",
                parameters
                    .iter()
                    .map(|p| p.argument.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
        close,
    ))
}

struct GenericParameter {
    impl_parameter: String,
    argument: String,
}

fn read_generic_parameter(
    src: &Source<'_>,
    from: usize,
    to: usize,
    owner: &str,
) -> Result<GenericParameter> {
    if src.is(from, "#") {
        bail!(
            "`{owner}` has an attributed generic parameter on line {}; attributes on generic parameters are not supported",
            src.line(from)
        );
    }
    let top_level = |needle: &str| {
        let mut brackets = 0i32;
        let mut angles = 0i32;
        for i in from..to {
            if src.is(i, needle) && brackets == 0 && angles == 0 {
                return Some(i);
            }
            match src.t(i) {
                "(" | "[" | "{" if src.tokens[i].kind == Kind::Punct => brackets += 1,
                ")" | "]" | "}" if src.tokens[i].kind == Kind::Punct => brackets -= 1,
                "<" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles += 1,
                ">" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles -= 1,
                _ => {}
            }
        }
        None
    };
    let equals = top_level("=");
    let parameter_end = equals.unwrap_or(to);
    let argument = if src.tokens[from].kind == Kind::Lifetime {
        if equals.is_some() {
            bail!(
                "lifetime parameter `{}` of `{owner}` cannot have a default",
                src.t(from)
            );
        }
        if from + 1 < parameter_end && !src.is(from + 1, ":") {
            bail!(
                "cannot read lifetime parameter `{}` of `{owner}` on line {}",
                src.t(from),
                src.line(from)
            );
        }
        if src.is(from + 1, ":") && from + 2 >= parameter_end {
            bail!(
                "lifetime parameter `{}` of `{owner}` has no bound",
                src.t(from)
            );
        }
        src.t(from).to_string()
    } else if src.is(from, "const") {
        if from + 1 >= to || src.tokens[from + 1].kind != Kind::Ident {
            bail!(
                "cannot read a const parameter of `{owner}` on line {}",
                src.line(from)
            );
        }
        let colon = top_level(":").filter(|&i| i == from + 2).with_context(|| {
            format!(
                "cannot read const parameter `{}` of `{owner}`; expected `const NAME: TYPE`",
                src.t(from + 1)
            )
        })?;
        if colon + 1 >= parameter_end {
            bail!(
                "const parameter `{}` of `{owner}` has no type",
                src.t(from + 1)
            );
        }
        if let Some(eq) = equals {
            validate_const_default(src, eq + 1, to, owner, src.t(from + 1))?;
        }
        src.t(from + 1).to_string()
    } else if src.tokens[from].kind == Kind::Ident {
        if from + 1 < parameter_end && !src.is(from + 1, ":") {
            bail!(
                "cannot read type parameter `{}` of `{owner}` on line {}; expected a bound or default",
                src.t(from),
                src.line(from)
            );
        }
        if src.is(from + 1, ":") && from + 2 >= parameter_end {
            bail!("type parameter `{}` of `{owner}` has no bound", src.t(from));
        }
        src.t(from).to_string()
    } else {
        bail!(
            "cannot read a generic parameter of `{owner}` on line {}",
            src.line(from)
        );
    };
    Ok(GenericParameter {
        impl_parameter: src.spell(from, parameter_end),
        argument,
    })
}

fn validate_const_default(
    src: &Source<'_>,
    from: usize,
    to: usize,
    owner: &str,
    parameter: &str,
) -> Result<()> {
    let simple = is_literal_or_const_path(src, from, to);
    if !simple {
        bail!(
            "const parameter `{parameter}` of `{owner}` has an unsupported default expression; use a literal or const path"
        );
    }
    Ok(())
}

fn validate_rebound_syntax(
    src: &Source<'_>,
    from: usize,
    to: usize,
    owner: &str,
    place: &str,
) -> Result<()> {
    for i in from..to {
        if src.tokens[i].kind == Kind::Ident && src.t(i) == "Self" {
            bail!(
                "the {place} of `{owner}` spells `Self` (line {}), which would name the builder inside its impl; spell `{owner}` explicitly",
                src.line(i)
            );
        }
        if src.is(i, "!") && i > from && src.tokens[i - 1].kind == Kind::Ident {
            bail!(
                "the {place} of `{owner}` invokes a macro on line {}; macro-expanded generic syntax is not supported",
                src.line(i)
            );
        }
        if src.is(i, "{") {
            bail!(
                "the {place} of `{owner}` has an unsupported const expression on line {}; const blocks are not supported",
                src.line(i)
            );
        }
    }
    Ok(())
}

fn close_angle(src: &Source<'_>, open: usize) -> Option<usize> {
    let mut angles = 0i32;
    let mut brackets = 0i32;
    for i in open..src.tokens.len() {
        match src.t(i) {
            "(" | "[" | "{" if src.tokens[i].kind == Kind::Punct => brackets += 1,
            ")" | "]" | "}" if src.tokens[i].kind == Kind::Punct => brackets -= 1,
            "<" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles += 1,
            ">" if src.tokens[i].kind == Kind::Punct && brackets == 0 => {
                angles -= 1;
                if angles == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn struct_body_after_where(src: &Source<'_>, from: usize, owner: &str) -> Result<usize> {
    let mut angles = 0i32;
    let mut brackets = 0i32;
    for i in from + 1..src.tokens.len() {
        match src.t(i) {
            "{" if src.tokens[i].kind == Kind::Punct && angles == 0 && brackets == 0 => {
                return Ok(i);
            }
            "(" | "[" | "{" if src.tokens[i].kind == Kind::Punct => brackets += 1,
            ")" | "]" | "}" if src.tokens[i].kind == Kind::Punct => brackets -= 1,
            "<" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles += 1,
            ">" if src.tokens[i].kind == Kind::Punct && brackets == 0 => angles -= 1,
            ";" if angles == 0 && brackets == 0 => break,
            _ => {}
        }
    }
    bail!("cannot find the body after the `where` clause of `{owner}`")
}

/// One field: tokens `from..to` of the body, between commas.
fn read_field(src: &Source<'_>, from: usize, to: usize, owner: &str) -> Result<BuilderField> {
    let mut i = from;
    let mut attributes = Vec::new();
    while i < to && src.is(i, "#") {
        if !src.is(i + 1, "[") {
            bail!(
                "cannot read the attribute on line {} of `{owner}`",
                src.line(i)
            );
        }
        let close = src
            .close_of(i + 1)
            .filter(|&c| c < to)
            .with_context(|| format!("unbalanced attribute on line {}", src.line(i)))?;
        attributes.push(src.attribute_path(i + 2, close));
        i = close + 1;
    }
    if i < to && src.is(i, "pub") {
        i += 1;
        if i < to && src.is(i, "(") {
            i = src
                .close_of(i)
                .filter(|&c| c < to)
                .with_context(|| format!("unbalanced visibility on line {}", src.line(i)))?
                + 1;
        }
    }
    if i < to && src.is(i, "unsafe") {
        bail!(
            "`{owner}` has an `unsafe` field (line {}); a safe setter cannot be generated for it",
            src.line(i)
        );
    }
    if i >= to || src.tokens[i].kind != Kind::Ident || i + 1 >= to || !src.is(i + 1, ":") {
        bail!(
            "cannot read a field of `{owner}` on line {}: expected `name: Type`",
            src.line(i.min(to - 1))
        );
    }
    let name = src.t(i).to_string();
    for path in &attributes {
        if path == "cfg" || path == "cfg_attr" {
            bail!(
                "field `{name}` of `{owner}` carries `#[{path}(…)]` (line {}): whether it exists depends on the build configuration, which a generated builder cannot follow",
                src.line(i)
            );
        }
    }
    let ty_from = i + 2;
    if ty_from >= to {
        bail!("field `{name}` of `{owner}` has no type");
    }
    let mut depth = 0i32;
    for j in ty_from..to {
        match src.t(j) {
            "(" | "[" | "{" if src.tokens[j].kind == Kind::Punct => depth += 1,
            ")" | "]" | "}" if src.tokens[j].kind == Kind::Punct => depth -= 1,
            "=" if src.tokens[j].kind == Kind::Punct && depth == 0 => bail!(
                "field `{name}` of `{owner}` has a default value (line {}); a builder that requires every field would not honour it",
                src.line(j)
            ),
            "Self" if src.tokens[j].kind == Kind::Ident => bail!(
                "the type of field `{name}` of `{owner}` spells `Self` (line {}), which names the builder inside it; spell the struct's name instead",
                src.line(j)
            ),
            _ => {}
        }
    }
    validate_field_type(src, ty_from, to, owner, &name)?;
    Ok(BuilderField {
        setter: name.clone(),
        ty: src.spell(ty_from, to),
        name,
    })
}

fn validate_field_type(
    src: &Source<'_>,
    from: usize,
    to: usize,
    owner: &str,
    field: &str,
) -> Result<()> {
    for i in from..to {
        if src.is(i, "!") && i > from && src.tokens[i - 1].kind == Kind::Ident {
            bail!(
                "the type of field `{field}` of `{owner}` invokes a macro on line {}; macro-expanded field types are not supported",
                src.line(i)
            );
        }
        if src.is(i, ";") {
            let mut end = i + 1;
            while end < to && !src.is(end, "]") {
                end += 1;
            }
            let simple = is_literal_or_const_path(src, i + 1, end);
            if !simple {
                bail!(
                    "the type of field `{field}` of `{owner}` has an unsupported const expression on line {}; array lengths must be a literal or const path",
                    src.line(i)
                );
            }
        }
    }
    Ok(())
}

fn is_literal_or_const_path(src: &Source<'_>, from: usize, to: usize) -> bool {
    if from + 1 == to && src.tokens[from].kind == Kind::Literal {
        return true;
    }
    let mut i = from;
    if src.is(i, "::") {
        i += 1;
    }
    if i >= to || src.tokens[i].kind != Kind::Ident {
        return false;
    }
    i += 1;
    while i < to {
        if !src.is(i, "::") || i + 1 >= to || src.tokens[i + 1].kind != Kind::Ident {
            return false;
        }
        i += 2;
    }
    true
}

/// The builder, its error type and their impls, one line per element of the result joined with
/// `newline`, every line indented like the declaration.
fn render_code(decl: &Declaration, builder: &str, error: &str, newline: &str) -> String {
    let ty = &decl.name;
    let type_use = format!("{ty}{}", decl.generics.arguments);
    let builder_declaration = format!("{builder}{}", decl.generics.declaration);
    let builder_use = format!("{builder}{}", decl.generics.arguments);
    let where_suffix = if decl.where_clause.is_empty() {
        String::new()
    } else {
        format!(" {}", decl.where_clause)
    };
    let shown = bare(ty);
    let vis = if decl.visibility.is_empty() {
        String::new()
    } else {
        format!("{} ", decl.visibility)
    };
    let mut lines: Vec<String> = Vec::new();
    let mut push = |s: String| lines.push(s);
    push(format!(
        "/// Builds a `{shown}` one field at a time. Every field is required: `{builder}::build`"
    ));
    push("/// names the first one that was never set.".to_string());
    push("#[must_use]".to_string());
    let snake = decl
        .fields
        .iter()
        .any(|f| bare(&f.name).chars().any(char::is_uppercase));
    if snake {
        push("#[allow(non_snake_case)]".to_string());
    }
    push(format!(
        "{vis}struct {builder_declaration}{where_suffix} {{"
    ));
    for f in &decl.fields {
        push(format!("    {}: ::core::option::Option<{}>,", f.name, f.ty));
    }
    push("}".to_string());
    push(String::new());
    let mut allowed = vec![
        "clippy::new_without_default",
        "clippy::should_implement_trait",
        "clippy::wrong_self_convention",
    ];
    if snake {
        allowed.push("non_snake_case");
    }
    push(format!("#[allow({})]", allowed.join(", ")));
    let impl_prefix = if decl.generics.impl_declaration.is_empty() {
        "impl".to_string()
    } else {
        format!("impl{}", decl.generics.impl_declaration)
    };
    push(format!("{impl_prefix} {builder_use}{where_suffix} {{"));
    push("    /// A builder with no field set.".to_string());
    push(format!("    {vis}fn new() -> Self {{"));
    if decl.fields.is_empty() {
        push("        Self {}".to_string());
    } else {
        push("        Self {".to_string());
        for f in &decl.fields {
            push(format!(
                "            {}: ::core::option::Option::None,",
                f.name
            ));
        }
        push("        }".to_string());
    }
    push("    }".to_string());
    for f in &decl.fields {
        push(String::new());
        push(format!("    /// Sets `{}`.", bare(&f.name)));
        push(format!(
            "    {vis}fn {}(mut self, value: {}) -> Self {{",
            f.setter, f.ty
        ));
        push(format!(
            "        self.{} = ::core::option::Option::Some(value);",
            f.name
        ));
        push("        self".to_string());
        push("    }".to_string());
    }
    push(String::new());
    push(format!(
        "    /// The `{shown}`, or the first field in declaration order that was never set."
    ));
    push(format!(
        "    {vis}fn build(self) -> ::core::result::Result<{type_use}, {error}> {{"
    ));
    if decl.fields.is_empty() {
        push(format!("        ::core::result::Result::Ok({ty} {{}})"));
    } else {
        push(format!("        ::core::result::Result::Ok({ty} {{"));
        for f in &decl.fields {
            push(format!(
                "            {}: self.{}.ok_or({error} {{ field: \"{}\" }})?,",
                f.name,
                f.name,
                bare(&f.name)
            ));
        }
        push("        })".to_string());
    }
    push("    }".to_string());
    push("}".to_string());
    push(String::new());
    push(format!(
        "/// The error `{builder}::build` returns for a field that was never set."
    ));
    push("#[derive(::core::fmt::Debug, ::core::clone::Clone, ::core::marker::Copy, ::core::cmp::PartialEq, ::core::cmp::Eq)]".to_string());
    push(format!("{vis}struct {error} {{"));
    push("    field: &'static str,".to_string());
    push("}".to_string());
    push(String::new());
    push(format!("impl {error} {{"));
    push("    /// The field that was never set, as declared.".to_string());
    push(format!("    {vis}fn field(&self) -> &'static str {{"));
    push("        self.field".to_string());
    push("    }".to_string());
    push("}".to_string());
    push(String::new());
    push(format!("impl ::core::fmt::Display for {error} {{"));
    push(
        "    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {"
            .to_string(),
    );
    push(format!("        f.write_str(\"`{shown}` field `\")?;"));
    push("        f.write_str(self.field)?;".to_string());
    push("        f.write_str(\"` was never set\")".to_string());
    push("    }".to_string());
    push("}".to_string());
    push(String::new());
    push(format!("impl ::core::error::Error for {error} {{}}"));
    lines
        .into_iter()
        .map(|l| {
            if l.is_empty() {
                l
            } else {
                format!("{}{l}", decl.indent)
            }
        })
        .collect::<Vec<_>>()
        .join(newline)
}

/// `text` with `snippet` inserted after line `line` (1-based) and a blank line before it, and
/// the line the snippet starts on.
fn insert_after(text: &str, line: u32, snippet: &str, newline: &str) -> (String, u32) {
    let mut at = text.len();
    let mut seen = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            seen += 1;
            if seen == line {
                at = i + 1;
                break;
            }
        }
    }
    let mut out = String::with_capacity(text.len() + snippet.len() + 8);
    out.push_str(&text[..at]);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(newline);
    }
    out.push_str(newline);
    let first = out.matches('\n').count() as u32 + 1;
    out.push_str(snippet);
    out.push_str(newline);
    // A blank line before the next item, none before the brace that closes a module.
    let rest = &text[at..];
    let next = rest.lines().next().unwrap_or("").trim();
    if !next.is_empty() && !next.starts_with('}') {
        out.push_str(newline);
    }
    out.push_str(rest);
    (out, first)
}

/// What the analyzer says the builder's names resolve to where it would be inserted.
enum Probe {
    Free,
    Taken(String, String),
    Blind(String),
}

async fn verify(
    remote: SocketAddr,
    root: &Path,
    path: &Path,
    file: &str,
    original: &str,
    plan: &BuilderPlan,
) -> Result<Verification> {
    match probe_names(remote, root, path, file, original, plan).await {
        Ok(Probe::Free) => {}
        Ok(Probe::Taken(name, place)) => bail!(
            "`{name}` already names {place} where the builder would be inserted (a glob import, the prelude or an extern crate); the builder would shadow it. Pass another `builder_name`"
        ),
        Ok(Probe::Blind(reason)) => return Ok(Verification::Unverified { reason }),
        Err(err) => {
            return Ok(Verification::Unverified {
                reason: format!(
                    "the analyzer gave no usable answer about which names are free: {err:#}"
                ),
            });
        }
    }
    let (ind, nl) = (&plan.indent, plan.newline);
    let canary = [
        format!("{ind}#[allow(dead_code)]"),
        format!("{ind}fn __prod_code_scope_canary() {{"),
        format!("{ind}    let _ = ().{CANARY_METHOD}();"),
        format!("{ind}}}"),
    ];
    let (probe, first) = insert_after(&plan.file_text, plan.code_lines.1, &canary.join(nl), nl);
    let canary_lines = first..=first + canary.len() as u32 - 1;
    let shift = probe.matches('\n').count() as u32 - plan.file_text.matches('\n').count() as u32;
    let reports =
        match crate::diagnostics::validate_texts(remote, root, &[(path.to_path_buf(), probe)], &[])
            .await
        {
            Ok(reports) => reports,
            Err(err) => {
                return Ok(Verification::Unverified {
                    reason: format!("the analyzer could not check the builder: {err:#}"),
                });
            }
        };
    let errors: Vec<_> = reports
        .iter()
        .flat_map(|r| r.items.iter())
        .filter(|d| d.severity == "error")
        .collect();
    let canary_seen = errors
        .iter()
        .any(|d| canary_lines.contains(&d.line) && d.message.contains(CANARY_METHOD));
    if !canary_seen {
        return Ok(Verification::Unverified {
            reason: format!(
                "the analyzer did not report a deliberate error placed next to the builder in {file}, so its silence about the builder proves nothing (an inactive `cfg`, a file outside the crate, or an engine still loading)"
            ),
        });
    }
    let diagnostics: Vec<String> = errors
        .iter()
        .filter(|d| !canary_lines.contains(&d.line))
        .map(|d| {
            let line = if d.line > *canary_lines.end() {
                d.line - shift
            } else {
                d.line
            };
            format!(
                "{}{} ({file}:{line}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.col
            )
        })
        .collect();
    Ok(if diagnostics.is_empty() {
        Verification::Clean
    } else {
        Verification::Rejected { diagnostics }
    })
}

/// Asks the analyzer what the struct's name and the builder's names resolve to at the insertion
/// point, in the file as it is: the struct's name must resolve to its declaration (or the
/// analyzer is not looking), the builder's must resolve to nothing.
async fn probe_names(
    remote: SocketAddr,
    root: &Path,
    path: &Path,
    file: &str,
    original: &str,
    plan: &BuilderPlan,
) -> Result<Probe> {
    let names = [
        ("__ProdCodeProbeTarget", plan.type_name.as_str()),
        ("__ProdCodeProbeBuilder", plan.builder_name.as_str()),
        ("__ProdCodeProbeError", plan.error_name.as_str()),
    ];
    let prefixes: Vec<String> = names
        .iter()
        .map(|(alias, _)| format!("{}#[allow(dead_code)] type {alias} = ", plan.indent))
        .collect();
    let snippet = names
        .iter()
        .zip(&prefixes)
        .map(|((_, name), prefix)| format!("{prefix}{name};"))
        .collect::<Vec<_>>()
        .join(plan.newline);
    let (text, first) = insert_after(original, plan.declaration_lines.1, &snippet, plan.newline);
    let mut session =
        crate::session::LspSession::open_for_validation(remote, root, Some(path)).await?;
    let uri = session.open_text(path, &text).await?;
    let mut answers = Vec::new();
    for (k, prefix) in prefixes.iter().enumerate() {
        let answer = session
            .request(
                "textDocument/definition",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": {
                        "line": first - 1 + k as u32,
                        "character": prefix.encode_utf16().count(),
                    },
                }),
            )
            .await;
        match answer {
            Ok(answer) => answers.push(answer),
            Err(err) => {
                session.close().await;
                return Err(err);
            }
        }
    }
    session.close().await;
    // A location that cannot be read is not the absence of one: the name could be taken.
    let resolved = names
        .iter()
        .zip(&answers)
        .map(|((_, name), answer)| {
            locations(answer)
                .with_context(|| format!("the answer for `{name}` at the insertion point"))
        })
        .collect::<Result<Vec<_>>>()?;
    let declared = resolved[0].iter().any(|(target, line)| {
        (target == path || target.ends_with(file))
            && (plan.declaration_lines.0..=plan.declaration_lines.1).contains(line)
    });
    if !declared {
        return Ok(Probe::Blind(format!(
            "the analyzer did not resolve `{}` to its declaration at the insertion point, so it cannot tell which names are free there (an engine still loading, or a scope it does not analyse)",
            plan.type_name
        )));
    }
    for ((_, name), targets) in names.iter().zip(&resolved).skip(1) {
        if let Some((target, line)) = targets.first() {
            let shown = target
                .strip_prefix(root)
                .unwrap_or(target)
                .to_string_lossy()
                .into_owned();
            return Ok(Probe::Taken(name.to_string(), format!("{shown}:{line}")));
        }
    }
    Ok(Probe::Free)
}

/// The files and 1-based lines a definition answer points at: none for `null` or `[]`, else a
/// `Location`, `Location[]` or `LocationLink[]`. Anything else, or an entry without a file URI
/// and a valid range, is an error rather than one location fewer.
fn locations(answer: &serde_json::Value) -> Result<Vec<(std::path::PathBuf, u32)>> {
    let items = match answer {
        serde_json::Value::Null => return Ok(Vec::new()),
        serde_json::Value::Array(items) => items.as_slice(),
        other => std::slice::from_ref(other),
    };
    items
        .iter()
        .map(|item| {
            let (uri, range) = match item.get("targetUri") {
                Some(uri) => (
                    Some(uri),
                    item.get("targetSelectionRange")
                        .or_else(|| item.get("targetRange")),
                ),
                None => (item.get("uri"), item.get("range")),
            };
            let path = uri
                .and_then(|u| u.as_str())
                .and_then(|u| url::Url::parse(u).ok())
                .and_then(|u| u.to_file_path().ok());
            match (path, range.and_then(range_lines)) {
                (Some(path), Some((line, _))) => Ok((path, line)),
                _ => bail!("malformed definition answer: {item}"),
            }
        })
        .collect()
}

/// `r#type` -> `type`.
fn bare(name: &str) -> &str {
    name.strip_prefix("r#").unwrap_or(name)
}

fn is_plain_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
        && chars.all(|c| c == '_' || c.is_alphanumeric())
        && name != "_"
        && !KEYWORDS.contains(&name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ident,
    Lifetime,
    Literal,
    Punct,
}

/// A token's kind and its byte range in the text.
#[derive(Debug, Clone, Copy)]
struct Token {
    kind: Kind,
    start: usize,
    end: usize,
}

/// A file's text and its tokens, comments left out.
struct Source<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    line_starts: Vec<usize>,
}

impl<'a> Source<'a> {
    fn new(text: &'a str) -> Result<Self> {
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Ok(Self {
            text,
            tokens: lex(text)?,
            line_starts,
        })
    }

    fn t(&self, i: usize) -> &'a str {
        let token = self.tokens[i];
        &self.text[token.start..token.end]
    }

    fn is(&self, i: usize, text: &str) -> bool {
        i < self.tokens.len() && self.tokens[i].kind != Kind::Literal && self.t(i) == text
    }

    /// The 1-based line token `i` starts on.
    fn line(&self, i: usize) -> u32 {
        self.line_starts
            .partition_point(|&s| s <= self.tokens[i].start) as u32
    }

    /// The closing bracket of the one opened at token `open`.
    fn close_of(&self, open: usize) -> Option<usize> {
        let mut depth = 0i32;
        for i in open..self.tokens.len() {
            if self.tokens[i].kind != Kind::Punct {
                continue;
            }
            match self.t(i) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The opening bracket of the one closed at token `close`.
    fn open_of(&self, close: usize) -> Option<usize> {
        let mut depth = 0i32;
        for i in (0..=close).rev() {
            if self.tokens[i].kind != Kind::Punct {
                continue;
            }
            match self.t(i) {
                ")" | "]" | "}" => depth += 1,
                "(" | "[" | "{" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// `serde`, `rustfmt::skip`: the path an attribute starts with, from token `from`.
    fn attribute_path(&self, from: usize, to: usize) -> String {
        let mut path = String::new();
        for i in from..to {
            match self.tokens[i].kind {
                Kind::Ident => path.push_str(self.t(i)),
                Kind::Punct if self.t(i) == "::" => path.push_str("::"),
                _ => break,
            }
        }
        path
    }

    /// Tokens `from..to` as written, with every gap between two of them (whitespace, comments,
    /// line breaks) collapsed to one space.
    fn spell(&self, from: usize, to: usize) -> String {
        let mut out = String::new();
        for i in from..to {
            if i > from && self.tokens[i].start > self.tokens[i - 1].end {
                out.push(' ');
            }
            out.push_str(self.t(i));
        }
        out
    }
}

/// Splits Rust source into identifiers, lifetimes, literals and punctuation, dropping
/// whitespace and comments. `::`, `->` and `=>` are single tokens, so a `>` is always a bracket.
fn lex(text: &str) -> Result<Vec<Token>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let at = |i: usize| chars.get(i).map(|&(_, c)| c);
    let offset = |i: usize| chars.get(i).map_or(text.len(), |&(o, _)| o);
    let ident_start = |c: char| c == '_' || c.is_alphabetic();
    let ident_char = |c: char| c == '_' || c.is_alphanumeric();
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(c) = at(i) {
        let start = i;
        let mut push = |kind: Kind, end: usize| {
            tokens.push(Token {
                kind,
                start: offset(start),
                end: offset(end),
            })
        };
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && at(i + 1) == Some('/') {
            while at(i).is_some_and(|c| c != '\n') {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == Some('*') {
            let mut depth = 0usize;
            loop {
                match (at(i), at(i + 1)) {
                    (Some('/'), Some('*')) => {
                        depth += 1;
                        i += 2;
                    }
                    (Some('*'), Some('/')) => {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    }
                    (Some(_), _) => i += 1,
                    (None, _) => bail!("unterminated block comment at byte {}", offset(start)),
                }
            }
        } else if c == 'r' && at(i + 1) == Some('#') && at(i + 2).is_some_and(ident_start) {
            i += 2;
            while at(i).is_some_and(ident_char) {
                i += 1;
            }
            push(Kind::Ident, i);
        } else if let Some(end) = quoted_end(&chars, i)
            .with_context(|| format!("cannot read the literal at byte {}", offset(start)))?
        {
            i = end;
            push(Kind::Literal, i);
        } else if ident_start(c) {
            while at(i).is_some_and(ident_char) {
                i += 1;
            }
            push(Kind::Ident, i);
        } else if c.is_ascii_digit() {
            while at(i).is_some_and(|c| c == '_' || c.is_ascii_alphanumeric())
                || (at(i) == Some('.') && at(i + 1).is_some_and(|c| c.is_ascii_digit()))
            {
                i += 1;
            }
            push(Kind::Literal, i);
        } else if c == '\'' {
            if at(i + 1) == Some('\\') {
                let mut j = i + 3;
                while at(j).is_some_and(|c| c != '\'') {
                    j += 1;
                }
                if at(j).is_none() {
                    bail!("unterminated character literal at byte {}", offset(start));
                }
                i = j + 1;
                push(Kind::Literal, i);
            } else if at(i + 1).is_some() && at(i + 2) == Some('\'') {
                i += 3;
                push(Kind::Literal, i);
            } else if at(i + 1).is_some_and(ident_start) {
                i += 1;
                if at(i) == Some('r') && at(i + 1) == Some('#') {
                    i += 2;
                }
                while at(i).is_some_and(ident_char) {
                    i += 1;
                }
                push(Kind::Lifetime, i);
            } else {
                bail!("cannot read the `'` at byte {}", offset(start));
            }
        } else {
            let pair: String = [Some(c), at(i + 1)].into_iter().flatten().collect();
            i += if matches!(pair.as_str(), "::" | "->" | "=>") {
                2
            } else {
                1
            };
            push(Kind::Punct, i);
        }
    }
    Ok(tokens)
}

/// Where a string, raw string or byte character literal starting at `i` ends, if one does.
fn quoted_end(chars: &[(usize, char)], i: usize) -> Result<Option<usize>> {
    let at = |i: usize| chars.get(i).map(|&(_, c)| c);
    let mut j = i;
    if matches!(at(j), Some('b') | Some('c')) {
        j += 1;
    }
    if at(j) == Some('r') && matches!(at(j + 1), Some('"') | Some('#')) {
        j += 1;
        let mut hashes = 0;
        while at(j) == Some('#') {
            hashes += 1;
            j += 1;
        }
        if at(j) != Some('"') {
            return Ok(None);
        }
        j += 1;
        loop {
            match at(j) {
                None => bail!("unterminated raw string"),
                Some('"') if (1..=hashes).all(|k| at(j + k) == Some('#')) => {
                    return Ok(Some(j + 1 + hashes));
                }
                _ => j += 1,
            }
        }
    }
    let quote = match at(j) {
        Some('"') => '"',
        Some('\'') if j == i + 1 && at(i) == Some('b') => '\'',
        _ => return Ok(None),
    };
    j += 1;
    loop {
        match at(j) {
            None => bail!("unterminated literal"),
            Some('\\') => j += 2,
            Some(c) if c == quote => return Ok(Some(j + 1)),
            _ => j += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "use std::collections::HashMap;

/// Settings.
#[derive(Debug, Clone)]
pub struct Config {
    /// The name.
    pub name: String,
    pub(crate) r#type: u8,
    limits: HashMap<String, Vec<(u8, Option<Box<[u16; 4]>>)>>,
    callback: fn(&str) -> Result<u8, String>,
    nested:
        Vec<
            Vec<u8>,
        >,
}

fn other() {}
";

    fn config_plan() -> BuilderPlan {
        plan(CONFIG, "Config", (3, 15), None).expect("a plan")
    }

    #[test]
    fn fields_keep_their_names_and_full_type_spellings() {
        let plan = config_plan();
        let fields: Vec<(&str, &str)> = plan
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.ty.as_str()))
            .collect();
        assert_eq!(
            fields,
            vec![
                ("name", "String"),
                ("r#type", "u8"),
                (
                    "limits",
                    "HashMap<String, Vec<(u8, Option<Box<[u16; 4]>>)>>"
                ),
                ("callback", "fn(&str) -> Result<u8, String>"),
                ("nested", "Vec< Vec<u8>, >"),
            ]
        );
        assert_eq!(plan.builder_name, "ConfigBuilder");
        assert_eq!(plan.error_name, "ConfigBuilderError");
        assert_eq!(plan.visibility, "pub");
        assert_eq!(plan.declaration_lines, (4, 15));
        assert_eq!(plan.insert_after_line, 15);
    }

    #[test]
    fn the_code_has_one_typed_setter_per_field_and_required_slots() {
        let plan = config_plan();
        let code = &plan.code;
        assert!(
            code.contains("    r#type: ::core::option::Option<u8>,"),
            "{code}"
        );
        assert!(
            code.contains("    pub fn r#type(mut self, value: u8) -> Self {"),
            "{code}"
        );
        assert!(
            code.contains("    pub fn limits(mut self, value: HashMap<String, Vec<(u8, Option<Box<[u16; 4]>>)>>) -> Self {"),
            "{code}"
        );
        assert!(
            code.contains(
                "            r#type: self.r#type.ok_or(ConfigBuilderError { field: \"type\" })?,"
            ),
            "{code}"
        );
        assert!(
            code.contains(
                "    pub fn build(self) -> ::core::result::Result<Config, ConfigBuilderError> {"
            ),
            "{code}"
        );
        assert!(!code.contains("Default::default"), "{code}");
        assert_eq!(code.matches("(mut self, value:").count(), 5, "{code}");
    }

    #[test]
    fn the_builder_goes_after_the_declaration_and_the_rest_is_untouched() {
        let plan = config_plan();
        let (before, after) = CONFIG.split_at(CONFIG.find("\nfn other").unwrap() + 1);
        assert!(plan.file_text.starts_with(before), "{}", plan.file_text);
        assert!(plan.file_text.ends_with(after), "{}", plan.file_text);
        let lines: Vec<&str> = plan.file_text.lines().collect();
        assert_eq!(lines[plan.insert_after_line as usize - 1], "}");
        assert_eq!(lines[plan.insert_after_line as usize], "");
        assert_eq!(
            lines[plan.code_lines.0 as usize - 1],
            "/// Builds a `Config` one field at a time. Every field is required: `ConfigBuilder::build`"
        );
        assert_eq!(
            lines[plan.code_lines.1 as usize - 1],
            "impl ::core::error::Error for ConfigBuilderError {}"
        );
    }

    #[test]
    fn a_declaration_in_a_module_is_indented_like_it() {
        let text = "mod inner {\n    pub(super) struct Point {\n        pub x: i32,\n    }\n}\n";
        let plan = plan(text, "Point", (2, 4), Some("PointMaker")).unwrap();
        assert_eq!(plan.error_name, "PointMakerError");
        assert!(
            plan.code.contains(
                "\n    pub(super) struct PointMaker {\n        x: ::core::option::Option<i32>,"
            ),
            "{}",
            plan.code
        );
        assert!(
            plan.file_text
                .ends_with("    }\n\n    impl ::core::error::Error for PointMakerError {}\n}\n"),
            "{}",
            plan.file_text
        );
        assert!(plan.file_text.contains("    }\n\n    /// Builds a `Point`"));
    }

    #[test]
    fn unsupported_shapes_are_refused_with_the_reason() {
        for (text, name, expected) in [
            ("pub struct P(u8, u16);\n", "P", "tuple struct"),
            ("pub struct P;\n", "P", "unit struct"),
            ("pub enum P {\n    A,\n}\n", "P", "is an enum"),
            ("pub union P {\n    a: u8,\n}\n", "P", "is a union"),
            (
                "#[cfg(test)]\npub struct P {\n    a: u8,\n}\n",
                "P",
                "`#[cfg(…)]`",
            ),
            (
                "pub struct P {\n    #[cfg(feature = \"x\")]\n    a: u8,\n}\n",
                "P",
                "field `a` of `P` carries `#[cfg(…)]`",
            ),
            (
                "pub struct P {\n    #[cfg_attr(test, allow(unused))]\n    a: u8,\n}\n",
                "P",
                "`#[cfg_attr(…)]`",
            ),
            (
                "#[my_macro]\npub struct P {\n    a: u8,\n}\n",
                "P",
                "attribute macro",
            ),
            (
                "pub struct P {\n    next: Option<Box<Self>>,\n}\n",
                "P",
                "spells `Self`",
            ),
            ("pub struct P {\n    a: u8 = 3,\n}\n", "P", "default value"),
            (
                "pub struct P {\n    build: u8,\n}\n",
                "P",
                "setter called `build`",
            ),
            (
                "pub struct P {\n    new: u8,\n}\n",
                "P",
                "setter called `new`",
            ),
            (
                "pub struct P {\n    a: u8,\n} fn f() {}\n",
                "P",
                "shares its last line",
            ),
        ] {
            let err = plan(text, name, (1, 4), None).expect_err(text);
            let err = format!("{err:#}");
            assert!(err.contains(expected), "{text}\n=> {err}");
        }
    }

    #[test]
    fn names_already_in_the_file_are_refused() {
        for text in [
            "pub struct P {\n    a: u8,\n}\npub struct PBuilder;\n",
            "use other::PBuilderError;\npub struct P {\n    a: u8,\n}\n",
            "pub struct P {\n    a: u8,\n}\nfn f() { let PBuilder = 1; }\n",
        ] {
            let err = format!("{:#}", plan(text, "P", (1, 4), None).expect_err(text));
            assert!(err.contains("already appears in this file"), "{err}");
        }
        // In a comment or a string it is not a name.
        let text =
            "// PBuilder\npub struct P {\n    a: &'static str,\n}\nconst S: &str = \"PBuilder\";\n";
        assert!(plan(text, "P", (1, 4), None).is_ok());
        let err = format!(
            "{:#}",
            plan("pub struct P {\n    a: u8,\n}\n", "P", (1, 3), Some("P")).unwrap_err()
        );
        assert!(err.contains("struct's own name"), "{err}");
        let err = format!(
            "{:#}",
            plan("pub struct P {\n    a: u8,\n}\n", "P", (1, 3), Some("fn")).unwrap_err()
        );
        assert!(err.contains("not an identifier"), "{err}");
    }

    #[test]
    fn helper_attributes_next_to_a_derive_are_noted_not_refused() {
        let text = "#[derive(Serialize)]\n#[serde(rename_all = \"camelCase\")]\n#[rustfmt::skip]\npub struct P {\n    #[serde(default)]\n    a: u8,\n}\n";
        let plan = plan(text, "P", (1, 7), None).unwrap();
        assert_eq!(plan.declaration_lines, (1, 7));
        assert_eq!(plan.notes.len(), 1, "{:?}", plan.notes);
        assert!(plan.notes[0].contains("`#[serde]`"), "{:?}", plan.notes);
    }

    #[test]
    fn many_fields_are_all_generated() {
        let mut text = String::from("pub struct Wide {\n");
        for i in 0..64 {
            text.push_str(&format!("    pub f{i}: u{},\n", 8 << (i % 4)));
        }
        text.push_str("}\n");
        let plan = plan(&text, "Wide", (1, 66), None).unwrap();
        assert_eq!(plan.fields.len(), 64);
        assert!(
            plan.code
                .contains("pub fn f63(mut self, value: u64) -> Self {")
        );
        assert!(
            plan.code
                .contains("f63: self.f63.ok_or(WideBuilderError { field: \"f63\" })?,")
        );
    }

    #[test]
    fn a_raw_struct_name_and_crlf_line_endings_are_kept() {
        let text = "pub struct r#Match {\r\n    pub r#in: u8,\r\n}\r\n";
        let plan = plan(text, "Match", (1, 3), None).unwrap();
        assert_eq!(plan.builder_name, "MatchBuilder");
        assert!(plan.code.contains("Result<r#Match, MatchBuilderError>"));
        assert!(
            plan.code
                .contains("::core::result::Result::Ok(r#Match {\r\n")
        );
        assert!(!plan.file_text.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn the_lexer_reads_literals_lifetimes_and_comments() {
        let text = "a /* x /* y */ z */ 'b' b'\\'' '\\u{1F600}' 'static r#\"q\"# br##\"w\"## c\"e\" 1.5e3 -> :: // tail\n";
        let src = Source::new(text).unwrap();
        let kinds: Vec<(Kind, &str)> = (0..src.tokens.len())
            .map(|i| (src.tokens[i].kind, src.t(i)))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (Kind::Ident, "a"),
                (Kind::Literal, "'b'"),
                (Kind::Literal, "b'\\''"),
                (Kind::Literal, "'\\u{1F600}'"),
                (Kind::Lifetime, "'static"),
                (Kind::Literal, "r#\"q\"#"),
                (Kind::Literal, "br##\"w\"##"),
                (Kind::Literal, "c\"e\""),
                (Kind::Literal, "1.5e3"),
                (Kind::Punct, "->"),
                (Kind::Punct, "::"),
            ]
        );
        assert!(lex("/* open").is_err());
        assert!(lex("\"open").is_err());
    }

    #[test]
    fn the_outline_node_is_the_struct_not_a_field_on_its_line() {
        let symbols = serde_json::json!([{
            "name": "P", "kind": 23,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 30 } },
            "children": [
                { "name": "r#a", "kind": 8,
                  "range": { "start": { "line": 0, "character": 15 }, "end": { "line": 0, "character": 20 } } }
            ]
        }]);
        assert_eq!(
            outline_node(&symbols, "P", 1).unwrap(),
            Some(OutlineNode {
                start: 1,
                end: 1,
                fields: Some(vec!["r#a".into()])
            })
        );
        assert_eq!(outline_node(&symbols, "Q", 1).unwrap(), None);
        assert_eq!(outline_node(&symbols, "P", 2).unwrap(), None);
        assert_eq!(
            outline_node(&serde_json::Value::Null, "P", 1).unwrap(),
            None
        );

        // The Rust engine's flat answer: a field is named by its container path and its line.
        let at = |line: u64| serde_json::json!({ "start": { "line": line, "character": 4 }, "end": { "line": line, "character": 0 } });
        let flat = serde_json::json!([
            { "name": "network", "kind": 2, "location": { "uri": "file:///w/a.rs", "range": at(0) } },
            { "name": "P", "kind": 23, "containerName": "network", "location": { "uri": "file:///w/a.rs", "range": { "start": { "line": 2, "character": 15 }, "end": { "line": 5, "character": 0 } } } },
            { "name": "r#type", "kind": 8, "containerName": "network > P", "location": { "uri": "file:///w/a.rs", "range": at(3) } },
            { "name": "b", "kind": 8, "containerName": "network > P", "location": { "uri": "file:///w/a.rs", "range": at(4) } },
            { "name": "c", "kind": 8, "containerName": "network > Q", "location": { "uri": "file:///w/a.rs", "range": at(8) } },
        ]);
        assert_eq!(
            outline_node(&flat, "P", 3).unwrap(),
            Some(OutlineNode {
                start: 3,
                end: 6,
                fields: Some(vec!["r#type".into(), "b".into()])
            })
        );
    }

    /// A range over the 0-based lines `from..=to`, written as JSON.
    fn span(from: serde_json::Value, to: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "start": { "line": from, "character": 0 }, "end": { "line": to, "character": 1 } })
    }

    #[test]
    fn coordinates_are_converted_checked() {
        let n = |v: serde_json::Value| one_based(Some(&v));
        assert_eq!(n(serde_json::json!(0)), Some(1));
        assert_eq!(n(serde_json::json!(u32::MAX - 1)), Some(u32::MAX));
        assert_eq!(n(serde_json::json!(u32::MAX)), None);
        assert_eq!(n(serde_json::json!(1u64 << 32)), None);
        assert_eq!(n(serde_json::json!(-1)), None);
        assert_eq!(n(serde_json::json!(1.5)), None);
        assert_eq!(n(serde_json::json!("3")), None);
        assert_eq!(one_based(None), None);
        assert_eq!(
            range_lines(&span(serde_json::json!(2), serde_json::json!(4))),
            Some((3, 5))
        );
        // Zero width, as sourcekit-lsp answers, is a range.
        let point = serde_json::json!({ "line": 2, "character": 3 });
        assert_eq!(
            range_lines(&serde_json::json!({ "start": point, "end": point })),
            Some((3, 3))
        );
        // A field in the Rust engine's flat outline: it ends at character 0 of the line it
        // starts on, which rust-analyzer behind the gateway really answers.
        assert_eq!(
            range_lines(
                &serde_json::json!({ "start": { "line": 27, "character": 8 }, "end": { "line": 27, "character": 0 } })
            ),
            Some((28, 28))
        );
        for bad in [
            span(serde_json::json!(4), serde_json::json!(2)),
            serde_json::json!({ "start": { "line": 2, "character": -5 }, "end": { "line": 2, "character": 1 } }),
            serde_json::json!({ "start": { "line": 2, "character": 0 }, "end": { "line": 2, "character": u32::MAX } }),
            serde_json::json!({ "start": { "line": 2 }, "end": { "line": 2, "character": 1 } }),
            serde_json::json!({ "start": { "line": 2, "character": 0 } }),
            span(serde_json::json!(u32::MAX), serde_json::json!(u32::MAX)),
            serde_json::json!(null),
        ] {
            assert_eq!(range_lines(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_malformed_outline_is_an_error_not_a_panic_or_a_guess() {
        let node = |range: serde_json::Value| serde_json::json!([{ "name": "P", "kind": 23, "range": range }]);
        let past_u32 = 1u64 << 32;
        for (outline, expected) in [
            (
                node(span(serde_json::json!(0), serde_json::json!(u32::MAX))),
                "a range that is not one",
            ),
            (
                node(span(
                    serde_json::json!(past_u32),
                    serde_json::json!(past_u32 + 2),
                )),
                "a range that is not one",
            ),
            (
                node(span(serde_json::json!(-1), serde_json::json!(2))),
                "a range that is not one",
            ),
            (
                serde_json::json!([{ "name": "P", "kind": 23 }]),
                "a range that is not one: none",
            ),
            (
                serde_json::json!([{ "name": "P", "kind": 23, "range": span(serde_json::json!(0), serde_json::json!(2)),
                    "children": [{ "kind": 8, "range": span(serde_json::json!(1), serde_json::json!(1)) }] }]),
                "a field of `P` without a name",
            ),
            (
                serde_json::json!([{ "name": "P", "kind": 23, "range": span(serde_json::json!(0), serde_json::json!(2)),
                    "children": [{ "name": "a" }] }]),
                "a member of `P` without a kind",
            ),
            (
                serde_json::json!([{ "name": "m", "kind": 2, "range": span(serde_json::json!(0), serde_json::json!(9)),
                    "children": "P" }]),
                "members that are not a list",
            ),
            (
                serde_json::json!([
                    { "name": "P", "kind": 23, "range": span(serde_json::json!(0), serde_json::json!(2)) },
                    { "name": "a", "kind": 8, "containerName": "P",
                      "range": span(serde_json::json!(past_u32 + 1), serde_json::json!(past_u32 + 1)) },
                ]),
                "field `a` of `P` a range that is not one",
            ),
            (
                serde_json::json!([
                    { "name": "P", "kind": 23, "range": span(serde_json::json!(0), serde_json::json!(2)) },
                    { "kind": 8, "containerName": "P", "range": span(serde_json::json!(1), serde_json::json!(1)) },
                ]),
                "a field of `P` without a name",
            ),
            (serde_json::json!({ "name": "P" }), "not a list of symbols"),
        ] {
            let err = outline_node(&outline, "P", 1).expect_err(&outline.to_string());
            let err = format!("{err:#}");
            assert!(err.contains(expected), "{outline}\n=> {err}");
        }
        // What is not about `P` does not stop it: another name's range, a field of another
        // struct, members of another kind, `null` children.
        let fine = serde_json::json!([
            { "name": "Q", "kind": 23, "range": span(serde_json::json!(u32::MAX), serde_json::json!(-1)) },
            { "name": "P", "kind": 23, "range": span(serde_json::json!(0), serde_json::json!(2)),
              "children": [
                  { "name": "a", "kind": 8, "range": span(serde_json::json!(1), serde_json::json!(1)) },
                  { "name": "f", "kind": 6, "range": span(serde_json::json!(1), serde_json::json!(1)), "children": null },
              ] },
            { "name": "b", "kind": 8, "containerName": "Q", "range": span(serde_json::json!(-1), serde_json::json!(-1)) },
        ]);
        assert_eq!(
            outline_node(&fine, "P", 2).unwrap(),
            Some(OutlineNode {
                start: 1,
                end: 3,
                fields: Some(vec!["a".into()])
            })
        );
    }

    #[test]
    fn definition_answers_of_every_shape_are_read() {
        let link = serde_json::json!([{ "targetUri": "file:///w/src/lib.rs",
            "targetRange": { "start": { "line": 1, "character": 0 }, "end": { "line": 3, "character": 1 } },
            "targetSelectionRange": { "start": { "line": 2, "character": 4 }, "end": { "line": 2, "character": 5 } } }]);
        assert_eq!(
            locations(&link).unwrap(),
            vec![(std::path::PathBuf::from("/w/src/lib.rs"), 3)]
        );
        let link_without_selection = serde_json::json!([{ "targetUri": "file:///w/src/lib.rs",
            "targetRange": { "start": { "line": 1, "character": 0 }, "end": { "line": 3, "character": 1 } } }]);
        assert_eq!(
            locations(&link_without_selection).unwrap(),
            vec![(std::path::PathBuf::from("/w/src/lib.rs"), 2)]
        );
        let single = serde_json::json!({ "uri": "file:///w/a.rs",
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } } });
        assert_eq!(locations(&single).unwrap().len(), 1);
        assert!(locations(&serde_json::Value::Null).unwrap().is_empty());
        assert!(locations(&serde_json::json!([])).unwrap().is_empty());
    }

    #[test]
    fn a_malformed_definition_answer_is_an_error_not_an_empty_one() {
        let range = span(serde_json::json!(8), serde_json::json!(8));
        let uri = "file:///w/a.rs";
        for answer in [
            serde_json::json!([{ "uri": uri }]),
            serde_json::json!([{ "range": range }]),
            serde_json::json!([{ "uri": 7, "range": range }]),
            serde_json::json!([{ "uri": "not a uri", "range": range }]),
            serde_json::json!([{ "uri": "untitled:Untitled-1", "range": range }]),
            serde_json::json!([{ "targetUri": uri, "range": range }]),
            serde_json::json!([{ "targetUri": uri, "targetSelectionRange": span(serde_json::json!(u32::MAX), serde_json::json!(u32::MAX)) }]),
            serde_json::json!([{ "uri": uri, "range": span(serde_json::json!(1u64 << 32), serde_json::json!(1u64 << 32)) }]),
            serde_json::json!([{ "uri": uri, "range": span(serde_json::json!(-1), serde_json::json!(0)) }]),
            // One good location does not make up for a bad one next to it.
            serde_json::json!([{ "uri": uri, "range": range }, { "uri": uri }]),
            serde_json::json!([null]),
            serde_json::json!("P"),
            serde_json::json!(true),
        ] {
            let err = locations(&answer).expect_err(&answer.to_string());
            assert!(
                format!("{err:#}").contains("malformed definition answer"),
                "{err:#}"
            );
        }
    }

    #[test]
    fn the_report_tells_verified_from_unverified() {
        let mut preview = BuilderPreview {
            file: "src/lib.rs".into(),
            plan: config_plan(),
            verification: Verification::Clean,
        };
        assert!(preview.verified());
        assert!(
            preview
                .render()
                .contains("verified: the analyzer checked it")
        );
        assert!(preview.render().contains("nothing was written"));
        preview.verification = Verification::Rejected {
            diagnostics: vec!["mismatched types [E0308] (src/lib.rs:20:5)".into()],
        };
        assert!(!preview.verified());
        assert_eq!(preview.diagnostics().len(), 1);
        assert!(preview.render().contains("rejected:"));
        preview.verification = Verification::Unverified {
            reason: "not asked".into(),
        };
        assert!(!preview.verified());
        assert!(preview.diagnostics().is_empty());
        assert!(preview.render().contains("not verified: not asked"));
    }
}
