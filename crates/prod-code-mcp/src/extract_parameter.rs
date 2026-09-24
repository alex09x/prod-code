//! Promoting an expression inside a function into a parameter of it.
//!
//! The expression leaves the body and becomes the argument every existing call site passes, so
//! the behaviour of every current caller is unchanged and the next one can choose. What makes
//! this different from bundling parameters is where it can go wrong: the expression may name
//! something that exists inside the function and nowhere else, and the report says so rather
//! than writing a call site that cannot compile.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What the extraction did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtractedParameter {
    /// The function the parameter was added to.
    pub symbol: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub name: String,
    /// Empty when the parameter is written without one (JavaScript, or Python with no type).
    pub ty: String,
    /// The new parameter as the declaration now spells it: `name: T`, `name T` or `name`.
    #[serde(skip)]
    pub parameter: String,
    /// The expression that left the body, as it was written.
    pub expression: String,
    /// How many places in the body now read the parameter.
    pub replaced: usize,
    pub call_sites: usize,
    pub rewritten: Vec<(String, String)>,
    pub unmatched: Vec<String>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl ExtractedParameter {
    /// The report: what moved out of the body, and whether the result compiles.
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- new parameter: `{}`\n- from the body: `{}`\n- {} place(s) in the \
             body now read it, {} call site(s) pass it\n\n",
            self.symbol, self.file, self.parameter, self.expression, self.replaced, self.call_sites
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
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot given the argument ({} reference(s) that are not a call with this \
                 arity — a function pointer, a macro, or a call already changed):\n",
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
            // The expression travelled to places where its names may not exist. That is the
            // usual cause and it is not worth making the reader work it out.
            out.push_str(
                "\nthe expression is now written at every call site: if it names a local, a \
                 parameter or anything private to the function it came from, it cannot be \
                 spelled there. Extract something the callers can see.\n",
            );
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

/// The type in a hover answer, when it is one this can read.
///
/// rust-analyzer writes `let x: u32` for a binding and a bare path for a type, and neither
/// shape is reliable for an arbitrary expression — so a hover that does not parse is a reason
/// to ask the caller for the type rather than to guess at it.
pub fn type_from_hover(hover: &str) -> Option<String> {
    for line in hover.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("let ")
            && let Some((_, ty)) = rest.split_once(':')
        {
            let ty = ty.trim().trim_end_matches(&[',', ';'][..]).trim();
            if !ty.is_empty() {
                return Some(ty.to_string());
            }
        }
    }
    None
}

/// The language of the file an extraction happens in.
///
/// The steps are the same everywhere: find the function, add a parameter, read it in the body,
/// pass the expression at every call. What differs is how a declaration is found, how a
/// parameter is spelled, and how each language server names a type in a hover, so those are
/// the parts chosen by the file's language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
}

impl Syntax {
    /// The syntax of `path`, from the language it is opened with; `None` for a language this
    /// cannot extract a parameter in.
    pub fn of(path: &Path) -> Option<Self> {
        match crate::lang::language_id_for_path(path) {
            "rust" => Some(Self::Rust),
            "typescript" | "typescriptreact" => Some(Self::TypeScript),
            "javascript" | "javascriptreact" => Some(Self::JavaScript),
            "python" => Some(Self::Python),
            "go" => Some(Self::Go),
            _ => None,
        }
    }

    /// The new parameter as the declaration spells it, or `None` when this language needs a
    /// type and there is none. JavaScript has no annotations, and a Python parameter without
    /// one is still a parameter, so those two never need one.
    pub fn parameter(self, name: &str, ty: Option<&str>) -> Option<String> {
        match (self, ty) {
            (Self::JavaScript, _) | (Self::Python, None) => Some(name.to_string()),
            (Self::Go, Some(ty)) => Some(format!("{name} {ty}")),
            (_, Some(ty)) => Some(format!("{name}: {ty}")),
            (_, None) => None,
        }
    }

    /// The type in a hover answer from this language's server, when it is one this can read.
    pub fn type_from_hover(self, hover: &str) -> Option<String> {
        match self {
            Self::Rust => type_from_hover(hover),
            Self::JavaScript => None,
            Self::TypeScript => typescript_type(hover),
            Self::Python => python_type(hover),
            Self::Go => go_type(hover),
        }
    }

    /// The type of a literal expression. No language server answers a hover on `80` or `"x"`,
    /// yet a literal is the most common thing to extract, and its type is not in doubt.
    pub fn literal_type(self, expression: &str) -> Option<&'static str> {
        let e = expression.trim();
        let integer = !e.is_empty()
            && e.strip_prefix('-')
                .unwrap_or(e)
                .chars()
                .all(|c| c.is_ascii_digit() || c == '_')
            && e.chars().any(|c| c.is_ascii_digit());
        let float = !integer && e.contains('.') && e.replace('_', "").parse::<f64>().is_ok();
        let quoted = |q: char| e.len() >= 2 && e.starts_with(q) && e.ends_with(q);
        let string = quoted('"') || (self != Self::Go && quoted('\'')) || quoted('`');
        let boolean = match self {
            Self::Python => e == "True" || e == "False",
            _ => e == "true" || e == "false",
        };
        match self {
            Self::Rust | Self::JavaScript => None,
            Self::TypeScript if integer || float => Some("number"),
            Self::TypeScript if string => Some("string"),
            Self::TypeScript if boolean => Some("boolean"),
            Self::Python if integer => Some("int"),
            Self::Python if float => Some("float"),
            Self::Python if string => Some("str"),
            Self::Python if boolean => Some("bool"),
            Self::Go if integer => Some("int"),
            Self::Go if float => Some("float64"),
            Self::Go if string => Some("string"),
            Self::Go if quoted('\'') => Some("rune"),
            Self::Go if boolean => Some("bool"),
            _ => None,
        }
    }

    /// Whether the parameter list ends in one that takes whatever arguments are left: `...rest`
    /// in TypeScript and JavaScript, `...T` in Go, `*args`, a bare `*` or `**kwargs` in Python.
    /// A parameter added after it would not receive the argument every call site passes, so
    /// the callers would change behaviour, which is exactly what this refactoring promises not
    /// to do.
    fn catch_all(self, list: &str) -> Option<String> {
        let params = crate::signature::split_params(list);
        match self {
            Self::Rust => None,
            Self::TypeScript | Self::JavaScript => {
                params.last().filter(|p| p.starts_with("...")).cloned()
            }
            Self::Go => params.last().filter(|p| p.contains("...")).cloned(),
            Self::Python => params.into_iter().find(|p| p.starts_with('*')),
        }
    }

    /// Whether `line` imports a name rather than using it. The TypeScript server leaves imports
    /// out of `references`, basedpyright does not, and an import needs no argument.
    fn is_import(self, line: &str) -> bool {
        let line = line.trim_start();
        match self {
            Self::TypeScript | Self::JavaScript => {
                line.starts_with("import ")
                    || line.starts_with("import{")
                    || line.starts_with("export {")
                    || line.starts_with("export{")
            }
            Self::Python => line.starts_with("import ") || line.starts_with("from "),
            Self::Rust | Self::Go => false,
        }
    }
}

/// The type in a TypeScript hover for a binding: `const width: 80`, `let n: number`,
/// `(parameter) text: string`, `(property) Store.entries: number[]`. A literal type is widened,
/// because a parameter typed `80` would accept nothing else. A function or a method is not a
/// binding: its hover names what it returns, not what it is.
fn typescript_type(hover: &str) -> Option<String> {
    const BINDINGS: [&str; 6] = [
        "const ",
        "let ",
        "var ",
        "(parameter) ",
        "(property) ",
        "(variable) ",
    ];
    for line in hover.lines() {
        let line = line.trim();
        let Some(rest) = BINDINGS.iter().find_map(|p| line.strip_prefix(p)) else {
            continue;
        };
        let Some((name, ty)) = rest.split_once(':') else {
            continue;
        };
        let ty = ty.trim().trim_end_matches(';').trim();
        if name.contains('(') || ty.is_empty() {
            continue;
        }
        // An object type is written over several lines, and its first line is not a type.
        let opened = ty.matches(['{', '(', '[', '<']).count();
        let closed = ty.matches(['}', ')', ']']).count() + ty.matches('>').count()
            - ty.matches("=>").count();
        if opened != closed {
            return None;
        }
        return Some(
            Syntax::TypeScript
                .literal_type(ty)
                .map(str::to_string)
                .unwrap_or_else(|| ty.to_string()),
        );
    }
    None
}

/// The type in a basedpyright hover for a binding: `(variable) width: Literal[80]`,
/// `(constant) WIDTH: Literal[80]`, `(parameter) text: str`. `Literal[80]` is widened to `int`
/// for the reason TypeScript's `80` is; a type the checker made up, like `Self@Store` or one with
/// `Unknown` in it, cannot be written in an annotation and gives none.
fn python_type(hover: &str) -> Option<String> {
    const BINDINGS: [&str; 3] = ["(variable) ", "(constant) ", "(parameter) "];
    for line in hover.lines() {
        let line = line.trim();
        let Some(rest) = BINDINGS.iter().find_map(|p| line.strip_prefix(p)) else {
            continue;
        };
        let Some((_, ty)) = rest.split_once(':') else {
            continue;
        };
        let ty = ty.trim();
        if ty.is_empty() || ty.contains('@') || ty.contains("Unknown") {
            return None;
        }
        if let Some(values) = ty
            .strip_prefix("Literal[")
            .and_then(|v| v.strip_suffix(']'))
        {
            let kinds: std::collections::BTreeSet<&str> = values
                .split(',')
                .map(|v| Syntax::Python.literal_type(v).unwrap_or("?"))
                .collect();
            return match kinds.into_iter().collect::<Vec<_>>().as_slice() {
                [kind] if *kind != "?" => Some(kind.to_string()),
                _ => Some(ty.to_string()),
            };
        }
        return Some(ty.to_string());
    }
    None
}

/// The type in a gopls hover for a binding: `var width int`, `field entries []int`,
/// `const Base untyped int = 80`. An untyped constant takes its default type, which is what a
/// variable initialised from it would have.
fn go_type(hover: &str) -> Option<String> {
    const BINDINGS: [&str; 3] = ["var ", "field ", "const "];
    for line in hover.lines() {
        let line = line.trim();
        let Some(rest) = BINDINGS.iter().find_map(|p| line.strip_prefix(p)) else {
            continue;
        };
        let Some((_, ty)) = rest.split_once(' ') else {
            continue;
        };
        let ty = ty.split(" = ").next().unwrap_or("").trim();
        let ty = match ty.strip_prefix("untyped ") {
            Some("float") => "float64",
            Some("complex") => "complex128",
            Some(kind) => kind,
            None => ty,
        };
        if !ty.is_empty() {
            return Some(ty.to_string());
        }
    }
    None
}

/// The smallest *function* containing `line`, and its line span.
///
/// Not the smallest declaration: `textDocument/documentSymbol` reports local bindings too, so
/// the innermost thing containing an expression is usually the `let` it is part of. Only a
/// function or a method can take a parameter, so only those are candidates.
pub fn enclosing_function(symbols: &serde_json::Value, line: u32) -> Option<(String, u32, u32)> {
    enclosing_declaration(symbols, line).map(|d| (d.name, d.start, d.end))
}

/// A function or method as `textDocument/documentSymbol` reports it, 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enclosing {
    pub name: String,
    pub start: u32,
    pub end: u32,
    /// The column the range ends at on `end`.
    pub end_col: u32,
    /// Where the name is written (`selectionRange`), when the analyzer says.
    pub name_at: Option<(u32, u32)>,
}

/// [`enclosing_function`], with the name's position and the exact end of the range.
pub fn enclosing_declaration(symbols: &serde_json::Value, line: u32) -> Option<Enclosing> {
    /// LSP `SymbolKind`: a free function, and a method on a type.
    const FUNCTION: u64 = 12;
    const METHOD: u64 = 6;

    fn walk(nodes: &[serde_json::Value], line: u32, best: &mut Option<Enclosing>) {
        for node in nodes {
            let range = node
                .get("range")
                .or_else(|| node.get("location").and_then(|l| l.get("range")));
            let kind = node.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
            if let Some(range) = range
                && (kind == FUNCTION || kind == METHOD)
                && let (Some(s), Some(e)) = (
                    range.pointer("/start/line").and_then(|l| l.as_u64()),
                    range.pointer("/end/line").and_then(|l| l.as_u64()),
                )
            {
                let (s, e) = (s as u32 + 1, e as u32 + 1);
                if s <= line && line <= e && best.as_ref().is_none_or(|b| e - s < b.end - b.start) {
                    let at = |pointer: &str| {
                        node.pointer(pointer)
                            .and_then(|v| v.as_u64())
                            .map(|v| v as u32 + 1)
                    };
                    *best = Some(Enclosing {
                        name: node
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        start: s,
                        end: e,
                        end_col: range
                            .pointer("/end/character")
                            .and_then(|c| c.as_u64())
                            .map_or(1, |c| c as u32 + 1),
                        name_at: at("/selectionRange/start/line")
                            .zip(at("/selectionRange/start/character")),
                    });
                }
            }
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                walk(children, line, best);
            }
        }
    }
    let mut best = None;
    walk(symbols.as_array().map(|a| a.as_slice())?, line, &mut best);
    best
}

/// The offset of a function's name in a language whose declarations do not start with `fn`.
///
/// The analyzer's `selectionRange` is the name itself, and it is trusted when the text there
/// says so; gopls calls a method `(*Store).Limit` but selects only `Limit`, which is why `bare`
/// is the name after the last dot. Without it, the first whole-word occurrence of the name
/// from the declaration's first line on that is followed by a parameter list.
fn name_offset(text: &str, bare: &str, start: u32, name_at: Option<(u32, u32)>) -> Option<usize> {
    if let Some((line, col)) = name_at
        && let Some(at) = crate::signature::offset_of(text, line, col)
        && text[at..].starts_with(bare)
    {
        return Some(at);
    }
    let from = crate::signature::offset_of(text, start, 1)?;
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    text[from..]
        .match_indices(bare)
        .map(|(i, _)| from + i)
        .find(|&at| {
            let before = text[..at].chars().next_back();
            let after = text[at + bare.len()..].trim_start().chars().next();
            !before.is_some_and(is_word) && matches!(after, Some('(' | '<' | '['))
        })
}

/// The span between the parentheses of the parameter list that follows a function's name, in
/// a language other than Rust. Type parameters come first and are skipped: `<T>` in TypeScript,
/// `[T any]` in Go and `[T]` in Python.
fn parameter_list(text: &str, name_end: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = name_end;
    loop {
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        match bytes.get(i)? {
            b'(' => {
                let close = crate::parameter_object::matching_bracket(text, i)?;
                return Some((i + 1, close));
            }
            b'[' => i = crate::parameter_object::matching_bracket(text, i)? + 1,
            b'<' => {
                // `=>` in a bound such as `<F extends () => void>` does not close anything.
                let mut depth = 0i32;
                loop {
                    match bytes.get(i)? {
                        b'<' => depth += 1,
                        b'>' if i == 0 || bytes[i - 1] != b'=' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            _ => return None,
        }
    }
}

/// The parameter list with `param` added at the end.
pub fn with_parameter(list: &str, param: &str) -> String {
    let trimmed = list.trim();
    if trimmed.is_empty() {
        return param.to_string();
    }
    // A trailing comma means the list is written one per line; keep that shape, and keep
    // whatever whitespace sits between the last parameter and the closing parenthesis.
    if trimmed.ends_with(',') {
        let head = list.trim_end_matches(|c: char| c.is_whitespace());
        let tail = &list[head.len()..];
        let indent: String = head
            .lines()
            .next_back()
            .unwrap_or("")
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        return format!("{head}\n{indent}{param},{tail}");
    }
    format!("{trimmed}, {param}")
}

/// The argument list with `argument` added at the end.
pub fn with_argument(args: &str, argument: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return argument.to_string();
    }
    format!("{trimmed}, {argument}")
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// The type a hover gives for the selection `from..to`, outside Rust.
///
/// A hover describes one token. When the selection is longer than the token the hover
/// covers, the type is the token's, not the expression's (in `name + 1`, `name` may be a string
/// and the sum a number), so it is taken only when the two spans are the same.
async fn hover_type(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    from: usize,
    to: usize,
    syntax: Syntax,
) -> Option<String> {
    let selected = &text[from..to];
    let start = from + (selected.len() - selected.trim_start().len());
    let end = from + selected.trim_end().len();
    let (line, col) = crate::signature::line_col_at(text, start);
    let hover = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": url::Url::from_file_path(file).ok()?.to_string() },
            "position": { "line": line - 1, "character": col - 1 },
        }),
    )
    .await
    .ok()?;
    if let Some(range) = hover.get("range") {
        let at = |pointer: &str| {
            range
                .pointer(pointer)
                .and_then(|v| v.as_u64())
                .map(|v| v as u32 + 1)
        };
        let covered = (
            at("/start/line").zip(at("/start/character")),
            at("/end/line").zip(at("/end/character")),
        );
        if covered
            != (
                Some((line, col)),
                Some(crate::signature::line_col_at(text, end)),
            )
        {
            return None;
        }
    }
    syntax.type_from_hover(hover.pointer("/contents/value")?.as_str()?)
}

/// Promotes the expression selected in `file` into a parameter of the function that contains it.
#[allow(clippy::too_many_arguments)]
pub async fn extract(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    start: (u32, u32),
    end: (u32, u32),
    name: &str,
    ty: Option<&str>,
    replace_all: bool,
    apply: bool,
    force: bool,
) -> Result<ExtractedParameter> {
    anyhow::ensure!(
        !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'),
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
    let syntax = Syntax::of(file).with_context(|| {
        format!(
            "{} is not in a language this can extract a parameter in (Rust, TypeScript, \
             JavaScript, Python, Go)",
            display(root, file)
        )
    })?;

    // The function the selection is inside, and its parameter list.
    let symbols = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": url::Url::from_file_path(file)
            .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?.to_string() } }),
    )
    .await?;
    let declaration = enclosing_declaration(&symbols, start.0)
        .context("the selection is not inside a function")?;
    let (callee, fn_start, fn_end) = (declaration.name, declaration.start, declaration.end);
    // What a call site spells: gopls names a method `(*Store).Limit`, its callers write `Limit`.
    let bare = callee.rsplit('.').next().unwrap_or(&callee).to_string();
    let (fn_offset, open, close) = if syntax == Syntax::Rust {
        let fn_offset = {
            let lines: Vec<&str> = text.lines().collect();
            let head = lines
                .get(fn_start as usize - 1)
                .context("the declaration's first line is not in the file")?;
            let at = head
                .find(&format!("fn {callee}"))
                .map(|i| i + 3)
                .with_context(|| format!("`{callee}` is not a function"))?;
            crate::signature::offset_of(&text, fn_start, at as u32 + 1)
                .context("the declaration is not where the analyzer put it")?
        };
        let (_, open, close) = crate::signature::param_span(&text, fn_offset)
            .with_context(|| format!("`{callee}` has no parameter list"))?;
        (fn_offset, open, close)
    } else {
        let fn_offset = name_offset(&text, &bare, fn_start, declaration.name_at)
            .with_context(|| format!("`{callee}` is not declared where the analyzer put it"))?;
        let (open, close) = parameter_list(&text, fn_offset + bare.len())
            .with_context(|| format!("`{callee}` has no parameter list"))?;
        (fn_offset, open, close)
    };
    anyhow::ensure!(
        from > close,
        "the selection is in the signature, not in the body"
    );
    if let Some(rest) = syntax.catch_all(&text[open..close]) {
        anyhow::bail!(
            "`{callee}` takes `{rest}`, which collects whatever arguments are left: a parameter \
             after it would not receive the one every call site passes, so the callers would \
             change behaviour"
        );
    }

    // The type: the caller's, the literal's, or the one hover gives when it gives a shape this
    // can read. JavaScript has no annotations, so it needs none of them.
    let ty = match ty {
        _ if syntax == Syntax::JavaScript => String::new(),
        Some(ty) => ty.to_string(),
        None if syntax != Syntax::Rust => match syntax.literal_type(&expression) {
            Some(ty) => ty.to_string(),
            None => hover_type(remote, root, file, &text, from, to, syntax)
                .await
                .unwrap_or_default(),
        },
        None => {
            let hover = crate::tools::execute_lsp_query(
                remote,
                root,
                file,
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": url::Url::from_file_path(file)
                        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?.to_string() },
                    "position": { "line": start.0.saturating_sub(1), "character": start.1.saturating_sub(1) },
                }),
            )
            .await
            .ok()
            .and_then(|h| {
                h.pointer("/contents/value")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
            type_from_hover(&hover).context(
                "the analyzer does not give a type for this selection in a shape this can read; \
                 pass the type explicitly",
            )?
        }
    };
    let parameter = syntax
        .parameter(name, Some(ty.as_str()).filter(|t| !t.is_empty()))
        .context(
            "the analyzer does not give a type for this selection in a shape this can read; \
             pass the type explicitly",
        )?;

    // Every edit against the file as it is, applied from the last offset backwards.
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    // A Rust function ends on a line of its own, `}`. A Python one ends with its last
    // statement, which is still body, so elsewhere the range ends exactly where the analyzer
    // says.
    let body_end = if syntax == Syntax::Rust {
        crate::signature::offset_of(&text, fn_end, 1)
    } else {
        crate::signature::offset_of(&text, fn_end, declaration.end_col)
    };
    let body_range = close..body_end.unwrap_or(text.len());
    let mut replaced = 0usize;
    if replace_all {
        let mut at = body_range.start;
        while let Some(i) = text[at..body_range.end.min(text.len())].find(&expression) {
            let hit = at + i;
            edits.entry(file.to_path_buf()).or_default().push((
                hit,
                expression.len(),
                name.to_string(),
            ));
            replaced += 1;
            at = hit + expression.len();
        }
    } else {
        edits
            .entry(file.to_path_buf())
            .or_default()
            .push((from, to - from, name.to_string()));
        replaced = 1;
    }
    edits.entry(file.to_path_buf()).or_default().push((
        open,
        close - open,
        with_parameter(&text[open..close], &parameter),
    ));

    // Every call site passes what the body used to say.
    let (fn_line, fn_col) = crate::signature::line_col_at(&text, fn_offset);
    let mut unmatched = Vec::new();
    let mut call_sites = 0usize;
    for (path, rl, rc) in crate::signature::references(remote, root, file, fn_line, fn_col)
        .await
        .unwrap_or_default()
    {
        let body = if path == *file {
            text.clone()
        } else {
            std::fs::read_to_string(&path).unwrap_or_default()
        };
        let Some(at) = crate::signature::offset_of(&body, rl, rc) else {
            continue;
        };
        // The declaration's own name is not a call, whatever the answer includes, and an import
        // names the function without calling it.
        let line_start = body[..at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = body[at..].find('\n').map_or(body.len(), |i| at + i);
        if (path == *file && at == fn_offset) || syntax.is_import(&body[line_start..line_end]) {
            continue;
        }
        // The analyzer's position is trusted only when the name is actually there. If the file
        // changed since it was analysed, the position points at something else, and appending
        // an argument to whatever call follows it is the one mistake this must never make (#75).
        if !body[at..].starts_with(bare.as_str()) {
            unmatched.push(format!(
                "{}:{rl}:{rc} (the analyzer places `{callee}` here, but the file says otherwise)",
                display(root, &path)
            ));
            continue;
        }
        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(&body, at + bare.len())
        else {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        };
        edits.entry(path).or_default().push((
            args_start,
            args_end - args_start,
            with_argument(&body[args_start..args_end], &expression),
        ));
        call_sites += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = if path == *file {
            text.clone()
        } else {
            std::fs::read_to_string(&path).unwrap_or_default()
        };
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
            "the change does not compile ({} error(s)); nothing was written. Extract something \
             the callers can see, or pass `force: true`:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(ExtractedParameter {
        symbol: callee,
        root: root.to_path_buf(),
        file: display(root, file),
        name: name.to_string(),
        ty,
        parameter,
        expression,
        replaced,
        call_sites,
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
    fn the_enclosing_function_is_not_the_let_the_expression_sits_in() {
        // What the analyzer really answers: the function, and the local inside it.
        let symbols = serde_json::json!([
            { "name": "move_item", "kind": 12,
              "range": { "start": { "line": 577 }, "end": { "line": 700 } },
              "children": [
                { "name": "removed", "kind": 13,
                  "range": { "start": { "line": 628 }, "end": { "line": 628 } } }
              ] }
        ]);
        assert_eq!(
            enclosing_function(&symbols, 629),
            Some(("move_item".to_string(), 578, 701)),
            "a local binding cannot take a parameter, so it is not a candidate"
        );
        assert_eq!(enclosing_function(&symbols, 900), None);
    }

    #[test]
    fn a_hover_that_names_a_binding_gives_its_type_and_anything_else_gives_none() {
        assert_eq!(
            type_from_hover("```rust\nlet decl_end: u32\n```").as_deref(),
            Some("u32")
        );
        assert_eq!(
            type_from_hover("```rust\nlet name: BTreeMap<String, u8>\n```").as_deref(),
            Some("BTreeMap<String, u8>")
        );
        assert_eq!(type_from_hover("```rust\ncore::str\n```"), None);
        assert_eq!(type_from_hover(""), None);
    }

    #[test]
    fn a_parameter_is_added_at_the_end_and_keeps_the_lists_shape() {
        assert_eq!(with_parameter("", "limit: usize"), "limit: usize");
        assert_eq!(
            with_parameter("a: u8, b: u8", "limit: usize"),
            "a: u8, b: u8, limit: usize"
        );
        // One per line, trailing comma: the new one keeps that shape and the indentation.
        assert_eq!(
            with_parameter("\n    a: u8,\n    b: u8,\n", "limit: usize"),
            "\n    a: u8,\n    b: u8,\n    limit: usize,\n"
        );
    }

    fn report(
        unmatched: Vec<String>,
        diagnostics: Vec<String>,
        applied: bool,
    ) -> ExtractedParameter {
        ExtractedParameter {
            symbol: "render".into(),
            root: PathBuf::from("/root"),
            file: "src/lib.rs".into(),
            name: "width_limit".into(),
            ty: "usize".into(),
            parameter: "width_limit: usize".into(),
            expression: "80".into(),
            replaced: 1,
            call_sites: 2,
            rewritten: vec![("/root/src/lib.rs".into(), "pub fn render() {}\n".into())],
            unmatched,
            diagnostics,
            applied,
        }
    }

    #[test]
    fn the_report_says_what_was_left_out_and_what_the_analyzer_thought() {
        let clean = report(Vec::new(), Vec::new(), false).render(4000);
        assert!(
            clean.contains("new parameter: `width_limit: usize`"),
            "{clean}"
        );
        assert!(clean.contains("2 call site(s) pass it"), "{clean}");
        assert!(
            clean.contains("the analyzer accepts the result: 0 errors"),
            "{clean}"
        );
        assert!(clean.contains("nothing was written"), "{clean}");

        let missed = report(vec!["src/other.rs:9:5".into()], Vec::new(), false).render(4000);
        assert!(
            missed.contains("not given the argument (1 reference"),
            "{missed}"
        );
        assert!(missed.contains("src/other.rs:9:5"), "{missed}");

        // A rejected result explains the usual cause rather than leaving a raw diagnostic.
        let broken = report(
            Vec::new(),
            vec!["cannot find value `n` [E0425] (src/lib.rs:8:17)".into()],
            false,
        )
        .render(4000);
        assert!(
            broken.contains("the analyzer rejects the result"),
            "{broken}"
        );
        assert!(broken.contains("if it names a local"), "{broken}");

        let written = report(Vec::new(), Vec::new(), true).render(4000);
        assert!(written.contains("[applied to 1 file(s)]"), "{written}");
        assert!(!written.contains("nothing was written"), "{written}");
    }

    #[test]
    fn a_diff_longer_than_the_budget_is_cut_and_says_so() {
        let cut = report(Vec::new(), Vec::new(), false).render(10);
        assert!(cut.contains("… diff truncated"), "{cut}");
    }

    #[test]
    fn each_languages_hover_for_a_binding_gives_a_type_that_can_be_written() {
        // What the TypeScript server, basedpyright and gopls answer, verbatim.
        let ts = Syntax::TypeScript;
        assert_eq!(
            ts.type_from_hover("```typescript\nconst cap: number\n```\n")
                .as_deref(),
            Some("number")
        );
        assert_eq!(
            ts.type_from_hover("```typescript\nconst width: 80\n```\n")
                .as_deref(),
            Some("number"),
            "a literal type is widened"
        );
        assert_eq!(
            ts.type_from_hover("```typescript\n(property) Store.entries: number[]\n```\n")
                .as_deref(),
            Some("number[]")
        );
        assert_eq!(
            ts.type_from_hover("```typescript\n(method) Store.limit(): number\n```\n"),
            None,
            "a method's hover names what it returns, not what it is"
        );
        assert_eq!(
            ts.type_from_hover("```typescript\nconst o: {\n    a: number;\n}\n```\n"),
            None
        );

        let py = Syntax::Python;
        assert_eq!(
            py.type_from_hover("```python\n(variable) width: Literal[80]\n```")
                .as_deref(),
            Some("int")
        );
        assert_eq!(
            py.type_from_hover("```python\n(parameter) text: str\n```")
                .as_deref(),
            Some("str")
        );
        assert_eq!(
            py.type_from_hover("```python\n(parameter) self: Self@Store\n```"),
            None
        );
        assert_eq!(
            py.type_from_hover(
                "```python\n(function) def len(\n    obj: Sized,\n    /\n) -> int\n```"
            ),
            None
        );

        let go = Syntax::Go;
        assert_eq!(
            go.type_from_hover("```go\nvar width int\n```").as_deref(),
            Some("int")
        );
        assert_eq!(
            go.type_from_hover("```go\nfield entries []int\n```")
                .as_deref(),
            Some("[]int")
        );
        assert_eq!(
            go.type_from_hover("```go\nconst Base untyped int = 80\n```\n\n---\n\n[`shop.Base` on pkg.go.dev](https://pkg.go.dev/example.com/xp/shop#Base)")
                .as_deref(),
            Some("int")
        );
        assert_eq!(go.type_from_hover("```go\nfunc len(v Type) int\n```"), None);
        assert_eq!(Syntax::JavaScript.type_from_hover("const a: number"), None);
    }

    #[test]
    fn a_literal_has_its_type_in_every_language_but_rust() {
        assert_eq!(Syntax::TypeScript.literal_type("80"), Some("number"));
        assert_eq!(Syntax::TypeScript.literal_type("'x'"), Some("string"));
        assert_eq!(Syntax::Python.literal_type("1.5"), Some("float"));
        assert_eq!(Syntax::Python.literal_type("True"), Some("bool"));
        assert_eq!(Syntax::Go.literal_type("64"), Some("int"));
        assert_eq!(Syntax::Go.literal_type("'x'"), Some("rune"));
        assert_eq!(Syntax::Go.literal_type("\"x\""), Some("string"));
        assert_eq!(Syntax::Go.literal_type("64 * 1024"), None);
        assert_eq!(
            Syntax::Rust.literal_type("80"),
            None,
            "Rust has several integer types"
        );
    }

    #[test]
    fn each_language_spells_the_parameter_its_own_way() {
        assert_eq!(
            Syntax::Rust.parameter("n", Some("usize")).as_deref(),
            Some("n: usize")
        );
        assert_eq!(
            Syntax::TypeScript.parameter("n", Some("number")).as_deref(),
            Some("n: number")
        );
        assert_eq!(Syntax::TypeScript.parameter("n", None), None);
        assert_eq!(
            Syntax::JavaScript.parameter("n", Some("number")).as_deref(),
            Some("n")
        );
        assert_eq!(Syntax::Python.parameter("n", None).as_deref(), Some("n"));
        assert_eq!(
            Syntax::Python.parameter("n", Some("int")).as_deref(),
            Some("n: int")
        );
        assert_eq!(
            Syntax::Go.parameter("n", Some("int")).as_deref(),
            Some("n int")
        );
        assert_eq!(Syntax::Go.parameter("n", None), None);
        assert_eq!(Syntax::of(Path::new("a/b.tsx")), Some(Syntax::TypeScript));
        assert_eq!(Syntax::of(Path::new("a/b.mjs")), Some(Syntax::JavaScript));
        assert_eq!(Syntax::of(Path::new("a/b.swift")), None);
    }

    #[test]
    fn the_parameter_list_is_found_after_type_parameters_and_a_go_receiver() {
        let go = "func (s *Store) Limit[T any](n T) int {\n";
        let at = name_offset(go, "Limit", 1, None).expect("the name");
        assert_eq!(&go[at..at + 5], "Limit");
        let (open, close) = parameter_list(go, at + 5).expect("the list");
        assert_eq!(&go[open..close], "n T");

        let ts = "export function pick<F extends () => void>(f: F): F {\n";
        let at = name_offset(ts, "pick", 1, Some((1, 17))).expect("the name");
        let (open, close) = parameter_list(ts, at + 4).expect("the list");
        assert_eq!(&ts[open..close], "f: F");

        // A selection range that does not hold the name is not trusted.
        let py = "def limit(self) -> int:\n";
        assert_eq!(name_offset(py, "limit", 1, Some((1, 1))), Some(4));
    }

    #[test]
    fn a_parameter_after_one_that_collects_the_rest_is_refused() {
        assert_eq!(
            Syntax::TypeScript
                .catch_all("a: number, ...rest: string[]")
                .as_deref(),
            Some("...rest: string[]")
        );
        assert_eq!(
            Syntax::Go.catch_all("a int, xs ...int").as_deref(),
            Some("xs ...int")
        );
        assert_eq!(
            Syntax::Python.catch_all("self, *, key: int").as_deref(),
            Some("*")
        );
        assert_eq!(Syntax::Python.catch_all("self, a: int = 3"), None);
        assert_eq!(Syntax::Rust.catch_all("a: u8"), None);
    }

    #[test]
    fn an_argument_is_added_at_the_end_of_whatever_was_there() {
        assert_eq!(with_argument("", "64"), "64");
        assert_eq!(with_argument("a, b", "64"), "a, b, 64");
        assert_eq!(with_argument("  a  ", "64"), "a, 64");
    }
}
