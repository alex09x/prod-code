//! Fixture generation (roadmap 8.5): a compile-ready value for a type, built from the shape
//! the analyzer reports and checked by the analyzer before it is shown.
//!
//! A test needs a `Config` with nineteen fields and an agent does not know their names, their
//! order or their types. It can read the declaration, which costs context and still produces
//! a literal that misses a field added last week. The analyzer already knows the shape:
//! `textDocument/hover` on a struct returns its fields with their types, resolved.
//!
//! So the generator asks for the shape, maps each field's type to a value, recurses into types
//! declared in the workspace, and then does the part that matters: it places the result in an
//! in-memory overlay of the file that declares the type and asks the analyzer whether it
//! compiles. Nothing is written, and a fixture that does not type-check is reported as such
//! rather than handed over.

use crate::tools::{SymbolHit, workspace_symbol_search};
use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::Path;

/// How deep to build nested workspace types before falling back to `Default::default()`.
pub const DEFAULT_DEPTH: u32 = 2;
/// Fields generated for one type; a bigger struct is still generated, just noted.
const MANY_FIELDS: usize = 40;

/// A generated fixture and what the analyzer said about it.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub type_name: String,
    /// The value expression, formatted over several lines.
    pub value: String,
    /// The file that declares the type, relative to the workspace root.
    pub file: String,
    /// Types that could not be built field by field and fell back to `Default::default()`.
    pub fallbacks: Vec<String>,
    /// What the analyzer reported for the generated code, empty when it is clean.
    pub diagnostics: Vec<String>,
    pub verified: bool,
}

impl Fixture {
    pub fn render(&self) -> String {
        let mut out = format!(
            "fixture for `{}` (declared in {})\n\n```rust\nlet {} = {};\n```\n",
            self.type_name,
            self.file,
            snake_case(&self.type_name),
            self.value
        );
        if !self.fallbacks.is_empty() {
            let mut names = self.fallbacks.clone();
            names.sort();
            names.dedup();
            out.push_str(&format!(
                "\n`Default::default()` stands in for: {} (not declared in this workspace, or deeper than the depth limit)\n",
                names.join(", ")
            ));
        }
        match (self.verified, self.diagnostics.is_empty()) {
            (true, true) => out.push_str("\nthe analyzer accepts it: 0 errors\n"),
            (true, false) => {
                out.push_str("\nthe analyzer rejects it:\n");
                for d in &self.diagnostics {
                    out.push_str(&format!("  {d}\n"));
                }
            }
            (false, _) => out.push_str("\nnot verified (pass `verify: true` to type-check it)\n"),
        }
        out
    }
}

/// `SliceReport` -> `slice_report`.
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

/// The declaration body hover returns, without the markdown fences and the module line.
/// Kept for the fallback path and because it documents the shape hover answers with; the
/// generator reads the file instead, since hover elides fields past the tenth.
#[allow(dead_code)]
fn declaration_from_hover(hover: &str) -> Option<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in hover.lines() {
        if line.trim_start().starts_with("```") {
            match current.take() {
                Some(block) => blocks.push(block),
                None => current = Some(String::new()),
            }
            continue;
        }
        if let Some(block) = current.as_mut() {
            block.push_str(line);
            block.push('\n');
        }
    }
    // The first block is the module path, the one that declares the item is what we want.
    blocks.into_iter().find(|b| {
        let t = b.trim_start();
        t.starts_with("pub struct")
            || t.starts_with("struct")
            || t.starts_with("pub enum")
            || t.starts_with("enum")
    })
}

/// The shape of a type as the analyzer printed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// A struct with named fields: `(name, type)`.
    Record(Vec<(String, String)>),
    /// A tuple struct: the field types in order.
    Tuple(Vec<String>),
    /// A unit struct.
    Unit,
    /// An enum: its variant names, first one usable as a value when it takes no fields.
    Enum(Vec<String>),
}

/// Reads a declaration into a shape. Doc comments and attributes are ignored.
pub fn parse_shape(decl: &str) -> Option<Shape> {
    let head = decl.trim_start();
    let is_enum = head.starts_with("enum") || head.starts_with("pub enum");
    let Some(open) = decl.find('{') else {
        // `struct Name(A, B);` or `struct Name;`
        if let Some(open) = decl.find('(') {
            let close = decl.rfind(')')?;
            let types = split_top_level(&decl[open + 1..close])
                .into_iter()
                .map(|t| strip_visibility(&t).to_string())
                .filter(|t| !t.is_empty())
                .collect();
            return Some(Shape::Tuple(types));
        }
        return Some(Shape::Unit);
    };
    let close = decl.rfind('}')?;
    let body = &decl[open + 1..close];
    if is_enum {
        let variants = body
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('#'))
            .filter_map(|l| {
                let name: String = l
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                (!name.is_empty()).then_some(name)
            })
            .collect();
        return Some(Shape::Enum(variants));
    }
    let mut fields = Vec::new();
    for raw in split_top_level(body) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        let line = strip_visibility(line);
        let Some((name, ty)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        fields.push((name.to_string(), ty.trim().to_string()));
    }
    Some(Shape::Record(fields))
}

fn strip_visibility(line: &str) -> &str {
    let t = line.trim_start();
    for prefix in ["pub(crate)", "pub(super)", "pub(in crate)", "pub"] {
        if let Some(rest) = t.strip_prefix(prefix) {
            return rest.trim_start();
        }
    }
    t
}

/// Splits on commas that are not inside brackets, so `HashMap<String, u64>` stays whole.
fn split_top_level(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '<' | '(' | '[' => {
                depth += 1;
                current.push(c);
            }
            '>' | ')' | ']' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut current));
            }
            '\n' if depth == 0 => {
                out.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    out.push(current);
    out
}

/// The generic argument of `Wrapper<T>`, if this is one.
fn inner_of<'a>(ty: &'a str, wrapper: &str) -> Option<&'a str> {
    let t = ty.trim();
    let name = t.split('<').next()?.trim().rsplit("::").next()?;
    if name != wrapper {
        return None;
    }
    let open = t.find('<')?;
    let close = t.rfind('>')?;
    Some(t[open + 1..close].trim())
}

/// A value expression for a type, without consulting the analyzer.
///
/// Returns `None` when the type is not one we know how to build, so the caller can try the
/// workspace for its declaration and fall back to `Default::default()`.
pub fn known_value(ty: &str) -> Option<String> {
    // `&'a mut str`, `'static str` and `str` are the same type for this purpose, and a caller
    // may already have stripped the reference.
    let mut t = ty.trim();
    loop {
        let before = t;
        t = t.trim_start_matches('&').trim_start();
        if let Some(rest) = t.strip_prefix('\'') {
            t = rest
                .split_once(char::is_whitespace)
                .map(|(_, rest)| rest)
                .unwrap_or("")
                .trim_start();
        }
        t = t.strip_prefix("mut ").unwrap_or(t).trim_start();
        if t == before {
            break;
        }
    }
    let bare = t.rsplit("::").next().unwrap_or(t).trim();
    let simple = bare.split('<').next().unwrap_or(bare).trim();
    let v = match simple {
        "bool" => "false".to_string(),
        "char" => "'a'".to_string(),
        "f32" | "f64" => "0.0".to_string(),
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" => "0".to_string(),
        "String" => "String::new()".to_string(),
        "str" => "\"\"".to_string(),
        "PathBuf" => "std::path::PathBuf::new()".to_string(),
        "Duration" => "std::time::Duration::from_secs(0)".to_string(),
        "Instant" => "std::time::Instant::now()".to_string(),
        "SocketAddr" => "\"127.0.0.1:0\".parse().unwrap()".to_string(),
        "Vec" | "VecDeque" | "HashMap" | "BTreeMap" | "HashSet" | "BTreeSet" | "BinaryHeap" => {
            format!("{simple}::new()")
        }
        "Option" => "None".to_string(),
        "AtomicBool" => "std::sync::atomic::AtomicBool::new(false)".to_string(),
        "AtomicU8" | "AtomicU16" | "AtomicU32" | "AtomicU64" | "AtomicUsize" | "AtomicI32"
        | "AtomicI64" | "AtomicIsize" => format!("std::sync::atomic::{simple}::new(0)"),
        _ => return None,
    };
    Some(v)
}

/// Wrappers whose value is the wrapped value in a constructor.
fn wrapper_value(ty: &str, inner_value: impl FnOnce(&str) -> String) -> Option<String> {
    for (wrapper, ctor) in [
        ("Box", "Box::new"),
        ("Arc", "std::sync::Arc::new"),
        ("Rc", "std::rc::Rc::new"),
        ("Mutex", "std::sync::Mutex::new"),
        ("RwLock", "std::sync::RwLock::new"),
        ("RefCell", "std::cell::RefCell::new"),
        ("Cell", "std::cell::Cell::new"),
    ] {
        if let Some(inner) = inner_of(ty, wrapper) {
            return Some(format!("{ctor}({})", inner_value(inner)));
        }
    }
    None
}

/// Builds the value for one type, asking the analyzer about types it does not know.
async fn value_for(
    remote: SocketAddr,
    root: &Path,
    ty: &str,
    depth: u32,
    indent: usize,
    seen: &mut Vec<String>,
    fallbacks: &mut Vec<String>,
) -> String {
    let t = ty.trim().trim_start_matches('&').trim();
    if t.is_empty() || t == "()" {
        return "()".to_string();
    }
    if let Some(v) = known_value(t) {
        return v;
    }
    if let Some(v) = wrapper_inner(remote, root, t, depth, indent, seen, fallbacks).await {
        return v;
    }
    // A tuple type: build each element.
    if t.starts_with('(') && t.ends_with(')') {
        let mut parts = Vec::new();
        for element in split_top_level(&t[1..t.len() - 1]) {
            if element.trim().is_empty() {
                continue;
            }
            parts.push(
                Box::pin(value_for(
                    remote, root, &element, depth, indent, seen, fallbacks,
                ))
                .await,
            );
        }
        return format!("({})", parts.join(", "));
    }
    let name = t
        .split('<')
        .next()
        .unwrap_or(t)
        .rsplit("::")
        .next()
        .unwrap_or(t)
        .trim();
    if depth == 0 || seen.iter().any(|s| s == name) {
        fallbacks.push(name.to_string());
        return "Default::default()".to_string();
    }
    seen.push(name.to_string());
    let built = build_literal(remote, root, name, depth - 1, indent, seen, fallbacks, None).await;
    seen.pop();
    match built {
        Some(literal) => literal,
        None => {
            fallbacks.push(name.to_string());
            "Default::default()".to_string()
        }
    }
}

/// `Box<T>`, `Arc<T>` and friends, whose inner value has to be built first.
async fn wrapper_inner(
    remote: SocketAddr,
    root: &Path,
    ty: &str,
    depth: u32,
    indent: usize,
    seen: &mut Vec<String>,
    fallbacks: &mut Vec<String>,
) -> Option<String> {
    for wrapper in ["Box", "Arc", "Rc", "Mutex", "RwLock", "RefCell", "Cell"] {
        if let Some(inner) = inner_of(ty, wrapper) {
            let inner_value = Box::pin(value_for(
                remote, root, inner, depth, indent, seen, fallbacks,
            ))
            .await;
            return wrapper_value(ty, |_| inner_value);
        }
    }
    None
}

/// The literal for a type declared in the workspace, or `None` when it cannot be found.
#[allow(clippy::too_many_arguments)]
async fn build_literal(
    remote: SocketAddr,
    root: &Path,
    name: &str,
    depth: u32,
    indent: usize,
    seen: &mut Vec<String>,
    fallbacks: &mut Vec<String>,
    hint: Option<&Path>,
) -> Option<String> {
    let (shape, _path) = shape_of(remote, root, name, hint).await.ok()?;
    let pad = "    ".repeat(indent + 1);
    let closing = "    ".repeat(indent);
    match shape {
        Shape::Unit => Some(name.to_string()),
        Shape::Enum(variants) => variants.first().map(|v| format!("{name}::{v}")),
        Shape::Tuple(types) => {
            let mut parts = Vec::new();
            for ty in types {
                parts.push(
                    Box::pin(value_for(remote, root, &ty, depth, indent, seen, fallbacks)).await,
                );
            }
            Some(format!("{name}({})", parts.join(", ")))
        }
        Shape::Record(fields) => {
            if fields.is_empty() {
                return Some(format!("{name} {{}}"));
            }
            let mut lines = Vec::new();
            for (field, ty) in fields.iter().take(MANY_FIELDS) {
                let value = Box::pin(value_for(
                    remote,
                    root,
                    ty,
                    depth,
                    indent + 1,
                    seen,
                    fallbacks,
                ))
                .await;
                lines.push(format!("{pad}{field}: {value},"));
            }
            Some(format!("{name} {{\n{}\n{closing}}}", lines.join("\n")))
        }
    }
}

/// The source text of the declaration that `line` (1-based) belongs to, taken from the
/// document symbols and the file itself so nothing is elided.
fn declaration_at(symbols: &serde_json::Value, line: u32, text: &str) -> Option<String> {
    fn walk(nodes: &[serde_json::Value], line: u32, best: &mut Option<(u32, u32)>) {
        for node in nodes {
            let range = node
                .get("range")
                .or_else(|| node.get("location").and_then(|l| l.get("range")));
            if let Some(range) = range
                && let (Some(start), Some(end)) = (range.get("start"), range.get("end"))
                && let (Some(s), Some(e)) = (
                    start.get("line").and_then(|l| l.as_u64()),
                    end.get("line").and_then(|l| l.as_u64()),
                )
            {
                let (s, e) = (s as u32 + 1, e as u32 + 1);
                if s <= line && line <= e && best.is_none_or(|(bs, be)| e - s < be - bs) {
                    *best = Some((s, e));
                }
            }
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                walk(children, line, best);
            }
        }
    }
    let mut best = None;
    walk(symbols.as_array().map(|a| a.as_slice())?, line, &mut best);
    let (start, end) = best?;
    Some(
        text.lines()
            .skip(start.saturating_sub(1) as usize)
            .take((end.saturating_sub(start) + 1) as usize)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The declaration of a *type* called `name`.
///
/// The general symbol resolver refuses a name that resolves twice, and a type re-exported from
/// a crate root, or sharing its name with an enum variant of a message type, always does. Here
/// the kind is known: only a struct or an enum can be built, so the candidates are filtered by
/// kind first and the ambiguity usually disappears.
async fn resolve_type(
    remote: SocketAddr,
    root: &Path,
    name: &str,
    hint: Option<&Path>,
) -> Result<SymbolHit> {
    let hits = workspace_symbol_search(remote, root, name, hint, 32).await?;
    let mut types: Vec<SymbolHit> = hits
        .into_iter()
        .filter(|h| h.name == name && matches!(h.kind, "Struct" | "Enum" | "Class" | "Interface"))
        .collect();
    if let Some(hint) = hint {
        let in_hint: Vec<SymbolHit> = types.iter().filter(|h| h.path == hint).cloned().collect();
        if !in_hint.is_empty() {
            types = in_hint;
        }
    }
    // `pub use messages::SearchHit;` in a crate root is indexed as a second declaration of the
    // same type. When a candidate lives in a module of its own, the crate root is the re-export.
    let declares = |h: &SymbolHit| {
        !matches!(
            h.path.file_name().and_then(|n| n.to_str()),
            Some("lib.rs") | Some("mod.rs") | Some("main.rs")
        )
    };
    if types.iter().any(declares) {
        types.retain(declares);
    }
    match types.len() {
        0 => anyhow::bail!("no struct or enum named `{name}` in this workspace"),
        1 => Ok(types.remove(0)),
        _ => {
            types.sort_by_key(|h| h.path.to_string_lossy().len());
            let first = types.remove(0);
            if types.iter().any(|h| h.path != first.path) {
                let list = std::iter::once(&first)
                    .chain(types.iter())
                    .map(|h| {
                        format!(
                            "  [{}] {} — {}:{}:{}",
                            h.kind,
                            h.name,
                            h.path
                                .strip_prefix(root)
                                .unwrap_or(&h.path)
                                .to_string_lossy(),
                            h.line,
                            h.col
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                anyhow::bail!("`{name}` is declared in more than one file; pass `path`:\n{list}");
            }
            Ok(first)
        }
    }
}

/// The shape of a named type and the file that declares it.
async fn shape_of(
    remote: SocketAddr,
    root: &Path,
    name: &str,
    hint: Option<&Path>,
) -> Result<(Shape, String)> {
    let hit = resolve_type(remote, root, name, hint).await?;
    let uri = url::Url::from_file_path(&hit.path)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", hit.path))?
        .to_string();
    // Not hover: rust-analyzer elides a struct's fields past the tenth with `/* … */`, and a
    // fixture built from that is missing fields without saying so. The declaration's line
    // range from documentSymbol plus the file itself is exact.
    let symbols = crate::tools::execute_lsp_query(
        remote,
        root,
        &hit.path,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await?;
    let text = std::fs::read_to_string(&hit.path)
        .with_context(|| format!("cannot read {}", hit.path.display()))?;
    let decl = declaration_at(&symbols, hit.line, &text).with_context(|| {
        format!(
            "no declaration of `{name}` at {}:{}",
            hit.path.display(),
            hit.line
        )
    })?;
    let shape = parse_shape(&decl).with_context(|| format!("cannot read the shape of `{name}`"))?;
    let file = hit
        .path
        .strip_prefix(root)
        .unwrap_or(&hit.path)
        .to_string_lossy()
        .into_owned();
    Ok((shape, file))
}

/// Generates a fixture for `symbol`, and type-checks it when asked.
pub async fn generate(
    remote: SocketAddr,
    root: &Path,
    symbol: &str,
    depth: u32,
    verify: bool,
    hint: Option<&Path>,
) -> Result<Fixture> {
    let (shape, file) = shape_of(remote, root, symbol, hint).await?;
    let mut fallbacks = Vec::new();
    let mut seen = vec![symbol.to_string()];
    let value = match &shape {
        Shape::Unit => symbol.to_string(),
        _ => build_literal(
            remote,
            root,
            symbol,
            depth,
            0,
            &mut seen,
            &mut fallbacks,
            hint,
        )
        .await
        .with_context(|| format!("cannot build a value for `{symbol}`"))?,
    };
    let mut fixture = Fixture {
        type_name: symbol.to_string(),
        value,
        file: file.clone(),
        fallbacks,
        diagnostics: Vec::new(),
        verified: false,
    };
    if verify {
        let path = root.join(&file);
        let original = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        // The generated code is appended, so anything it breaks is reported at a line past the
        // original file. Diagnostics above that line belong to the file as it already is
        // (unexpanded derives, for instance) and are not ours to report.
        let first_generated_line = original.lines().count() as u32 + 1;
        let probe = format!(
            "{original}\n#[cfg(test)]\nmod prod_code_generated_fixture {{\n    #[allow(unused_imports)]\n    use super::*;\n\n    #[test]\n    fn builds() {{\n        let {} = {};\n        let _ = {};\n    }}\n}}\n",
            snake_case(symbol),
            fixture.value,
            snake_case(symbol)
        );
        let reports =
            crate::diagnostics::validate_texts(remote, root, &[(path.clone(), probe)], &[]).await?;
        fixture.verified = true;
        fixture.diagnostics = reports
            .iter()
            .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
            .filter(|(_, d)| d.severity == "error" && d.line >= first_generated_line)
            .map(|(file, d)| {
                format!(
                    "{}{} ({file}:{}:{})",
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
    }
    Ok(fixture)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_splits_humps() {
        assert_eq!(snake_case("SliceReport"), "slice_report");
        assert_eq!(snake_case("Config"), "config");
        assert_eq!(snake_case("HTTPClient"), "h_t_t_p_client");
    }

    #[test]
    fn declaration_is_taken_from_the_hover_block_that_declares_it() {
        let hover = "```rust\nprod_code_mcp::slice\n```\n\n```rust\npub struct SliceReport {\n    pub seed: String,\n}\n```\n\n---\n\ndocs";
        let decl = declaration_from_hover(hover).expect("a declaration");
        assert!(decl.starts_with("pub struct SliceReport"));
        assert!(declaration_from_hover("```rust\njust::a::path\n```").is_none());
    }

    #[test]
    fn shapes_are_read_from_declarations() {
        let record =
            parse_shape("pub struct A {\n    pub a: String,\n    b: HashMap<String, u64>,\n}")
                .unwrap();
        assert_eq!(
            record,
            Shape::Record(vec![
                ("a".into(), "String".into()),
                ("b".into(), "HashMap<String, u64>".into()),
            ])
        );
        assert_eq!(
            parse_shape("pub struct B(pub u32, String);").unwrap(),
            Shape::Tuple(vec!["u32".into(), "String".into()])
        );
        assert_eq!(parse_shape("struct C;").unwrap(), Shape::Unit);
        assert_eq!(
            parse_shape("pub enum D {\n    First,\n    Second(u8),\n}").unwrap(),
            Shape::Enum(vec!["First".into(), "Second".into()])
        );
    }

    #[test]
    fn a_doc_comment_between_fields_is_not_a_field() {
        let shape = parse_shape(
            "pub struct A {\n    /// how many\n    pub count: usize,\n    #[serde(default)]\n    pub name: String,\n}",
        )
        .unwrap();
        assert_eq!(
            shape,
            Shape::Record(vec![
                ("count".into(), "usize".into()),
                ("name".into(), "String".into())
            ])
        );
    }

    #[test]
    fn known_types_get_a_value_and_unknown_ones_do_not() {
        assert_eq!(known_value("bool").as_deref(), Some("false"));
        assert_eq!(known_value("usize").as_deref(), Some("0"));
        assert_eq!(known_value("String").as_deref(), Some("String::new()"));
        assert_eq!(known_value("&str").as_deref(), Some("\"\""));
        assert_eq!(known_value("&'a str").as_deref(), Some("\"\""));
        assert_eq!(known_value("&mut String").as_deref(), Some("String::new()"));
        // a caller may have stripped the reference already, leaving the lifetime
        assert_eq!(known_value("'static str").as_deref(), Some("\"\""));
        assert_eq!(known_value("Option<Whatever>").as_deref(), Some("None"));
        assert_eq!(known_value("Vec<Whatever>").as_deref(), Some("Vec::new()"));
        assert_eq!(
            known_value("std::collections::HashMap<String, u64>").as_deref(),
            Some("HashMap::new()")
        );
        assert_eq!(known_value("MyStruct"), None);
    }

    #[test]
    fn generic_arguments_survive_the_comma_split() {
        assert_eq!(
            split_top_level("a: HashMap<String, u64>, b: (u8, u8)")
                .into_iter()
                .map(|s| s.trim().to_string())
                .collect::<Vec<_>>(),
            vec!["a: HashMap<String, u64>", "b: (u8, u8)"]
        );
        assert_eq!(inner_of("Arc<Mutex<Engine>>", "Arc"), Some("Mutex<Engine>"));
        assert_eq!(inner_of("Vec<u8>", "Arc"), None);
    }

    #[test]
    fn a_wrapper_builds_its_inner_value() {
        assert_eq!(
            wrapper_value("Arc<String>", |inner| {
                assert_eq!(inner, "String");
                "String::new()".to_string()
            })
            .as_deref(),
            Some("std::sync::Arc::new(String::new())")
        );
        assert!(wrapper_value("Vec<String>", |_| String::new()).is_none());
    }

    #[test]
    fn the_report_says_whether_the_analyzer_accepted_it() {
        let mut f = Fixture {
            type_name: "Config".into(),
            value: "Config {\n    port: 0,\n}".into(),
            file: "src/config.rs".into(),
            fallbacks: vec!["Engine".into(), "Engine".into()],
            diagnostics: Vec::new(),
            verified: true,
        };
        let text = f.render();
        assert!(text.contains("let config = Config {"), "{text}");
        assert!(text.contains("stands in for: Engine ("), "{text}");
        assert!(text.contains("the analyzer accepts it: 0 errors"), "{text}");
        f.diagnostics = vec!["error: missing field `host`".into()];
        assert!(f.render().contains("the analyzer rejects it"));
        f.verified = false;
        assert!(f.render().contains("not verified"));
    }
}
