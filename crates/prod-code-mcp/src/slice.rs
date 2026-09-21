//! Program slicing (roadmap 7.3): given a symbol, return the code it actually depends on
//! instead of the files it happens to live in.
//!
//! The slice is built from what the analyzer already knows, so it needs no new wire message:
//! `textDocument/documentSymbol` gives every item's range in a file, so an item's source text
//! is a range of lines; `callHierarchy/outgoingCalls` gives the functions a function calls;
//! `textDocument/definition` resolves any other name a body mentions (a type, a constant, a
//! trait) to the item that declares it. Starting from the seed item, the slicer walks those
//! edges breadth-first to a depth limit and returns the collected items, grouped by file, in
//! file order, with a count of what it left out.
//!
//! Names are found by scanning the body for identifier-shaped tokens and asking the analyzer
//! about each distinct one. A token that is a keyword, a local or a literal resolves to
//! nothing, or to a position inside the body itself, and is dropped; the analyzer decides,
//! not the scanner.

use crate::session::LspSession;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// How many bytes of slice to return before stopping, unless the caller says otherwise.
pub const DEFAULT_MAX_BYTES: usize = 24 * 1024;
/// How far to follow dependencies from the seed by default.
pub const DEFAULT_DEPTH: u32 = 2;
/// Distinct names resolved per item; a body that mentions more than this is truncated.
const MAX_NAMES_PER_ITEM: usize = 64;
/// Names from outside the workspace listed in the report before it says "and N more".
const EXTERNAL_SHOWN: usize = 12;

/// One item in the slice: a whole declaration, as it appears in its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceItem {
    /// Path relative to the workspace root.
    pub file: String,
    pub name: String,
    pub kind: &'static str,
    /// 1-based inclusive line range of the whole declaration.
    pub start_line: u32,
    pub end_line: u32,
    /// How many edges from the seed: 0 is the seed itself.
    pub depth: u32,
    /// Why it is here: the item that referenced it.
    pub because: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct SliceReport {
    pub seed: String,
    pub items: Vec<SliceItem>,
    /// Bytes of the files the slice draws from.
    pub source_bytes: usize,
    /// Names that resolved outside the workspace (std, crates.io) and were not followed.
    pub external: Vec<String>,
    /// Items dropped because the budget ran out.
    pub truncated: usize,
}

impl SliceReport {
    pub fn slice_bytes(&self) -> usize {
        self.items.iter().map(|i| i.text.len()).sum()
    }

    /// Share of the source files the slice replaces, as a percentage.
    pub fn reduction_percent(&self) -> f64 {
        if self.source_bytes == 0 {
            return 0.0;
        }
        100.0 - (self.slice_bytes() as f64 * 100.0 / self.source_bytes as f64)
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "slice of `{}`: {} item(s), {} bytes from {} bytes of source ({:.0}% smaller)\n",
            self.seed,
            self.items.len(),
            self.slice_bytes(),
            self.source_bytes,
            self.reduction_percent()
        );
        if self.truncated > 0 {
            out.push_str(&format!(
                "{} further item(s) omitted: the byte budget ran out\n",
                self.truncated
            ));
        }
        if !self.external.is_empty() {
            let mut ext = self.external.clone();
            ext.sort();
            ext.dedup();
            let shown = ext.len().min(EXTERNAL_SHOWN);
            let more = ext.len() - shown;
            out.push_str(&format!(
                "outside the workspace, not followed: {}{}\n",
                ext[..shown].join(", "),
                if more > 0 {
                    format!(", and {more} more")
                } else {
                    String::new()
                }
            ));
        }
        let mut by_file: BTreeMap<&str, Vec<&SliceItem>> = BTreeMap::new();
        for item in &self.items {
            by_file.entry(item.file.as_str()).or_default().push(item);
        }
        for (file, mut items) in by_file {
            items.sort_by_key(|i| i.start_line);
            out.push_str(&format!("\n=== {file}\n"));
            for item in items {
                let why = match (&item.because, item.depth) {
                    (_, 0) => " (the seed)".to_string(),
                    (Some(from), d) => format!(" (depth {d}, used by {from})"),
                    (None, d) => format!(" (depth {d})"),
                };
                out.push_str(&format!(
                    "\n[{}] {}  {}:{}-{}{}\n{}\n",
                    item.kind, item.name, file, item.start_line, item.end_line, why, item.text
                ));
            }
        }
        out
    }
}

/// An item declared in a file, as `textDocument/documentSymbol` reports it.
#[derive(Debug, Clone)]
struct Decl {
    name: String,
    kind: &'static str,
    start_line: u32,
    end_line: u32,
}

fn kind_name(kind: u64) -> &'static str {
    match kind {
        2 => "module",
        5 => "class",
        6 => "method",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        22 => "enum member",
        23 => "struct",
        26 => "type parameter",
        _ => "item",
    }
}

/// Declarations worth slicing: things with a body or a shape, not fields or locals.
fn is_sliceable(kind: u64) -> bool {
    matches!(kind, 5 | 6 | 9 | 10 | 11 | 12 | 14 | 23)
}

/// Flattens a documentSymbol tree into declarations, keeping nested items (methods in an
/// impl) as their own entries.
fn collect_decls(symbols: &[serde_json::Value], out: &mut Vec<Decl>) {
    for sym in symbols {
        let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
        let name = sym
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        let range = sym
            .get("range")
            .or_else(|| sym.get("location").and_then(|l| l.get("range")));
        let sel = sym.get("selectionRange").or(range);
        if let (Some(range), Some(_sel)) = (range, sel)
            && let (Some(start), Some(end)) = (range.get("start"), range.get("end"))
            && is_sliceable(kind)
            && !name.is_empty()
        {
            let n = |v: &serde_json::Value, k: &str| {
                v.get(k).and_then(|x| x.as_u64()).unwrap_or(0) as u32 + 1
            };
            out.push(Decl {
                name,
                kind: kind_name(kind),
                start_line: n(start, "line"),
                end_line: n(end, "line"),
            });
        }
        if let Some(children) = sym.get("children").and_then(|c| c.as_array()) {
            collect_decls(children, out);
        }
    }
}

/// Identifier-shaped tokens of a body, in order, deduplicated, with the line and column of
/// the first occurrence of each (1-based). Words inside line comments and string literals are
/// skipped: they cost a round trip and never resolve to anything useful.
pub fn candidate_names(text: &str, start_line: u32) -> Vec<(String, u32, u32)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = start_line + i as u32;
        let code = strip_noise(raw);
        let bytes = code.as_bytes();
        let mut col = 0usize;
        while col < bytes.len() {
            let c = bytes[col] as char;
            if !(c.is_ascii_alphabetic() || c == '_') {
                col += 1;
                continue;
            }
            let start = col;
            while col < bytes.len() {
                let c = bytes[col] as char;
                if c.is_ascii_alphanumeric() || c == '_' {
                    col += 1;
                } else {
                    break;
                }
            }
            let word = &code[start..col];
            if word.len() > 1 && !is_keyword(word) && seen.insert(word.to_string()) {
                out.push((word.to_string(), line, start as u32 + 1));
            }
        }
    }
    out
}

/// Blanks out line comments and string literals so their words are not resolved.
fn strip_noise(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_string = !in_string;
                out.push(' ');
            }
            '\\' if in_string => {
                chars.next();
                out.push_str("  ");
            }
            '/' if !in_string && chars.peek() == Some(&'/') => {
                out.push_str(&" ".repeat(line.len() - out.len()));
                break;
            }
            '#' if !in_string => {
                out.push_str(&" ".repeat(line.len() - out.len()));
                break;
            }
            _ if in_string => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

fn is_keyword(word: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        // Rust
        "as",
        "async",
        "await",
        "break",
        "const",
        "continue",
        "crate",
        "dyn",
        "else",
        "enum",
        "extern",
        "false",
        "fn",
        "for",
        "if",
        "impl",
        "in",
        "let",
        "loop",
        "match",
        "mod",
        "move",
        "mut",
        "pub",
        "ref",
        "return",
        "self",
        "Self",
        "static",
        "struct",
        "super",
        "trait",
        "true",
        "type",
        "unsafe",
        "use",
        "where",
        "while",
        // Go, TypeScript, Python, C-family words that are not names either
        "func",
        "package",
        "import",
        "var",
        "range",
        "defer",
        "chan",
        "go",
        "interface",
        "map",
        "nil",
        "function",
        "class",
        "new",
        "this",
        "null",
        "undefined",
        "export",
        "def",
        "class_",
        "None",
        "True",
        "False",
        "elif",
        "pass",
        "raise",
        "with",
        "lambda",
        "int",
        "bool",
        "string",
        "str",
        "float",
        "void",
        "auto",
        "template",
        "namespace",
    ];
    KEYWORDS.contains(&word)
}

/// Text of lines `start..=end` (1-based, inclusive).
fn lines_of(text: &str, start: u32, end: u32) -> String {
    text.lines()
        .skip(start.saturating_sub(1) as usize)
        .take((end.saturating_sub(start) + 1) as usize)
        .collect::<Vec<_>>()
        .join("\n")
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Everything the slicer needs to know about one file, fetched once.
struct FileFacts {
    text: String,
    decls: Vec<Decl>,
}

async fn file_facts(session: &mut LspSession, file: &Path) -> Result<FileFacts> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let uri = session.uri_for(file)?;
    let symbols = session
        .query(
            file,
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await?;
    let mut decls = Vec::new();
    collect_decls(
        symbols.as_array().map(|a| a.as_slice()).unwrap_or(&[]),
        &mut decls,
    );
    Ok(FileFacts { text, decls })
}

/// The smallest declaration containing `line`.
fn decl_at(decls: &[Decl], line: u32) -> Option<&Decl> {
    decls
        .iter()
        .filter(|d| d.start_line <= line && line <= d.end_line)
        .min_by_key(|d| d.end_line - d.start_line)
}

/// One definition location, as `textDocument/definition` answers.
fn first_location(value: &serde_json::Value) -> Option<(String, u32)> {
    let one = if value.is_array() {
        value.as_array()?.first()?.clone()
    } else {
        value.clone()
    };
    let uri = one
        .get("uri")
        .or_else(|| one.get("targetUri"))
        .and_then(|u| u.as_str())?
        .to_string();
    let range = one
        .get("range")
        .or_else(|| one.get("targetSelectionRange"))?;
    let line = range.get("start")?.get("line")?.as_u64()? as u32 + 1;
    Some((uri, line))
}

/// Builds the slice. `seed` is a file and a 1-based position on the symbol's name.
pub async fn slice(
    remote: SocketAddr,
    root: &Path,
    seed_file: &Path,
    seed_line: u32,
    seed_col: u32,
    depth: u32,
    max_bytes: usize,
) -> Result<SliceReport> {
    let mut session = LspSession::open(remote, root, Some(seed_file)).await?;
    let result = slice_with(
        &mut session,
        root,
        seed_file,
        seed_line,
        seed_col,
        depth,
        max_bytes,
    )
    .await;
    session.close().await;
    result
}

async fn slice_with(
    session: &mut LspSession,
    root: &Path,
    seed_file: &Path,
    seed_line: u32,
    seed_col: u32,
    depth: u32,
    max_bytes: usize,
) -> Result<SliceReport> {
    let mut facts: BTreeMap<PathBuf, FileFacts> = BTreeMap::new();
    facts.insert(
        seed_file.to_path_buf(),
        file_facts(session, seed_file).await?,
    );

    let seed_decl = {
        let f = &facts[seed_file];
        decl_at(&f.decls, seed_line).cloned()
    }
    .with_context(|| {
        format!(
            "no declaration at {}:{seed_line}",
            relative(root, seed_file)
        )
    })?;

    let mut report = SliceReport {
        seed: seed_decl.name.clone(),
        ..Default::default()
    };
    let mut queued: HashSet<(PathBuf, u32)> = HashSet::new();
    let mut queue: VecDeque<(PathBuf, Decl, u32, Option<String>)> = VecDeque::new();
    queued.insert((seed_file.to_path_buf(), seed_decl.start_line));
    queue.push_back((seed_file.to_path_buf(), seed_decl, 0, None));
    let _ = (seed_col, &mut report.external);

    let mut bytes = 0usize;
    while let Some((file, decl, item_depth, because)) = queue.pop_front() {
        let text = {
            let f = facts
                .get(&file)
                .expect("facts are inserted before a file is queued");
            lines_of(&f.text, decl.start_line, decl.end_line)
        };
        if bytes + text.len() > max_bytes && !report.items.is_empty() {
            report.truncated += 1 + queue.len();
            break;
        }
        bytes += text.len();
        report.items.push(SliceItem {
            file: relative(root, &file),
            name: decl.name.clone(),
            kind: decl.kind,
            start_line: decl.start_line,
            end_line: decl.end_line,
            depth: item_depth,
            because,
            text: text.clone(),
        });
        if item_depth >= depth {
            continue;
        }

        // Ask the analyzer about every distinct name this body mentions. A name that is a
        // local, a keyword or a literal resolves to nothing or to this same declaration.
        let names = candidate_names(&text, decl.start_line);
        for (name, line, col) in names.into_iter().take(MAX_NAMES_PER_ITEM) {
            let uri = session.uri_for(&file)?;
            let params = serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": line - 1, "character": col - 1 },
            });
            let Ok(found) = session
                .query(&file, "textDocument/definition", params)
                .await
            else {
                continue;
            };
            let Some((target_uri, target_line)) = first_location(&found) else {
                continue;
            };
            let target = PathBuf::from(crate::remote_fs::uri_to_path(&target_uri));
            if crate::remote_fs::is_external(root, &target.to_string_lossy()) {
                report.external.push(name);
                continue;
            }
            if !target.is_file() {
                continue;
            }
            if !facts.contains_key(&target) {
                match file_facts(session, &target).await {
                    Ok(f) => {
                        facts.insert(target.clone(), f);
                    }
                    Err(_) => continue,
                }
            }
            let target_decl = {
                let f = &facts[&target];
                decl_at(&f.decls, target_line).cloned()
            };
            let Some(target_decl) = target_decl else {
                continue;
            };
            // A name that resolves inside the item we are already looking at is a local.
            if target == file
                && target_decl.start_line == decl.start_line
                && target_decl.end_line == decl.end_line
            {
                continue;
            }
            if queued.insert((target.clone(), target_decl.start_line)) {
                queue.push_back((target, target_decl, item_depth + 1, Some(decl.name.clone())));
            }
        }
    }

    // What the agent would have read instead: every file the slice touched.
    let touched: HashSet<&str> = report.items.iter().map(|i| i.file.as_str()).collect();
    report.source_bytes = touched
        .iter()
        .filter_map(|rel| facts.get(&root.join(rel)).map(|f| f.text.len()))
        .sum();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_names_skips_keywords_comments_and_strings() {
        let body = "pub fn run(cfg: Config) -> Result<Metrics> {\n    // Metrics of the thing\n    let name = \"Config in a string\";\n    record(cfg, name)\n}";
        let names: Vec<String> = candidate_names(body, 10)
            .into_iter()
            .map(|(n, _, _)| n)
            .collect();
        assert!(names.contains(&"Config".to_string()));
        assert!(names.contains(&"Result".to_string()));
        assert!(names.contains(&"record".to_string()));
        assert!(!names.contains(&"pub".to_string()));
        assert!(!names.contains(&"fn".to_string()));
        assert!(!names.contains(&"let".to_string()));
        // "Metrics" appears in the signature, so its first position is the signature, not
        // the comment; the comment itself contributes nothing new.
        let metrics = candidate_names(body, 10)
            .into_iter()
            .find(|(n, _, _)| n == "Metrics")
            .expect("Metrics is a candidate");
        assert_eq!(metrics.1, 10, "first occurrence is on the signature line");
        assert!(!names.contains(&"string".to_string()));
    }

    #[test]
    fn candidate_positions_are_one_based_and_point_at_the_name() {
        let body = "fn f() {\n    let x = Helper::new();\n}";
        let (name, line, col) = candidate_names(body, 1)
            .into_iter()
            .find(|(n, _, _)| n == "Helper")
            .expect("Helper is a candidate");
        assert_eq!((name.as_str(), line), ("Helper", 2));
        assert_eq!(
            &"    let x = Helper::new();"[col as usize - 1..col as usize + 5],
            "Helper"
        );
    }

    #[test]
    fn decl_at_picks_the_innermost_declaration() {
        let decls = vec![
            Decl {
                name: "impl Thing".into(),
                kind: "class",
                start_line: 10,
                end_line: 60,
            },
            Decl {
                name: "run".into(),
                kind: "method",
                start_line: 20,
                end_line: 30,
            },
        ];
        assert_eq!(decl_at(&decls, 25).unwrap().name, "run");
        assert_eq!(decl_at(&decls, 15).unwrap().name, "impl Thing");
        assert!(decl_at(&decls, 90).is_none());
    }

    #[test]
    fn lines_of_is_inclusive_and_one_based() {
        let text = "a\nb\nc\nd";
        assert_eq!(lines_of(text, 2, 3), "b\nc");
        assert_eq!(lines_of(text, 1, 1), "a");
    }

    #[test]
    fn first_location_reads_both_location_shapes() {
        let plain = serde_json::json!([{ "uri": "file:///w/a.rs", "range": { "start": { "line": 4, "character": 0 } } }]);
        assert_eq!(
            first_location(&plain),
            Some(("file:///w/a.rs".to_string(), 5))
        );
        let link = serde_json::json!({ "targetUri": "file:///w/b.rs", "targetSelectionRange": { "start": { "line": 0, "character": 2 } } });
        assert_eq!(
            first_location(&link),
            Some(("file:///w/b.rs".to_string(), 1))
        );
        assert_eq!(first_location(&serde_json::json!([])), None);
    }

    #[test]
    fn report_counts_bytes_and_reduction() {
        let report = SliceReport {
            seed: "run".into(),
            items: vec![SliceItem {
                file: "src/a.rs".into(),
                name: "run".into(),
                kind: "function",
                start_line: 1,
                end_line: 3,
                depth: 0,
                because: None,
                text: "x".repeat(100),
            }],
            source_bytes: 1000,
            external: vec!["HashMap".into(), "HashMap".into()],
            truncated: 2,
        };
        assert_eq!(report.slice_bytes(), 100);
        assert!((report.reduction_percent() - 90.0).abs() < 0.001);
        let text = report.render();
        assert!(text.contains("90% smaller"), "{text}");
        assert!(text.contains("2 further item(s) omitted"), "{text}");
        assert!(text.contains("not followed: HashMap\n"), "{text}");
        assert!(text.contains("(the seed)"), "{text}");
    }
}
