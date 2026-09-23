//! Rename a schema field across the languages that use it (roadmap 7.6).
//!
//! A field in a `.proto`, a SQL table or a JSON payload is spelled differently in every
//! language that touches it: `order_id` in the schema and in Rust, `OrderID` in Go with
//! `json:"order_id"` next to it, `orderId` in TypeScript. Renaming it means finding all of
//! those spellings and changing each one *the way its own language wants*, which is why a
//! find-and-replace is wrong and a single semantic rename is not enough: no analyzer knows
//! that the Go field and the TypeScript property are the same thing.
//!
//! So this does both, and keeps them apart. Every spelling is found textually — that is
//! discovery, not editing. Then each identifier is renamed by the analyzer of its own
//! sub-project (`textDocument/rename` through the session that project's engine answers on),
//! which is what makes the Go rewrite follow `OrderID` into every file that reads it. Only
//! what no analyzer owns — the schema files, and the field's name inside string literals such
//! as a `json:` tag or an SQL query — is edited textually, at positions that were found, never
//! by replacing a word everywhere. Anything left over is listed rather than guessed at.
//!
//! OpenAPI documents and GraphQL schemas are read for their structure, not as plain text. In
//! OpenAPI the field is a key (`order_id:` under `properties`) or a whole value
//! (`required: [order_id]`, `name: order_id`); a description that mentions it is prose. In
//! GraphQL it is a name outside comments and description strings. Only the field is rewritten,
//! and every mention is listed.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// One spelling of the field, and what it becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub from: String,
    pub to: String,
    pub style: &'static str,
}

/// Where a spelling appears. Positions are 1-based, `col` and `len` counted in characters.
#[derive(Debug, Clone)]
struct Occurrence {
    file: PathBuf,
    line: u32,
    col: u32,
    len: usize,
    variant: usize,
    in_string: bool,
}

/// What the rename did, or would do.
#[derive(Debug)]
pub struct SchemaRename {
    pub field: String,
    pub to: String,
    pub root: PathBuf,
    /// Every file this changes, as (path, whole new content).
    pub rewritten: Vec<(PathBuf, String)>,
    /// One line per language: how many occurrences, and how they were handled.
    pub summary: Vec<String>,
    /// Occurrences nothing rewrote: a comment, a language with no engine here, a rename the
    /// analyzer refused.
    pub left: Vec<String>,
    /// Errors the analyzers report for the changed files, checked per project.
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl SchemaRename {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!("`{}` → `{}`\n\n", self.field, self.to);
        for line in &self.summary {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
        let mut body = String::new();
        let mut changed = 0usize;
        for (path, new_text) in &self.rewritten {
            let old = crate::refactor::text_before_apply(Path::new(path));
            let rel = display(&self.root, path);
            let diff = similar::TextDiff::from_lines(&old, new_text);
            changed += diff
                .iter_all_changes()
                .filter(|c| c.tag() != similar::ChangeTag::Equal)
                .count();
            body.push_str(
                &diff
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        out.push_str(&format!(
            "{} changed line(s) in {} file(s)\n\n",
            changed,
            self.rewritten.len()
        ));
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }
        if !self.left.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({}), because no analyzer owns them and they are not string \
                 literals — usually comments and documentation:\n",
                self.left.len()
            ));
            for l in &self.left {
                out.push_str(&format!("  {l}\n"));
            }
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzers accept the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzers reject the result:\n");
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

/// Words of a name, whatever style it is written in: `order_id`, `orderId`, `OrderID`,
/// `ORDER_ID` and `order-id` all give `["order", "id"]`.
fn words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (i, c) in chars.iter().copied().enumerate() {
        if c == '_' || c == '-' || c == '.' || c == ' ' {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        // A capital starts a word, unless it is inside a run of capitals that is not ending
        // (`OrderID` is order + id, `IDOrder` is id + order).
        let prev_lower = i > 0 && chars[i - 1].is_lowercase();
        let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        if c.is_uppercase() && !current.is_empty() && (prev_lower || next_lower) {
            out.push(std::mem::take(&mut current));
        }
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Words Go capitalises whole, by convention.
const INITIALISMS: &[&str] = &[
    "id", "url", "uri", "api", "http", "https", "json", "xml", "sql", "db", "uuid", "ip", "tcp",
    "udp", "ttl", "cpu", "ram", "os", "io", "eof",
];

fn snake(w: &[String]) -> String {
    w.join("_")
}

fn kebab(w: &[String]) -> String {
    w.join("-")
}

fn screaming(w: &[String]) -> String {
    w.iter()
        .map(|s| s.to_uppercase())
        .collect::<Vec<_>>()
        .join("_")
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn camel(w: &[String]) -> String {
    w.iter()
        .enumerate()
        .map(|(i, s)| if i == 0 { s.clone() } else { capitalize(s) })
        .collect()
}

fn pascal(w: &[String]) -> String {
    w.iter().map(|s| capitalize(s)).collect()
}

/// Go's spelling: the same as Pascal case, except that an initialism is written whole.
fn pascal_go(w: &[String]) -> String {
    w.iter()
        .map(|s| {
            if INITIALISMS.contains(&s.as_str()) {
                s.to_uppercase()
            } else {
                capitalize(s)
            }
        })
        .collect()
}

/// A naming style: what it is called, and how it writes a name's words.
type Style = (&'static str, fn(&[String]) -> String);

/// Every spelling of `field`, paired with the same spelling of `to`.
pub fn variants(field: &str, to: &str) -> Vec<Variant> {
    let (f, t) = (words(field), words(to));
    let styles: [Style; 6] = [
        ("snake_case", snake),
        ("camelCase", camel),
        ("PascalCase", pascal),
        ("Go PascalCase", pascal_go),
        ("SCREAMING_CASE", screaming),
        ("kebab-case", kebab),
    ];
    let mut out: Vec<Variant> = Vec::new();
    for (style, render) in styles {
        let from = render(&f);
        let to = render(&t);
        if from.is_empty() || out.iter().any(|v| v.from == from) {
            continue;
        }
        out.push(Variant { from, to, style });
    }
    out
}

/// What kind of file this is, which decides who is allowed to edit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A language with an engine on the gateway: identifiers are renamed by the analyzer.
    Code(&'static str),
    /// A schema, a query or a document: the text is all there is.
    Text(&'static str),
    Skip,
}

fn kind_of(path: &Path) -> Kind {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rs" => Kind::Code("rust"),
        "go" => Kind::Code("go"),
        "ts" | "tsx" | "mts" | "cts" => Kind::Code("typescript"),
        "js" | "jsx" | "mjs" | "cjs" => Kind::Code("javascript"),
        "py" | "pyi" => Kind::Code("python"),
        "swift" => Kind::Code("swift"),
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hxx" => Kind::Code("c/c++"),
        "proto" => Kind::Text("protobuf"),
        "sql" => Kind::Text("sql"),
        "graphql" | "gql" => Kind::Text("graphql"),
        "json" => Kind::Text("json"),
        "yaml" | "yml" => Kind::Text("yaml"),
        "toml" => Kind::Text("toml"),
        "md" => Kind::Text("markdown"),
        "sh" | "bash" | "fish" => Kind::Text("shell"),
        "txt" | "csv" | "env" => Kind::Text("text"),
        _ => Kind::Skip,
    }
}

/// A schema format whose structure decides which spellings are the field and which are prose
/// that mentions it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Schema {
    OpenApi,
    GraphQl,
}

/// Which schema format a file is, when it is one. An OpenAPI document is YAML or JSON with an
/// `openapi` (or Swagger's `swagger`) key: at the start of a line in YAML, anywhere in JSON.
fn schema_of(path: &Path, text: &str) -> Option<Schema> {
    let keyed = |line: &str, key: &str| {
        line.strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
    };
    match kind_of(path) {
        Kind::Text("graphql") => Some(Schema::GraphQl),
        Kind::Text("yaml") => text
            .lines()
            .any(|l| keyed(l, "openapi") || keyed(l, "swagger"))
            .then_some(Schema::OpenApi),
        Kind::Text("json") => text
            .lines()
            .map(|l| l.trim_start_matches(|c: char| c == '{' || c.is_whitespace()))
            .any(|l| keyed(l, "\"openapi\"") || keyed(l, "\"swagger\""))
            .then_some(Schema::OpenApi),
        _ => None,
    }
}

/// What a file's occurrences are counted under: its schema format, or its language.
fn label(path: &Path, text: &str) -> Option<&'static str> {
    match (schema_of(path, text), kind_of(path)) {
        (Some(Schema::OpenApi), _) => Some("openapi"),
        (Some(Schema::GraphQl), _) => Some("graphql"),
        (None, Kind::Code(l) | Kind::Text(l)) => Some(l),
        (None, Kind::Skip) => None,
    }
}

/// Is this occurrence the field itself, rather than a mention of it in prose?
///
/// In OpenAPI the field is a whole key or a whole scalar: what comes before it (past a quote
/// and spaces) opens a key or a value (`:`, `-`, `[`, `,`, `{`, or nothing), and what comes after
/// closes one (`:`, `,`, `]`, `}`, a comment, or nothing). `order_id:`, `- order_id`,
/// `required: [id, order_id]` and `"order_id": {` qualify; `description: The order_id of…` does
/// not. In GraphQL it is any name outside a `#` comment and a string or `"""` description.
fn is_structural(schema: Schema, text: &str, o: &Occurrence) -> bool {
    let line = text.lines().nth(o.line as usize - 1).unwrap_or("");
    let chars: Vec<char> = line.chars().collect();
    let start = (o.col as usize - 1).min(chars.len());
    let end = (start + o.len).min(chars.len());
    let before: String = chars[..start].iter().collect();
    let after: String = chars[end..].iter().collect();
    match schema {
        Schema::OpenApi => {
            let mut before = before.trim_end();
            let mut after = after.as_str();
            if let (Some(q @ ('"' | '\'')), Some(c)) = (before.chars().last(), after.chars().next())
                && c == q
            {
                before = &before[..before.len() - 1];
                after = &after[1..];
            }
            let before = before.trim();
            let after = after.trim_start();
            let opens = before.is_empty() || before.ends_with([':', '-', '[', ',', '{']);
            let closes = after.is_empty() || after.starts_with([':', ',', ']', '}', '#']);
            opens && closes
        }
        Schema::GraphQl => !o.in_string && !before.contains('#') && !in_block_string(text, o),
    }
}

/// Is this occurrence inside a GraphQL block string (`"""…"""`, a description)?
fn in_block_string(text: &str, o: &Occurrence) -> bool {
    let mut quotes = 0;
    for (n, line) in text.lines().enumerate() {
        if n + 1 == o.line as usize {
            let prefix: String = line.chars().take(o.col as usize - 1).collect();
            quotes += prefix.matches("\"\"\"").count();
            break;
        }
        quotes += line.matches("\"\"\"").count();
    }
    quotes % 2 == 1
}

/// Directories that never hold sources worth rewriting.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    "vendor",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".svelte-kit",
    ".build",
    ".prod",
    "Pods",
];

/// Every file worth scanning under `root`.
fn walk(root: &Path, max_bytes: u64) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
            } else if meta.is_file()
                && meta.len() <= max_bytes
                && !matches!(kind_of(&path), Kind::Skip)
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Is the character at `col` (1-based, characters) inside a quoted string on this line?
///
/// Counting quotes on one line is not a parser, and it does not have to be: what it decides is
/// whether an occurrence may be edited as text, and a wrong answer shows up in the diff the
/// caller reads before anything is written.
fn inside_quotes(line: &str, col: u32) -> bool {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in line.chars().enumerate() {
        if i as u32 + 1 >= col {
            break;
        }
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (_, '\\') => escaped = true,
            (None, '"') | (None, '\'') | (None, '`') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            _ => {}
        }
    }
    quote.is_some()
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every whole-word occurrence of any variant in one file.
fn scan(text: &str, variants: &[Variant], file: &Path) -> Vec<Occurrence> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        for (v, variant) in variants.iter().enumerate() {
            let needle: Vec<char> = variant.from.chars().collect();
            if needle.is_empty() || needle.len() > chars.len() {
                continue;
            }
            for start in 0..=(chars.len() - needle.len()) {
                if chars[start..start + needle.len()] != needle[..] {
                    continue;
                }
                let before_ok = start == 0 || !is_word_char(chars[start - 1]);
                let after = start + needle.len();
                let after_ok = after >= chars.len() || !is_word_char(chars[after]);
                if !before_ok || !after_ok {
                    continue;
                }
                let col = start as u32 + 1;
                out.push(Occurrence {
                    file: file.to_path_buf(),
                    line: n as u32 + 1,
                    col,
                    len: needle.len(),
                    variant: v,
                    in_string: inside_quotes(line, col),
                });
            }
        }
    }
    out
}

/// An LSP text edit replacing one occurrence with the variant's new spelling.
fn edit_for(occurrence: &Occurrence, variant: &Variant) -> serde_json::Value {
    serde_json::json!({
        "range": {
            "start": { "line": occurrence.line - 1, "character": occurrence.col - 1 },
            "end": { "line": occurrence.line - 1, "character": occurrence.col - 1 + occurrence.len as u32 }
        },
        "newText": variant.to
    })
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Renames a schema field everywhere the workspace spells it.
///
/// Two phases that never touch the same text. First the analyzers: every identifier that is
/// not in a string or a comment is renamed by the engine of its own sub-project, and a rename
/// whose edits collide with one already collected is skipped and reported rather than merged —
/// two renames that want the same characters produce nonsense, and one of them can be done on
/// the next run. Then the text: whatever spellings remain in the *result* of the first phase
/// are replaced where no analyzer owns them (schema files) or where they are string literals,
/// which is why the second phase re-scans instead of reusing the first scan's positions.
#[allow(clippy::too_many_arguments)]
pub async fn rename(
    remote: SocketAddr,
    root: &Path,
    field: &str,
    to: &str,
    apply: bool,
    force: bool,
    scope: Option<&Path>,
) -> Result<SchemaRename> {
    anyhow::ensure!(
        field.chars().count() >= 3 || force,
        "`{field}` is too short to look for safely; pass `force: true` if you mean it"
    );
    anyhow::ensure!(field != to, "`{field}` and `{to}` are the same name");
    let variants = variants(field, to);
    anyhow::ensure!(
        !variants.is_empty(),
        "`{field}` has no spellings to look for"
    );

    let area = scope.unwrap_or(root);
    const MAX_FILE: u64 = 512 * 1024;
    let mut originals: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut found: Vec<Occurrence> = Vec::new();
    for file in walk(area, MAX_FILE) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue; // not UTF-8: not a source file
        };
        let hits = scan(&text, &variants, &file);
        if !hits.is_empty() {
            found.extend(hits);
            originals.insert(file, text);
        }
    }
    anyhow::ensure!(
        !found.is_empty(),
        "`{field}` {NOT_FOUND} {}",
        match display(root, area).as_str() {
            "" => "this workspace".to_string(),
            rel => rel.to_string(),
        }
    );
    const MAX_OCCURRENCES: usize = 400;
    anyhow::ensure!(
        found.len() <= MAX_OCCURRENCES || force,
        "{} occurrences of `{field}` — too many to rewrite in one step; narrow it with `path`, \
         or pass `force: true`",
        found.len()
    );

    // Phase one: the analyzers.
    const MAX_RENAMES: usize = 60;
    let mut whole: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut ranged: BTreeMap<PathBuf, Vec<serde_json::Value>> = BTreeMap::new();
    let mut claimed: BTreeMap<PathBuf, Vec<(u32, u32, u32, u32)>> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    let mut tried: Vec<(PathBuf, u32, u32)> = Vec::new();
    // Occurrences a rename already covered. Without this the loop asks the analyzer about
    // every place it has just rewritten, and every answer looks like a collision with the
    // answer that rewrote it.
    let mut done: Vec<(PathBuf, u32, u32)> = Vec::new();
    for _ in 0..MAX_RENAMES {
        let Some(o) = found
            .iter()
            .find(|o| {
                matches!(kind_of(&o.file), Kind::Code(_))
                    && !o.in_string
                    && !in_comment(&originals, o)
                    && !tried.contains(&(o.file.clone(), o.line, o.col))
                    && !done.contains(&(o.file.clone(), o.line, o.col))
            })
            .cloned()
        else {
            break;
        };
        tried.push((o.file.clone(), o.line, o.col));
        let new_name = &variants[o.variant].to;
        let edit = match rename_symbol(remote, root, &o.file, o.line, o.col, new_name).await {
            Ok(edit) => edit,
            Err(err) => {
                notes.push(format!(
                    "{}:{}:{} — rename refused: {}",
                    display(root, &o.file),
                    o.line,
                    o.col,
                    format!("{err:#}").lines().next().unwrap_or("")
                ));
                continue;
            }
        };
        let parts = ranged_edits(&edit);
        if parts.is_empty() {
            notes.push(format!(
                "{}:{}:{} — the analyzer rewrote nothing",
                display(root, &o.file),
                o.line,
                o.col
            ));
            continue;
        }
        // A rename that wants characters another rename already took is not merged: the two
        // results were both computed against the file as it is now, and applying both would
        // produce text neither of them meant.
        let collides = parts.iter().any(|(path, edits, replaces_file)| {
            let taken = claimed.get(path).map(|v| v.as_slice()).unwrap_or_default();
            let mine_replaces = *replaces_file;
            (mine_replaces && !taken.is_empty())
                || whole.contains_key(path) && !edits.is_empty()
                || edits.iter().any(|e| {
                    let span = span_of(e);
                    taken.iter().any(|other| overlaps(span, *other))
                })
        });
        if collides {
            notes.push(format!(
                "{}:{}:{} — another rename already changes these characters; apply this run and \
                 run it again to finish",
                display(root, &o.file),
                o.line,
                o.col
            ));
            continue;
        }
        for (path, edits, replaces_file) in parts {
            if replaces_file {
                let text = edits
                    .first()
                    .and_then(|e| e.get("newText"))
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string();
                // Which occurrences the new text covered can only be seen by looking at it:
                // the ones whose spelling is gone are done, and one still in place belongs to
                // another symbol, which needs a rename of its own on a later run.
                done.extend(
                    found
                        .iter()
                        .filter(|o| {
                            o.file == path && !still_spelled(&text, o, &variants[o.variant])
                        })
                        .map(|o| (o.file.clone(), o.line, o.col)),
                );
                whole.insert(path.clone(), text);
                claimed
                    .entry(path.clone())
                    .or_default()
                    .push((0, 0, u32::MAX, 0));
            } else {
                for e in &edits {
                    let span = span_of(e);
                    claimed.entry(path.clone()).or_default().push(span);
                    done.push((path.clone(), span.0 + 1, span.1 + 1));
                }
                ranged.entry(path).or_default().extend(edits);
            }
        }
    }

    // What the analyzers made of each file they touched, plus every file the scan found.
    let mut base: BTreeMap<PathBuf, String> = originals.clone();
    for (path, text) in whole {
        base.insert(path, text);
    }
    for (path, edits) in ranged {
        let before = match base.get(&path) {
            Some(text) => text.clone(),
            None => std::fs::read_to_string(&path).unwrap_or_default(),
        };
        let after = crate::refactor::apply_text_edits(&before, &edits)
            .with_context(|| format!("applying the rename to {}", display(root, &path)))?;
        base.insert(path, after);
    }

    // Phase two: the text the analyzers do not own, found again in what they produced.
    let mut rewritten: Vec<(PathBuf, String)> = Vec::new();
    let mut left: Vec<String> = Vec::new();
    let mut as_text: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut remaining: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (path, text) in &base {
        let kind = kind_of(path);
        let language = label(path, text);
        let schema = schema_of(path, text);
        let hits = scan(text, &variants, path);
        let mut edits = Vec::new();
        for o in &hits {
            let Some(language) = language else {
                continue;
            };
            let (ours, why) = match (kind, schema) {
                (Kind::Text(_), Some(schema)) => (
                    is_structural(schema, text, o),
                    match schema {
                        Schema::OpenApi => " (prose in the OpenAPI document, not the field)",
                        Schema::GraphQl => " (a GraphQL comment or description)",
                    },
                ),
                (Kind::Text(_), None) => (true, ""),
                _ => (o.in_string, ""),
            };
            if ours {
                *as_text.entry(language).or_default() += 1;
                edits.push(edit_for(o, &variants[o.variant]));
            } else {
                *remaining.entry(language).or_default() += 1;
                left.push(format!(
                    "{}:{}:{} `{}`{why}",
                    display(root, path),
                    o.line,
                    o.col,
                    variants[o.variant].from
                ));
            }
        }
        let after = if edits.is_empty() {
            text.clone()
        } else {
            crate::refactor::apply_text_edits(text, &edits)
                .with_context(|| format!("rewriting the text of {}", display(root, path)))?
        };
        let on_disk = originals
            .get(path)
            .cloned()
            .or_else(|| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        if after != on_disk {
            rewritten.push((path.clone(), after));
        }
    }
    rewritten.sort_by(|a, b| a.0.cmp(&b.0));
    left.extend(notes);

    // Per language: what the scan found, and what became of it.
    let mut summary = Vec::new();
    let mut by_language: BTreeMap<&'static str, usize> = BTreeMap::new();
    for o in &found {
        if let Some(l) = originals.get(&o.file).and_then(|text| label(&o.file, text)) {
            *by_language.entry(l).or_default() += 1;
        }
    }
    for (language, total) in &by_language {
        let text = as_text.get(language).copied().unwrap_or(0);
        let still = remaining.get(language).copied().unwrap_or(0);
        let semantic = total.saturating_sub(text + still);
        summary.push(format!(
            "{language}: {total} occurrence(s) found, {semantic} renamed by the analyzer, \
             {text} rewritten as text, {still} left alone"
        ));
    }
    let unseen = rewritten
        .iter()
        .filter(|(p, _)| !originals.contains_key(p))
        .count();
    if unseen > 0 {
        summary.push(format!(
            "{unseen} file(s) the scan never looked at were updated by an analyzer, because the \
             symbol reaches them"
        ));
    }
    summary.push(format!(
        "spellings looked for: {}",
        variants
            .iter()
            .map(|v| format!("`{}` ({})", v.from, v.style))
            .collect::<Vec<_>>()
            .join(", ")
    ));

    // Every project checked by its own analyzer: one language's engine cannot judge another's.
    let mut by_project: BTreeMap<String, Vec<(PathBuf, String)>> = BTreeMap::new();
    for (path, text) in &rewritten {
        if !matches!(kind_of(path), Kind::Code(_)) {
            continue;
        }
        let (subdir, _) = crate::sync::engine_project(root, path);
        by_project
            .entry(subdir.unwrap_or_default())
            .or_default()
            .push((path.clone(), text.clone()));
    }
    let mut diagnostics = Vec::new();
    for group in by_project.values() {
        let reports = crate::diagnostics::validate_texts(remote, root, group, &[]).await?;
        for report in &reports {
            for d in report.items.iter().filter(|d| d.severity == "error") {
                diagnostics.push(format!(
                    "{}{} ({}:{}:{})",
                    d.message.lines().next().unwrap_or(""),
                    d.code
                        .as_deref()
                        .map(|c| format!(" [{c}]"))
                        .unwrap_or_default(),
                    report.file,
                    d.line,
                    d.col
                ));
            }
        }
    }

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the rename does not compile ({} error(s)); nothing was written. Pass `force: true` \
             to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        write_rewritten(root, &rewritten)?;
        applied = true;
    }

    Ok(SchemaRename {
        field: field.to_string(),
        to: to.to_string(),
        root: root.to_path_buf(),
        rewritten,
        summary,
        left,
        diagnostics,
        applied,
    })
}

/// What [`rename`] says when the field is nowhere in the area it searched.
const NOT_FOUND: &str = "does not appear under";

/// Writes every rewritten file of the checkout at `root` in one edit.
fn write_rewritten(root: &Path, rewritten: &[(PathBuf, String)]) -> Result<()> {
    let changes: Vec<serde_json::Value> = rewritten
        .iter()
        .map(|(path, new_text)| {
            let old_lines = std::fs::read_to_string(path)
                .map(|t| t.lines().count())
                .unwrap_or(0);
            serde_json::json!({
                "textDocument": { "uri": format!("file://{}", path.display()), "version": null },
                "edits": [ {
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": old_lines, "character": 0 }
                    },
                    "newText": new_text
                } ]
            })
        })
        .collect();
    crate::refactor::apply_workspace_edit(
        root,
        &serde_json::json!({ "documentChanges": changes }),
    )?;
    Ok(())
}

/// A rename across several repositories: the backend that owns the schema and the frontends
/// and services that read it.
#[derive(Debug)]
pub struct AcrossRepos {
    /// One plan per repository the field appears in, in the order given.
    pub repos: Vec<SchemaRename>,
    /// The repositories it does not appear in.
    pub missing: Vec<PathBuf>,
    pub applied: bool,
}

impl AcrossRepos {
    /// Every repository's analyzers accept its result.
    pub fn clean(&self) -> bool {
        self.repos.iter().all(|r| r.diagnostics.is_empty())
    }

    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = String::new();
        let share = diff_budget / self.repos.len().max(1);
        for repo in &self.repos {
            out.push_str(&format!(
                "## {}\n\n{}\n",
                repo.root.display(),
                repo.render(share)
            ));
        }
        for root in &self.missing {
            out.push_str(&format!(
                "## {}\n\nthe field does not appear here\n\n",
                root.display()
            ));
        }
        out.push_str(&if self.applied {
            format!("[applied to {} repositories together]\n", self.repos.len())
        } else {
            format!(
                "nothing was written in any of the {} repositories; `apply: true` writes them all \
                 or none\n",
                self.repos.len() + self.missing.len()
            )
        });
        out
    }
}

/// Renames a schema field in several repositories as one change.
///
/// Each repository is planned and checked by its own analyzers, exactly as [`rename`] does for
/// one. Nothing is written until every plan is ready, and with `apply` nothing is written unless
/// every repository accepts its result (or `force`). The repositories are then written one
/// after another; when one of them cannot be written, the ones already written are put back
/// as they were, so the change lands in all of them or in none.
pub async fn rename_across(
    remote: SocketAddr,
    roots: &[PathBuf],
    field: &str,
    to: &str,
    apply: bool,
    force: bool,
) -> Result<AcrossRepos> {
    let mut repos = Vec::new();
    let mut missing = Vec::new();
    for root in roots {
        match rename(remote, root, field, to, false, force, None).await {
            Ok(plan) => repos.push(plan),
            Err(err) if format!("{err}").contains(NOT_FOUND) => missing.push(root.clone()),
            Err(err) => return Err(err.context(format!("in {}", root.display()))),
        }
    }
    anyhow::ensure!(
        !repos.is_empty(),
        "`{field}` does not appear in any of the {} repositories",
        roots.len()
    );
    let mut applied = false;
    if apply {
        let errors: Vec<String> = repos
            .iter()
            .flat_map(|r| {
                r.diagnostics
                    .iter()
                    .map(move |d| format!("{}: {d}", r.root.display()))
            })
            .collect();
        anyhow::ensure!(
            errors.is_empty() || force,
            "the rename does not compile ({} error(s)); nothing was written in any repository. \
             Pass `force: true` to write it anyway:\n  {}",
            errors.len(),
            errors.join("\n  ")
        );
        let mut written: Vec<(PathBuf, String)> = Vec::new();
        for repo in &repos {
            let before: Vec<(PathBuf, String)> = repo
                .rewritten
                .iter()
                .map(|(path, _)| {
                    (
                        path.clone(),
                        std::fs::read_to_string(path).unwrap_or_default(),
                    )
                })
                .collect();
            if let Err(err) = write_rewritten(&repo.root, &repo.rewritten) {
                let unrestored: Vec<String> = written
                    .iter()
                    .filter_map(|(path, text)| {
                        std::fs::write(path, text)
                            .err()
                            .map(|e| format!("{}: {e}", path.display()))
                    })
                    .collect();
                anyhow::ensure!(
                    unrestored.is_empty(),
                    "writing {} failed ({err:#}), and these files could not be put back:\n  {}",
                    repo.root.display(),
                    unrestored.join("\n  ")
                );
                return Err(err.context(format!(
                    "writing {} failed; the repositories written before it were put back, so \
                     nothing changed",
                    repo.root.display()
                )));
            }
            written.extend(before);
        }
        for repo in &mut repos {
            repo.applied = true;
        }
        applied = true;
    }
    Ok(AcrossRepos {
        repos,
        missing,
        applied,
    })
}

/// Is the occurrence still spelled the old way in this text, at the position it was found?
fn still_spelled(text: &str, occurrence: &Occurrence, variant: &Variant) -> bool {
    text.lines()
        .nth(occurrence.line as usize - 1)
        .map(|line| {
            let at: String = line
                .chars()
                .skip(occurrence.col as usize - 1)
                .take(occurrence.len)
                .collect();
            at == variant.from
        })
        .unwrap_or(false)
}

/// Do two edit ranges want any of the same characters?
fn overlaps(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> bool {
    let (a_start, a_end) = ((a.0, a.1), (a.2, a.3));
    let (b_start, b_end) = ((b.0, b.1), (b.2, b.3));
    a_start < b_end && b_start < a_end
}

/// Is this occurrence inside a comment? A rename does not follow a name into prose, and
/// neither does this: such an occurrence is reported instead of quietly rewritten.
///
/// Which marker starts a comment depends on the language, and `#` in particular is a comment
/// in Python and an attribute in Rust, so it is not treated as one for a Rust file.
fn in_comment(texts: &BTreeMap<PathBuf, String>, o: &Occurrence) -> bool {
    let Some(text) = texts.get(&o.file) else {
        return false;
    };
    let Some(line) = text.lines().nth(o.line as usize - 1) else {
        return false;
    };
    let prefix: String = line.chars().take(o.col as usize - 1).collect();
    let markers: &[&str] = match kind_of(&o.file) {
        Kind::Code("python") => &["#"],
        Kind::Code("rust")
        | Kind::Code("go")
        | Kind::Code("typescript")
        | Kind::Code("javascript")
        | Kind::Code("swift")
        | Kind::Code("c/c++") => &["//", "/*"],
        Kind::Text("sql") => &["--"],
        Kind::Text("python") | Kind::Text("shell") | Kind::Text("yaml") | Kind::Text("toml") => {
            &["#"]
        }
        _ => &[],
    };
    markers.iter().any(|m| prefix.contains(m))
}

/// The text edits of a workspace edit, per file, and whether they replace the file wholesale.
///
/// The two shapes are not interchangeable: the forwarded language servers answer a rename with
/// one edit per occurrence, while our in-process Rust engine answers with the file's whole new
/// text. Merging the second with anything else produces nonsense, so it is flagged here and
/// handled separately.
fn ranged_edits(edit: &serde_json::Value) -> Vec<(PathBuf, Vec<serde_json::Value>, bool)> {
    let mut out = Vec::new();
    let changes = edit
        .get("documentChanges")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    for change in changes {
        let Some(uri) = change.pointer("/textDocument/uri").and_then(|u| u.as_str()) else {
            continue;
        };
        let Some(list) = change.get("edits").and_then(|e| e.as_array()) else {
            continue;
        };
        out.push((
            PathBuf::from(crate::remote_fs::uri_to_path(uri)),
            list.clone(),
            replaces_whole_file(list),
        ));
    }
    if out.is_empty()
        && let Some(map) = edit.get("changes").and_then(|c| c.as_object())
    {
        for (uri, list) in map {
            if let Some(list) = list.as_array() {
                out.push((
                    PathBuf::from(crate::remote_fs::uri_to_path(uri)),
                    list.clone(),
                    replaces_whole_file(list),
                ));
            }
        }
    }
    out
}

/// One edit that starts at the top of the file and ends at the start of a later line is a
/// whole-file replacement; a rename's edit covers one identifier and never looks like that.
fn replaces_whole_file(edits: &[serde_json::Value]) -> bool {
    match edits {
        [only] => {
            let (sl, sc, el, ec) = span_of(only);
            sl == 0 && sc == 0 && ec == 0 && el >= 1
        }
        _ => false,
    }
}

/// A text edit's range as (start line, start character, end line, end character), 0-based.
fn span_of(edit: &serde_json::Value) -> (u32, u32, u32, u32) {
    let at = |p: &str| edit.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    (
        at("/range/start/line"),
        at("/range/start/character"),
        at("/range/end/line"),
        at("/range/end/character"),
    )
}

/// One semantic rename, on the session of the project the file belongs to.
async fn rename_symbol(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    new_name: &str,
) -> Result<serde_json::Value> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/rename",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            "newName": new_name,
        }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_name_however_it_is_written() {
        for name in [
            "order_id", "orderId", "OrderId", "OrderID", "ORDER_ID", "order-id",
        ] {
            assert_eq!(words(name), ["order", "id"], "{name}");
        }
        assert_eq!(words("httpURLBuilder"), ["http", "url", "builder"]);
    }

    #[test]
    fn every_language_gets_its_own_spelling() {
        let v = variants("order_id", "trade_id");
        let by_style = |style: &str| {
            v.iter()
                .find(|x| x.style == style)
                .map(|x| (x.from.as_str(), x.to.as_str()))
        };
        assert_eq!(by_style("snake_case"), Some(("order_id", "trade_id")));
        assert_eq!(by_style("camelCase"), Some(("orderId", "tradeId")));
        assert_eq!(by_style("PascalCase"), Some(("OrderId", "TradeId")));
        assert_eq!(by_style("Go PascalCase"), Some(("OrderID", "TradeID")));
        assert_eq!(by_style("SCREAMING_CASE"), Some(("ORDER_ID", "TRADE_ID")));
    }

    #[test]
    fn a_spelling_that_repeats_is_listed_once() {
        // `symbol` is one word: snake, camel and kebab all spell it the same.
        let v = variants("symbol", "ticker");
        assert_eq!(v.iter().filter(|x| x.from == "symbol").count(), 1);
    }

    #[test]
    fn only_whole_words_are_found() {
        let v = variants("order_id", "trade_id");
        let text = "let order_id = 1; let reorder_id = 2; let order_ident = 3;\n";
        let found = scan(text, &v, Path::new("a.rs"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].col, 5);
    }

    #[test]
    fn a_name_inside_a_string_is_marked() {
        let v = variants("order_id", "trade_id");
        let text = "OrderID string `json:\"order_id\"`\n";
        let found = scan(text, &v, Path::new("a.go"));
        assert_eq!(found.len(), 2);
        assert!(!found.iter().any(|o| o.in_string && o.variant == 3));
        let in_tag = found.iter().find(|o| o.in_string).expect("the tag");
        assert_eq!(&v[in_tag.variant].from, "order_id");
    }

    #[test]
    fn quotes_of_every_kind_are_understood() {
        assert!(inside_quotes("a = \"order_id\"", 7));
        assert!(inside_quotes("a = `order_id`", 7));
        assert!(!inside_quotes("a = order_id", 5));
        assert!(!inside_quotes("a = \"x\" + order_id", 12));
    }

    #[test]
    fn a_file_is_classified_by_what_owns_it() {
        assert_eq!(kind_of(Path::new("a/b.rs")), Kind::Code("rust"));
        assert_eq!(kind_of(Path::new("a/b.proto")), Kind::Text("protobuf"));
        assert_eq!(kind_of(Path::new("a/b.png")), Kind::Skip);
    }

    #[test]
    fn the_two_shapes_of_a_rename_answer_are_told_apart() {
        // gopls and the TypeScript server: one edit per occurrence.
        let ranged = serde_json::json!({ "documentChanges": [ {
            "textDocument": { "uri": "file:///w/a.go" },
            "edits": [
                { "range": { "start": { "line": 3, "character": 1 }, "end": { "line": 3, "character": 8 } }, "newText": "TradeID" }
            ]
        } ] });
        let parts = ranged_edits(&ranged);
        assert_eq!(parts.len(), 1);
        assert!(!parts[0].2, "one identifier edit is not a whole file");

        // rust-analyzer: the file's whole new text.
        let whole = serde_json::json!({ "documentChanges": [ {
            "textDocument": { "uri": "file:///w/a.rs" },
            "edits": [
                { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 12, "character": 0 } }, "newText": "fn main() {}\n" }
            ]
        } ] });
        let parts = ranged_edits(&whole);
        assert!(
            parts[0].2,
            "a replacement from the top of the file is the whole file"
        );
    }

    #[test]
    fn an_identifier_at_the_very_start_of_a_file_is_not_a_whole_file_replacement() {
        let edit = serde_json::json!([
            { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 8 } }, "newText": "trade_id" }
        ]);
        assert!(!replaces_whole_file(edit.as_array().unwrap()));
    }

    #[test]
    fn edits_that_want_the_same_characters_are_seen_to_overlap() {
        let a = (5, 10, 5, 17);
        assert!(overlaps(a, (5, 12, 5, 20)), "a later start inside it");
        assert!(
            overlaps(a, (5, 0, 5, 11)),
            "an earlier one reaching into it"
        );
        assert!(
            !overlaps(a, (5, 17, 5, 24)),
            "starting where it ends is not an overlap"
        );
        assert!(!overlaps(a, (4, 0, 4, 40)), "another line");
        assert!(
            overlaps(a, (0, 0, u32::MAX, 0)),
            "a whole-file replacement takes everything"
        );
    }

    #[test]
    fn a_comment_marker_depends_on_the_language() {
        let rust = PathBuf::from("/w/src/lib.rs");
        let python = PathBuf::from("/w/app.py");
        let mut texts = BTreeMap::new();
        texts.insert(
            rust.clone(),
            "#[derive(Debug)] // order_id
let order_id = 1;
"
            .to_string(),
        );
        texts.insert(
            python.clone(),
            "# order_id is the key
"
            .to_string(),
        );
        // `#` starts an attribute in Rust, not a comment: the attribute line is not prose.
        let attribute = Occurrence {
            file: rust.clone(),
            line: 2,
            col: 5,
            len: 8,
            variant: 0,
            in_string: false,
        };
        assert!(!in_comment(&texts, &attribute));
        let after_slashes = Occurrence {
            file: rust,
            line: 1,
            col: 21,
            len: 8,
            variant: 0,
            in_string: false,
        };
        assert!(in_comment(&texts, &after_slashes));
        let hash = Occurrence {
            file: python,
            line: 1,
            col: 3,
            len: 8,
            variant: 0,
            in_string: false,
        };
        assert!(in_comment(&texts, &hash), "in Python it is a comment");
    }

    #[test]
    fn openapi_and_graphql_are_read_for_their_structure() {
        let yaml = Path::new("/w/api/openapi.yaml");
        let spec = "openapi: 3.0.3\ncomponents:\n  schemas:\n    Order:\n      required: [id, order_id]\n      properties:\n        order_id:\n          description: The order_id of the order\n        list:\n          - order_id # the key\n          - \"order_id\"\n      x-note: order_id, then\n";
        assert_eq!(schema_of(yaml, spec), Some(Schema::OpenApi));
        assert_eq!(schema_of(yaml, "name: order_id\n"), None);
        assert_eq!(label(yaml, spec), Some("openapi"));
        assert_eq!(label(yaml, "a: 1\n"), Some("yaml"));
        let json = Path::new("/w/api/openapi.json");
        let doc = "{\n  \"openapi\": \"3.1.0\",\n  \"required\": [\"order_id\"],\n  \"order_id\": {\"description\": \"the order_id\"}\n}\n";
        assert_eq!(schema_of(json, doc), Some(Schema::OpenApi));
        assert_eq!(schema_of(json, "{\"a\": 1}\n"), None);
        let variants = variants("order_id", "trade_id");
        let structural = |path: &Path, text: &str| -> Vec<(u32, bool)> {
            let schema = schema_of(path, text).unwrap();
            scan(text, &variants, path)
                .iter()
                .map(|o| (o.line, is_structural(schema, text, o)))
                .collect()
        };
        assert_eq!(
            structural(yaml, spec),
            vec![
                (5, true),
                (7, true),
                (8, false),
                (10, true),
                (11, true),
                // `x-note: order_id, then` looks like a flow list: a whole value followed by a
                // comma. Prose written that way is rewritten; the diff shows it.
                (12, true),
            ]
        );
        assert_eq!(
            structural(json, doc),
            vec![(3, true), (4, true), (4, false)]
        );

        let gql = Path::new("/w/schema.graphql");
        let sdl = "type Order {\n  \"\"\"\n  Not the orderId of a trade.\n  \"\"\"\n  orderId: ID! # orderId is the key\n  \"the orderId\" total(orderId: ID): Int\n}\n";
        assert_eq!(schema_of(gql, sdl), Some(Schema::GraphQl));
        assert_eq!(label(gql, sdl), Some("graphql"));
        assert_eq!(
            structural(gql, sdl),
            vec![(3, false), (5, true), (5, false), (6, false), (6, true)]
        );
    }

    #[test]
    fn the_walk_skips_what_is_not_source() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (rel, text) in [
            ("src/a.rs", "fn a() {}"),
            ("schema/b.proto", "message B {}"),
            ("target/debug/c.rs", "fn c() {}"),
            ("node_modules/d/e.ts", "export {};"),
            ("logo.png", "not text"),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let found: Vec<String> = walk(root, 512 * 1024)
            .iter()
            .map(|p| display(root, p))
            .collect();
        assert_eq!(found, ["schema/b.proto", "src/a.rs"]);
    }

    #[test]
    fn an_edit_replaces_exactly_the_occurrence() {
        let v = variants("order_id", "trade_id");
        let text = "  order_id TEXT PRIMARY KEY,\n";
        let found = scan(text, &v, Path::new("schema.sql"));
        let edit = edit_for(&found[0], &v[found[0].variant]);
        assert_eq!(edit["range"]["start"]["character"], 2);
        assert_eq!(edit["range"]["end"]["character"], 10);
        assert_eq!(edit["newText"], "trade_id");
    }
}
