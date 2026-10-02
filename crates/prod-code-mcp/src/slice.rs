//! Program slicing (roadmap 7.3): given a symbol, return the declarations its body names
//! instead of the files they happen to live in.
//!
//! The slice is built from what the analyzer already knows, so it needs no new wire message:
//! `textDocument/documentSymbol` gives every item's range in a file, so an item's source text
//! is a range of lines; `textDocument/definition` resolves each name a body mentions (a
//! function, a type, a constant, a trait) to the item that declares it. Starting from the seed
//! item, the slicer walks those edges breadth-first to a depth limit and returns the collected
//! items, grouped by file, in file order. It is a declaration-level slice by name resolution:
//! whole items a body names, not a data-flow slice of the statements that affect a value.
//!
//! Names are found by scanning the body for identifier-shaped tokens and asking the analyzer
//! about each distinct one, at its position in UTF-16 code units (the protocol's default). A
//! null or empty answer is the analyzer saying the name is a keyword, a literal or resolves
//! nowhere, and an answer inside the body itself is a local; both are ordinary and dropped.
//!
//! The slice says what it could not establish. A definition query the analyzer failed, an
//! answer that is not a location, a target file that cannot be read or whose symbols cannot be
//! listed, and a body naming more names than are resolved per item are gaps: each is named and
//! the report is incomplete. A walk in which the analyzer gave no usable answer to any
//! definition query is an error rather than a slice of the seed alone. The depth limit and the
//! byte budget are the caller's bounds, and the report says where they cut the walk: such a
//! slice has complete evidence but is bounded, not the whole dependency closure.
//!
//! Coordinates are checked against the file they point into. A location's URI must be an
//! absolute `file:` URI of a local path without query or fragment, or another scheme, which is a
//! source outside the workspace the slicer cannot read; anything else is malformed. A position
//! must lie on a line of the file, at most at the end of that line and not between the two
//! UTF-16 units of one character; a line ends at `\n`, and a `\r` before it is not part of the
//! line, as in the Rust engine's own line index.

use crate::session::LspSession;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// How many bytes of slice to return before stopping, unless the caller says otherwise.
pub const DEFAULT_MAX_BYTES: usize = 24 * 1024;
/// How far to follow dependencies from the seed by default.
pub const DEFAULT_DEPTH: u32 = 2;
/// Distinct names resolved per item; a body that mentions more is reported as a gap.
const MAX_NAMES_PER_ITEM: usize = 64;
/// Names from outside the workspace listed in the report before it says "and N more".
const EXTERNAL_SHOWN: usize = 12;
/// Gaps listed in the report before it says "and N more".
const GAPS_SHOWN: usize = 20;

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

/// Why a dependency of an item has no usable evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapKind {
    /// The analyzer failed or refused a definition query.
    QueryFailed,
    /// A definition answer that is neither null, a location nor a list of locations, or one
    /// whose coordinates do not fit.
    Malformed,
    /// A definition in the workspace whose file cannot be read or whose symbols cannot be listed.
    Unreadable,
    /// A body that names more distinct names than are resolved per item.
    NameLimit,
}

impl GapKind {
    fn label(self) -> &'static str {
        match self {
            GapKind::QueryFailed => "definition query failed",
            GapKind::Malformed => "malformed definition answer",
            GapKind::Unreadable => "target cannot be sliced",
            GapKind::NameLimit => "name limit",
        }
    }
}

/// One dependency lookup that has no usable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceGap {
    pub kind: GapKind,
    /// The item whose body was being resolved.
    pub item: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct SliceReport {
    pub seed: String,
    pub items: Vec<SliceItem>,
    /// Optional intra-function data-flow and control-dependency slice of the seed.
    pub dataflow_slice: Option<crate::dataflow::DataFlowSlice>,
    /// Bytes of the files the slice draws from.
    pub source_bytes: usize,
    /// Names that resolved outside the workspace (std, crates.io) and were not followed.
    pub external: Vec<String>,
    /// Names that resolved to a URI of another scheme than `file:` (a class in a jar, a
    /// generated document), as `name (scheme:)`; outside the workspace and not readable.
    pub unsupported: Vec<String>,
    /// Names that resolved in the workspace outside any declaration the slicer includes (a
    /// module, a field of no listed item), as `name (file:line)`; not followed.
    pub unsliced: Vec<String>,
    /// Lookups without usable evidence: while any is here, the slice may miss items.
    pub gaps: Vec<SliceGap>,
    /// The depth the walk was asked to follow.
    pub depth_limit: u32,
    /// Items at the depth limit, whose own dependencies were not looked up.
    pub unexpanded: usize,
    /// The byte budget the walk was given.
    pub max_bytes: usize,
    /// The seed alone exceeds the budget; it is returned whole.
    pub seed_over_budget: bool,
    /// Queued items left out because the budget ran out; their dependencies were not looked up.
    pub truncated: usize,
}

/// Configuration options for program slicing.
#[derive(Debug, Clone)]
pub struct SliceOptions {
    pub depth: u32,
    pub max_bytes: usize,
    pub dataflow: bool,
    pub target_line: Option<u32>,
    pub target_var: Option<String>,
}

impl Default for SliceOptions {
    fn default() -> Self {
        Self {
            depth: DEFAULT_DEPTH,
            max_bytes: DEFAULT_MAX_BYTES,
            dataflow: false,
            target_line: None,
            target_var: None,
        }
    }
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

    /// Whether every dependency lookup of the walk had a usable answer.
    pub fn has_complete_evidence(&self) -> bool {
        self.gaps.is_empty()
    }

    /// Whether the walk stopped before the whole dependency closure: at the depth limit, at the
    /// byte budget, or at a target in the workspace outside any declaration it slices.
    pub fn is_bounded(&self) -> bool {
        self.unexpanded > 0 || self.truncated > 0 || !self.unsliced.is_empty()
    }

    /// Whether the slice is the seed's whole dependency closure in the workspace: every lookup
    /// answered and nothing left behind a bound. Names outside the workspace are never followed
    /// and do not count against it.
    pub fn is_complete(&self) -> bool {
        self.has_complete_evidence() && !self.is_bounded()
    }

    pub fn render(&self) -> String {
        let mut out = if self.is_complete() {
            format!(
                "slice of `{}`: {} item(s), {} bytes from {} bytes of source ({:.0}% smaller)\n",
                self.seed,
                self.items.len(),
                self.slice_bytes(),
                self.source_bytes,
                self.reduction_percent()
            )
        } else if self.has_complete_evidence() {
            format!(
                "BOUNDED slice of `{}`: {} item(s), {} bytes from {} bytes of source ({:.0}% \
                 smaller); every dependency lookup was answered, but the walk stopped at the \
                 bounds below, so it is not the whole dependency closure\n",
                self.seed,
                self.items.len(),
                self.slice_bytes(),
                self.source_bytes,
                self.reduction_percent()
            )
        } else {
            format!(
                "INCOMPLETE slice of `{}`: {} item(s), {} bytes from {} bytes of source; {} \
                 dependency lookup(s) have no usable answer, so items may be missing and no \
                 reduction is claimed\n",
                self.seed,
                self.items.len(),
                self.slice_bytes(),
                self.source_bytes,
                self.gaps.len()
            )
        };
        if self.seed_over_budget {
            out.push_str(&format!(
                "the seed alone is {} bytes, over the byte budget of {} bytes; it is returned whole\n",
                self.items.first().map_or(0, |i| i.text.len()),
                self.max_bytes
            ));
        }
        if self.truncated > 0 {
            out.push_str(&format!(
                "byte budget of {} bytes reached: {} queued item(s) left out, and their own \
                 dependencies were not looked up\n",
                self.max_bytes, self.truncated
            ));
        }
        if self.unexpanded > 0 {
            out.push_str(&format!(
                "depth limit {} reached: the dependencies of {} item(s) at depth {} were not \
                 looked up\n",
                self.depth_limit, self.unexpanded, self.depth_limit
            ));
        }
        push_list(
            &mut out,
            "outside the workspace, not followed",
            &self.external,
        );
        push_list(
            &mut out,
            "outside the workspace in a source of an unsupported URI scheme, not followed",
            &self.unsupported,
        );
        push_list(
            &mut out,
            "outside any declaration the slicer includes, not followed",
            &self.unsliced,
        );
        if !self.gaps.is_empty() {
            out.push_str("missing evidence:\n");
            for gap in self.gaps.iter().take(GAPS_SHOWN) {
                out.push_str(&format!(
                    "  - {} in `{}`: {}\n",
                    gap.kind.label(),
                    gap.item,
                    gap.detail
                ));
            }
            if self.gaps.len() > GAPS_SHOWN {
                out.push_str(&format!("  - and {} more\n", self.gaps.len() - GAPS_SHOWN));
            }
        }
        if let Some(ref df) = self.dataflow_slice {
            out.push('\n');
            out.push_str(&df.formatted_slice);
            out.push('\n');
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
                    (_, 0) if self.dataflow_slice.is_some() => " (the seed, data-flow sliced)".to_string(),
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

/// `title: a, b, c, and N more`, sorted and deduplicated; nothing when `names` is empty.
fn push_list(out: &mut String, title: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let mut names = names.to_vec();
    names.sort();
    names.dedup();
    let shown = names.len().min(EXTERNAL_SHOWN);
    let more = names.len() - shown;
    out.push_str(&format!(
        "{title}: {}{}\n",
        names[..shown].join(", "),
        if more > 0 {
            format!(", and {more} more")
        } else {
            String::new()
        }
    ));
}

/// A 0-based protocol position; the character counts UTF-16 code units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Pos {
    line: u32,
    character: u32,
}

/// A range whose end is not before its start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Span {
    start: Pos,
    end: Pos,
}

impl Span {
    fn contains(&self, pos: Pos) -> bool {
        self.start <= pos && pos <= self.end
    }

    fn lines(&self) -> u32 {
        self.end.line - self.start.line
    }
}

/// An item declared in a file, as `textDocument/documentSymbol` reports it.
#[derive(Debug, Clone)]
struct Decl {
    name: String,
    kind: &'static str,
    range: Span,
    /// Where its name is; the whole range for a flat `SymbolInformation`.
    selection: Span,
}

impl Decl {
    /// 1-based; coordinates are below `u32::MAX`, so this cannot overflow.
    fn start_line(&self) -> u32 {
        self.range.start.line + 1
    }

    fn end_line(&self) -> u32 {
        self.range.end.line + 1
    }
}

/// What kind of JSON value this is, for a message about an answer of the wrong shape.
fn shape(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "a list",
        serde_json::Value::Object(_) => "an object",
    }
}

/// A line or character: a non-negative integer small enough that its 1-based form fits a `u32`.
fn coordinate(value: &serde_json::Value, key: &str) -> Result<u32, String> {
    let raw = value
        .get(key)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| format!("`{key}` is not a non-negative integer"))?;
    u32::try_from(raw)
        .ok()
        .filter(|v| *v < u32::MAX)
        .ok_or_else(|| format!("`{key}` {raw} is out of range"))
}

fn parse_pos(value: Option<&serde_json::Value>) -> Result<Pos, String> {
    let value = value.ok_or("a position is missing")?;
    Ok(Pos {
        line: coordinate(value, "line")?,
        character: coordinate(value, "character")?,
    })
}

fn parse_symbol_span(value: Option<&serde_json::Value>) -> Result<Span, String> {
    let value = value.ok_or("a range is missing")?;
    let mut start = parse_pos(value.get("start"))?;
    let mut end = parse_pos(value.get("end"))?;
    if end < start {
        std::mem::swap(&mut start, &mut end);
    }
    Ok(Span { start, end })
}

fn parse_span(value: Option<&serde_json::Value>) -> Result<Span, String> {
    let value = value.ok_or("a range is missing")?;
    let span = Span {
        start: parse_pos(value.get("start"))?,
        end: parse_pos(value.get("end"))?,
    };
    if span.end < span.start {
        return Err("a range ends before it starts".to_string());
    }
    Ok(span)
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

/// The declarations of a `textDocument/documentSymbol` answer: null is a file with none, a list
/// is flattened with nested items (methods in an impl) as their own entries, and anything else,
/// a symbol without a name or kind, or a sliceable one without a valid range, is malformed. The
/// ranges of symbols the slicer does not slice are never used, so they are not judged: the real
/// Rust engine answers `mod name;` with a range that ends before it starts.
fn parse_decls(answer: &serde_json::Value) -> Result<Vec<Decl>, String> {
    let mut decls = Vec::new();
    match answer {
        serde_json::Value::Null => {}
        serde_json::Value::Array(symbols) => collect_decls(symbols, &mut decls)?,
        other => {
            return Err(format!(
                "expected a list of symbols or null, got {}",
                shape(other)
            ));
        }
    }
    Ok(decls)
}

fn collect_decls(symbols: &[serde_json::Value], out: &mut Vec<Decl>) -> Result<(), String> {
    for sym in symbols {
        let name = sym
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| format!("a symbol without a name: {}", shape(sym)))?;
        let kind = sym
            .get("kind")
            .and_then(|k| k.as_u64())
            .ok_or_else(|| format!("symbol `{name}` has no kind"))?;
        if is_sliceable(kind) && !name.is_empty() {
            let range = parse_symbol_span(
                sym.get("range")
                    .or_else(|| sym.get("location").and_then(|l| l.get("range"))),
            )
            .map_err(|e| format!("symbol `{name}`: {e}"))?;
            let selection = match sym.get("selectionRange") {
                Some(sel) => parse_symbol_span(Some(sel)).map_err(|e| format!("symbol `{name}`: {e}"))?,
                None => range,
            };
            out.push(Decl {
                name: name.to_string(),
                kind: kind_name(kind),
                range,
                selection,
            });
        }
        match sym.get("children") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(children)) => collect_decls(children, out)?,
            Some(other) => {
                return Err(format!(
                    "symbol `{name}` has children that are {}, not a list",
                    shape(other)
                ));
            }
        }
    }
    Ok(())
}

/// Identifier-shaped tokens of a body, in order, deduplicated, with the 1-based line and the
/// 1-based column in UTF-16 code units of the first occurrence of each. Words inside line
/// comments and string literals are skipped: they cost a round trip and never resolve to
/// anything useful.
pub fn candidate_names(text: &str, start_line: u32) -> Vec<(String, u32, u32)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let Some(line) = u32::try_from(i)
            .ok()
            .and_then(|i| start_line.checked_add(i))
        else {
            break;
        };
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
                // `strip_noise` keeps byte offsets, so `start` is the word's offset in `raw`.
                let Some(utf16) = u32::try_from(raw[..start].encode_utf16().count())
                    .ok()
                    .and_then(|c| c.checked_add(1))
                else {
                    continue;
                };
                out.push((word.to_string(), line, utf16));
            }
        }
    }
    out
}

/// Blanks out line comments and string literals so their words are not resolved. Every
/// character is replaced by as many spaces as it has bytes, so offsets stay those of `line`.
fn strip_noise(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let blank = |out: &mut String, c: char| out.push_str(&" ".repeat(c.len_utf8()));
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_string = !in_string;
                out.push(' ');
            }
            '\\' if in_string => {
                out.push(' ');
                if let Some(next) = chars.next() {
                    blank(&mut out, next);
                }
            }
            '/' if !in_string && chars.peek() == Some(&'/') => {
                out.push_str(&" ".repeat(line.len() - out.len()));
                break;
            }
            '#' if !in_string => {
                out.push_str(&" ".repeat(line.len() - out.len()));
                break;
            }
            _ if in_string => blank(&mut out, c),
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

/// The byte range of each line of `text` without its line break: lines end at `\n`, and a `\r`
/// before it belongs to the break. A text ending in a line break has an empty last line, the
/// place a position just past the final break points at.
fn line_bounds(text: &str) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (at, _) in text.match_indices('\n') {
        let end = if text[start..at].ends_with('\r') {
            at - 1
        } else {
            at
        };
        lines.push(start..end);
        start = at + 1;
    }
    lines.push(start..text.len());
    lines
}

/// Whether the source holds `pos`: its line exists, and its character, in UTF-16 code units, is
/// at most the line's length and falls between characters, not inside a surrogate pair. The
/// message counts lines and columns from 1, as the report does.
fn check_pos(text: &str, lines: &[Range<usize>], pos: Pos) -> Result<(), String> {
    let (line, character) = (pos.line as usize, u64::from(pos.character));
    let at = format!("position {}:{}", u64::from(pos.line) + 1, character + 1);
    let bounds = lines.get(line).ok_or_else(|| {
        format!(
            "{at} is past the last line of the source, which has {} line(s)",
            lines.len()
        )
    })?;
    let mut units = 0u64;
    for c in text[bounds.clone()].chars() {
        if units >= character {
            break;
        }
        units += c.len_utf16() as u64;
    }
    match units.cmp(&character) {
        std::cmp::Ordering::Equal => Ok(()),
        std::cmp::Ordering::Greater => {
            Err(format!("{at} splits a surrogate pair on line {}", line + 1))
        }
        std::cmp::Ordering::Less => Err(format!(
            "{at} is past the end of line {}, which is {units} UTF-16 unit(s) long",
            line + 1
        )),
    }
}

fn check_span(text: &str, lines: &[Range<usize>], span: Span) -> Result<(), String> {
    check_pos(text, lines, span.start)?;
    check_pos(text, lines, span.end)
}

/// Text of lines `start..=end` (1-based, inclusive), joined by `\n`; `None` when the source
/// has no such lines.
fn lines_of(text: &str, lines: &[Range<usize>], start: u32, end: u32) -> Option<String> {
    let first = start.checked_sub(1)? as usize;
    let last = end.checked_sub(1)? as usize;
    if first > last {
        return None;
    }
    Some(
        lines
            .get(first..=last)?
            .iter()
            .map(|r| &text[r.clone()])
            .collect::<Vec<_>>()
            .join("\n"),
    )
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
    lines: Vec<Range<usize>>,
    decls: Vec<Decl>,
}

impl FileFacts {
    fn check(&self, span: Span) -> Result<(), String> {
        check_span(&self.text, &self.lines, span)
    }
}

/// Reads a file and lists its declarations. A declaration whose range or name the file cannot
/// hold makes the answer malformed, like a declaration without a range: slicing it would cut
/// lines that are not there.
async fn file_facts(session: &mut LspSession, file: &Path) -> Result<FileFacts> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(_) => {
            let (bytes, _) = crate::remote_fs::read_source(
                session.remote(),
                session.root(),
                &file.to_string_lossy(),
            )
            .await
            .with_context(|| format!("cannot read {}", file.display()))?;
            String::from_utf8(bytes).or_else(|e| {
                Ok::<String, anyhow::Error>(String::from_utf8_lossy(&e.into_bytes()).into_owned())
            })?
        }
    };
    let uri = session.uri_for(file)?;
    let symbols = session
        .query(
            file,
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await?;
    let lines = line_bounds(&text);
    let decls = parse_decls(&symbols)
        .and_then(|decls| {
            for decl in &decls {
                check_span(&text, &lines, decl.range)
                    .and_then(|()| check_span(&text, &lines, decl.selection))
                    .map_err(|e| format!("symbol `{}`: {e}", decl.name))?;
            }
            Ok(decls)
        })
        .map_err(|e| {
            anyhow::anyhow!(
                "malformed textDocument/documentSymbol answer for {}: {e}",
                file.display()
            )
        })?;
    Ok(FileFacts { text, lines, decls })
}

/// The declaration at `pos`: the innermost one whose name is there, else the smallest one
/// spanning its line, preferring one whose range covers the position itself.
fn decl_at(decls: &[Decl], pos: Pos) -> Option<&Decl> {
    decls
        .iter()
        .filter(|d| d.selection.contains(pos))
        .min_by_key(|d| d.range.lines())
        .or_else(|| {
            decls
                .iter()
                .filter(|d| d.range.start.line <= pos.line && pos.line <= d.range.end.line)
                .min_by_key(|d| (d.range.lines(), !d.range.contains(pos)))
        })
}

/// What a location's URI names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// A local file.
    File(PathBuf),
    /// A document of another scheme, such as a class inside a jar: outside the workspace and
    /// not a file the slicer can read.
    Other { scheme: String },
}

/// A place a definition answer points at: the source and the range the answer gives there.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    source: Source,
    range: Span,
}

/// Every location of a `textDocument/definition` answer. Null and an empty list are the
/// analyzer's "nothing here"; a `Location`, a `LocationLink` or a list of them is followed in
/// full; anything else is malformed.
fn parse_locations(answer: &serde_json::Value) -> Result<Vec<Target>, String> {
    match answer {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(items) => items.iter().map(parse_location).collect(),
        serde_json::Value::Object(_) => parse_location(answer).map(|t| vec![t]),
        other => Err(format!(
            "expected a location, a list of locations or null, got {}",
            shape(other)
        )),
    }
}

fn parse_location(value: &serde_json::Value) -> Result<Target, String> {
    let (uri, range) = match value.get("targetUri") {
        Some(uri) => (
            uri,
            value
                .get("targetSelectionRange")
                .or_else(|| value.get("targetRange")),
        ),
        None => (
            value
                .get("uri")
                .ok_or_else(|| format!("{} without `uri` or `targetUri`", shape(value)))?,
            value.get("range"),
        ),
    };
    let uri = uri.as_str().ok_or("a location's uri is not a string")?;
    Ok(Target {
        source: parse_source(uri)?,
        range: parse_span(range)?,
    })
}

/// The source a location's URI names. It must be an absolute URI. A `file:` URI must spell an
/// absolute path (`file:///a.rs`, `file:/a.rs`, `file://localhost/a.rs`) of a local file, with
/// no query or fragment; the URL parser would otherwise read `file:a.rs` as `/a.rs` and drop a
/// query, turning a malformed answer into a real-looking path.
fn parse_source(uri: &str) -> Result<Source, String> {
    let url =
        url::Url::parse(uri).map_err(|e| format!("uri `{uri}` is not an absolute URI: {e}"))?;
    if url.scheme() != "file" {
        if url.path().is_empty() {
            return Err(format!("uri `{uri}` names nothing after its scheme"));
        }
        return Ok(Source::Other {
            scheme: url.scheme().to_string(),
        });
    }
    let spelled_absolute = uri
        .get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case("file:/"));
    if !spelled_absolute {
        return Err(format!("uri `{uri}` is not an absolute file URI"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(format!(
            "file uri `{uri}` has a query or fragment, which no file path has"
        ));
    }
    if url.path().ends_with('/') {
        return Err(format!("file uri `{uri}` names a directory, not a file"));
    }
    url.to_file_path()
        .map(Source::File)
        .map_err(|()| format!("file uri `{uri}` does not name a path on this machine"))
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
    slice_with_options(
        remote,
        root,
        seed_file,
        seed_line,
        seed_col,
        SliceOptions {
            depth,
            max_bytes,
            dataflow: false,
            target_line: None,
            target_var: None,
        },
    )
    .await
}

/// Builds the slice with customized slicing options (including intra-function data-flow).
pub async fn slice_with_options(
    remote: SocketAddr,
    root: &Path,
    seed_file: &Path,
    seed_line: u32,
    seed_col: u32,
    options: SliceOptions,
) -> Result<SliceReport> {
    let mut session = LspSession::open(remote, root, Some(seed_file)).await?;
    let result = slice_with(
        &mut session,
        root,
        seed_file,
        seed_line,
        seed_col,
        options,
    )
    .await;
    session.close().await;
    result
}

/// An item waiting to be sliced: its file, its declaration, its depth and who named it.
type Queued = (PathBuf, Decl, u32, Option<String>);

async fn slice_with(
    session: &mut LspSession,
    root: &Path,
    seed_file: &Path,
    seed_line: u32,
    seed_col: u32,
    options: SliceOptions,
) -> Result<SliceReport> {
    let (Some(line), Some(character)) = (seed_line.checked_sub(1), seed_col.checked_sub(1)) else {
        anyhow::bail!(
            "seed position {seed_line}:{seed_col} is not 1-based: line and character start at 1"
        );
    };
    let seed_pos = Pos { line, character };
    let mut facts: BTreeMap<PathBuf, FileFacts> = BTreeMap::new();
    facts.insert(
        seed_file.to_path_buf(),
        file_facts(session, seed_file).await?,
    );
    // A column past the line would still find the declaration spanning the line; a caller that
    // only knows the line gives column 1, which every line holds.
    if let Err(e) = check_pos(&facts[seed_file].text, &facts[seed_file].lines, seed_pos) {
        anyhow::bail!(
            "no declaration at {}:{seed_line}:{seed_col}: the seed {e}",
            relative(root, seed_file)
        );
    }

    let seed_decl = decl_at(&facts[seed_file].decls, seed_pos)
        .cloned()
        .with_context(|| {
            format!(
                "no declaration at {}:{seed_line}",
                relative(root, seed_file)
            )
        })?;

    let mut report = SliceReport {
        seed: seed_decl.name.clone(),
        depth_limit: options.depth,
        max_bytes: options.max_bytes,
        ..Default::default()
    };
    // Files that could not be read or listed, with why, so each is tried once.
    let mut unreadable: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut queued: HashSet<(PathBuf, Pos)> = HashSet::new();
    let mut queue: VecDeque<Queued> = VecDeque::new();
    queued.insert((seed_file.to_path_buf(), seed_decl.range.start));
    queue.push_back((seed_file.to_path_buf(), seed_decl, 0, None));

    // Definition queries asked, and those with a well-formed answer.
    let (mut asked, mut answered) = (0usize, 0usize);
    let mut bytes = 0usize;
    while let Some((file, decl, item_depth, because)) = queue.pop_front() {
        let text = {
            let f = &facts[&file];
            // `file_facts` checked every declaration's range against the file.
            lines_of(&f.text, &f.lines, decl.start_line(), decl.end_line()).with_context(|| {
                format!(
                    "`{}` at {}:{}-{} is outside its file",
                    decl.name,
                    relative(root, &file),
                    decl.start_line(),
                    decl.end_line()
                )
            })?
        };
        let rel = relative(root, &file);

        let (display_text, names_source_text) = if options.dataflow && item_depth == 0 {
            let target_line = options.target_line.or(if seed_line >= decl.start_line() && seed_line <= decl.end_line() {
                Some(seed_line)
            } else {
                None
            });
            let df = crate::dataflow::slice_intra_function(
                &facts[&file].text,
                decl.start_line(),
                decl.end_line(),
                &decl.name,
                &rel,
                target_line,
                options.target_var.as_deref(),
            );
            let formatted = df.formatted_slice.clone();
            report.dataflow_slice = Some(df);
            (formatted.clone(), formatted)
        } else {
            (text.clone(), text.clone())
        };

        if bytes.saturating_add(display_text.len()) > options.max_bytes {
            if report.items.is_empty() {
                report.seed_over_budget = true;
            } else {
                report.truncated = 1 + queue.len();
                break;
            }
        }
        bytes = bytes.saturating_add(display_text.len());
        report.items.push(SliceItem {
            file: rel.clone(),
            name: decl.name.clone(),
            kind: decl.kind,
            start_line: decl.start_line(),
            end_line: decl.end_line(),
            depth: item_depth,
            because,
            text: display_text,
        });
        if item_depth >= options.depth {
            report.unexpanded += 1;
            continue;
        }

        let names = candidate_names(&names_source_text, decl.start_line());
        if names.len() > MAX_NAMES_PER_ITEM {
            report.gaps.push(SliceGap {
                kind: GapKind::NameLimit,
                item: decl.name.clone(),
                detail: format!(
                    "its body names {} distinct names; only the first {MAX_NAMES_PER_ITEM} were \
                     resolved, and the other {} (from `{}` on) were not looked up",
                    names.len(),
                    names.len() - MAX_NAMES_PER_ITEM,
                    names[MAX_NAMES_PER_ITEM].0
                ),
            });
        }
        let uri = session.uri_for(&file)?;
        for (name, line, col) in names.into_iter().take(MAX_NAMES_PER_ITEM) {
            let at = format!("`{name}` at {rel}:{line}:{col}");
            // Candidates are 1-based by construction.
            let params = serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            });
            asked += 1;
            let found = match session
                .query(&file, "textDocument/definition", params)
                .await
            {
                Ok(found) => found,
                Err(e) => {
                    report.gaps.push(SliceGap {
                        kind: GapKind::QueryFailed,
                        item: decl.name.clone(),
                        detail: format!("{at}: {e:#}"),
                    });
                    continue;
                }
            };
            let targets = match parse_locations(&found) {
                Ok(targets) => targets,
                Err(e) => {
                    report.gaps.push(SliceGap {
                        kind: GapKind::Malformed,
                        item: decl.name.clone(),
                        detail: format!("{at}: {e}"),
                    });
                    continue;
                }
            };
            // An answer is usable when it is empty or at least one of its targets is not
            // malformed evidence.
            let mut usable = targets.is_empty();
            for target in targets {
                let path = match target.source {
                    Source::File(path) => path,
                    Source::Other { scheme } => {
                        usable = true;
                        report.unsupported.push(format!("{name} ({scheme}:)"));
                        continue;
                    }
                };
                if crate::remote_fs::is_external(root, &path.to_string_lossy()) {
                    usable = true;
                    report.external.push(name.clone());
                    continue;
                }
                // Coordinates are below `u32::MAX`, so the 1-based line fits.
                let there = format!("{}:{}", relative(root, &path), target.range.start.line + 1);
                if !facts.contains_key(&path) {
                    let why = match unreadable.get(&path) {
                        Some(why) => Some(why.clone()),
                        None => match file_facts(session, &path).await {
                            Ok(f) => {
                                facts.insert(path.clone(), f);
                                None
                            }
                            Err(e) => {
                                let why = format!("{e:#}");
                                unreadable.insert(path.clone(), why.clone());
                                Some(why)
                            }
                        },
                    };
                    if let Some(why) = why {
                        usable = true;
                        report.gaps.push(SliceGap {
                            kind: GapKind::Unreadable,
                            item: decl.name.clone(),
                            detail: format!("{at} resolves to {there}: {why}"),
                        });
                        continue;
                    }
                }
                if let Err(e) = facts[&path].check(target.range) {
                    report.gaps.push(SliceGap {
                        kind: GapKind::Malformed,
                        item: decl.name.clone(),
                        detail: format!("{at} resolves to {there}, but its {e}"),
                    });
                    continue;
                }
                usable = true;
                let Some(target_decl) = decl_at(&facts[&path].decls, target.range.start).cloned()
                else {
                    report.unsliced.push(format!("{name} ({there})"));
                    continue;
                };
                // A name that resolves inside the item we are already looking at is a local.
                if path == file && target_decl.range == decl.range {
                    continue;
                }
                if queued.insert((path.clone(), target_decl.range.start)) {
                    queue.push_back((path, target_decl, item_depth + 1, Some(decl.name.clone())));
                }
            }
            if usable {
                answered += 1;
            }
        }
    }
    if asked > 0 && answered == 0 {
        let first = report
            .gaps
            .iter()
            .find(|g| matches!(g.kind, GapKind::QueryFailed | GapKind::Malformed))
            .map(|g| g.detail.clone())
            .unwrap_or_default();
        anyhow::bail!(
            "no dependency evidence for `{}`: the analyzer gave no usable answer to any of its \
             {asked} definition queries, so a slice would be the seed alone; first: {first}",
            report.seed
        );
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

    fn span(from: (u32, u32), to: (u32, u32)) -> Span {
        Span {
            start: Pos {
                line: from.0,
                character: from.1,
            },
            end: Pos {
                line: to.0,
                character: to.1,
            },
        }
    }

    fn decl(name: &str, range: Span, selection: Span) -> Decl {
        Decl {
            name: name.into(),
            kind: "function",
            range,
            selection,
        }
    }

    fn at(line: u32, character: u32) -> Pos {
        Pos { line, character }
    }

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
    fn candidate_columns_count_utf16_units_after_wide_characters() {
        // `é` is one UTF-16 unit and two bytes, `😀` two units and four bytes; an escaped
        // quote and a comment with a wide character must not shift anything either.
        let body = "let s = \"é\\\"😀\"; helper(ünit); // ☃ tail";
        let names = candidate_names(body, 7);
        let helper = names.iter().find(|(n, _, _)| n == "helper").unwrap();
        let expected = body[..body.find("helper").unwrap()].encode_utf16().count() as u32 + 1;
        assert_eq!((helper.1, helper.2), (7, expected));
        assert!(!names.iter().any(|(n, _, _)| n == "tail"));
        let nit = names.iter().find(|(n, _, _)| n == "nit").unwrap();
        let expected = body[..body.find("nit").unwrap()].encode_utf16().count() as u32 + 1;
        assert_eq!(nit.2, expected);
    }

    #[test]
    fn strip_noise_keeps_byte_offsets() {
        for line in [
            "a \"é😀\\\"x\" b",
            "x // ☃",
            "#[derive(Ü)]",
            "\"unterminated \\",
        ] {
            assert_eq!(strip_noise(line).len(), line.len(), "{line}");
        }
    }

    #[test]
    fn candidate_lines_stop_before_overflowing() {
        let names = candidate_names("alpha\nbeta\ngamma", u32::MAX - 1);
        let lines: Vec<u32> = names.iter().map(|(_, l, _)| *l).collect();
        assert_eq!(lines, vec![u32::MAX - 1, u32::MAX]);
    }

    #[test]
    fn decl_at_picks_the_innermost_declaration() {
        let decls = vec![
            decl("impl Thing", span((9, 0), (59, 1)), span((9, 5), (9, 10))),
            decl("run", span((19, 4), (29, 5)), span((19, 7), (19, 10))),
        ];
        assert_eq!(decl_at(&decls, at(24, 0)).unwrap().name, "run");
        assert_eq!(decl_at(&decls, at(14, 0)).unwrap().name, "impl Thing");
        // A position before the method's first character, on its line, is still the method.
        assert_eq!(decl_at(&decls, at(19, 0)).unwrap().name, "run");
        assert!(decl_at(&decls, at(89, 0)).is_none());
    }

    #[test]
    fn decl_at_prefers_the_name_then_the_covering_range_on_a_shared_line() {
        let decls = vec![
            decl("alpha", span((3, 0), (3, 13)), span((3, 3), (3, 8))),
            decl("beta", span((3, 14), (3, 26)), span((3, 17), (3, 21))),
        ];
        assert_eq!(decl_at(&decls, at(3, 18)).unwrap().name, "beta");
        assert_eq!(decl_at(&decls, at(3, 24)).unwrap().name, "beta");
        assert_eq!(decl_at(&decls, at(3, 4)).unwrap().name, "alpha");
    }

    #[test]
    fn lines_of_is_inclusive_and_one_based() {
        let text = "a\nb\r\nc\nd";
        let lines = line_bounds(text);
        assert_eq!(lines_of(text, &lines, 2, 3).as_deref(), Some("b\nc"));
        assert_eq!(lines_of(text, &lines, 1, 1).as_deref(), Some("a"));
        assert_eq!(lines_of(text, &lines, 4, 4).as_deref(), Some("d"));
        // Lines the text does not have are refused, not cut to the ones it has.
        assert_eq!(lines_of(text, &lines, 3, 5), None);
        assert_eq!(lines_of(text, &lines, 0, 1), None);
        assert_eq!(lines_of(text, &lines, 3, 2), None);
    }

    #[test]
    fn line_bounds_leave_the_line_break_out_and_keep_a_final_empty_line() {
        let text = "ab\r\n\ncd\r\n";
        let lines: Vec<&str> = line_bounds(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(lines, vec!["ab", "", "cd", ""]);
        // A lone `\r` is not a line break, as in the Rust engine's line index.
        let lone = "a\rb";
        assert_eq!(line_bounds(lone), vec![0..3]);
        assert_eq!(line_bounds(""), vec![0..0]);
    }

    #[test]
    fn check_pos_counts_utf16_units_and_refuses_what_the_source_cannot_hold() {
        let text = "é😀x\r\nfn\n";
        let lines = line_bounds(text);
        // `é` is one unit, `😀` two, `x` one: the first line is 4 units long.
        for character in [0, 1, 3, 4] {
            assert_eq!(
                check_pos(text, &lines, at(0, character)),
                Ok(()),
                "{character}"
            );
        }
        let err = |line, character| check_pos(text, &lines, at(line, character)).unwrap_err();
        assert!(err(0, 2).contains("splits a surrogate pair on line 1"));
        // Unit 5 would be between `\r` and `\n`.
        assert!(err(0, 5).contains("past the end of line 1, which is 4 UTF-16 unit(s) long"));
        assert!(err(0, u32::MAX - 1).contains("past the end of line 1"));
        assert_eq!(
            check_pos(text, &lines, at(2, 0)),
            Ok(()),
            "after the final break"
        );
        assert!(err(2, 1).contains("past the end of line 3"));
        assert!(
            err(3, 0)
                .contains("position 4:1 is past the last line of the source, which has 3 line(s)")
        );
        assert_eq!(
            check_span(text, &lines, span((0, 1), (1, 2))),
            Ok(()),
            "a span inside the text"
        );
        assert!(
            check_span(text, &lines, span((0, 1), (1, 3)))
                .unwrap_err()
                .contains("line 2")
        );
    }

    fn file(path: &str) -> Source {
        Source::File(PathBuf::from(path))
    }

    #[test]
    fn parse_locations_reads_every_location_of_both_shapes() {
        let plain = serde_json::json!([
            { "uri": "file:///w/a.rs", "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 4, "character": 3 } } },
            { "uri": "file:///w/c%20d.rs", "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 3 } } }
        ]);
        assert_eq!(
            parse_locations(&plain).unwrap(),
            vec![
                Target {
                    source: file("/w/a.rs"),
                    range: span((4, 0), (4, 3))
                },
                Target {
                    source: file("/w/c d.rs"),
                    range: span((1, 2), (1, 3))
                }
            ]
        );
        let link = serde_json::json!({ "targetUri": "file:///w/b.rs", "targetRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 3, "character": 1 } }, "targetSelectionRange": { "start": { "line": 0, "character": 2 }, "end": { "line": 0, "character": 5 } } });
        assert_eq!(
            parse_locations(&link).unwrap(),
            vec![Target {
                source: file("/w/b.rs"),
                range: span((0, 2), (0, 5))
            }]
        );
        assert_eq!(parse_locations(&serde_json::json!([])).unwrap(), vec![]);
        assert_eq!(parse_locations(&serde_json::Value::Null).unwrap(), vec![]);
    }

    #[test]
    fn parse_source_takes_local_file_uris_and_names_other_schemes() {
        for (uri, path) in [
            ("file:///w/a.rs", "/w/a.rs"),
            ("file:/w/a.rs", "/w/a.rs"),
            ("FILE:///w/a.rs", "/w/a.rs"),
            ("file://localhost/w/a.rs", "/w/a.rs"),
        ] {
            assert_eq!(parse_source(uri), Ok(file(path)), "{uri}");
        }
        for (uri, scheme) in [
            ("jdt://contents/rt.jar/java.lang/String.class", "jdt"),
            ("untitled:Untitled-1", "untitled"),
        ] {
            assert_eq!(
                parse_source(uri),
                Ok(Source::Other {
                    scheme: scheme.into()
                }),
                "{uri}"
            );
        }
        for (uri, why) in [
            ("", "not an absolute URI"),
            ("src/a.rs", "not an absolute URI"),
            ("/w/a.rs", "not an absolute URI"),
            ("file:a.rs", "not an absolute file URI"),
            (" file:///w/a.rs", "not an absolute file URI"),
            ("file:///w/a.rs?x=1", "query or fragment"),
            ("file:///w/a.rs#L3", "query or fragment"),
            ("file:///w/", "names a directory"),
            ("file://", "names a directory"),
            (
                "file://buildhost/w/a.rs",
                "does not name a path on this machine",
            ),
            ("mailto:", "names nothing after its scheme"),
        ] {
            let err = parse_source(uri).expect_err(uri);
            assert!(err.contains(why), "{uri}: {err}");
        }
    }

    #[test]
    fn parse_locations_refuses_malformed_answers() {
        for (answer, why) in [
            (serde_json::json!("src/a.rs:3"), "got a string"),
            (serde_json::json!(7), "got a number"),
            (serde_json::json!([{ "range": {} }]), "without `uri`"),
            (serde_json::json!({ "uri": 5, "range": {} }), "not a string"),
            (
                serde_json::json!({ "uri": "file:///a", "range": { "start": { "line": -1, "character": 0 }, "end": { "line": 0, "character": 0 } } }),
                "not a non-negative integer",
            ),
            (
                serde_json::json!({ "uri": "file:///a", "range": { "start": { "line": 4294967295u64, "character": 0 }, "end": { "line": 4294967295u64, "character": 0 } } }),
                "out of range",
            ),
            (
                serde_json::json!({ "uri": "file:///a", "range": { "start": { "line": 2, "character": 0 }, "end": { "line": 1, "character": 0 } } }),
                "ends before it starts",
            ),
            (
                serde_json::json!({ "uri": "file:///a" }),
                "range is missing",
            ),
            (
                serde_json::json!({ "uri": "file:///a", "range": { "end": { "line": 1, "character": 0 } } }),
                "position is missing",
            ),
        ] {
            let err = parse_locations(&answer).expect_err(why);
            assert!(err.contains(why), "{answer}: {err}");
        }
    }

    #[test]
    fn parse_decls_reads_nested_and_flat_symbols_and_refuses_malformed_ones() {
        let nested = serde_json::json!([{
            "name": "Thing", "kind": 23,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 9, "character": 1 } },
            "selectionRange": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 16 } },
            "children": [
                { "name": "field", "kind": 8,
                  "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 9 } },
                  "selectionRange": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 9 } } },
                { "name": "run", "kind": 6,
                  "range": { "start": { "line": 3, "character": 4 }, "end": { "line": 5, "character": 5 } },
                  "selectionRange": { "start": { "line": 3, "character": 7 }, "end": { "line": 3, "character": 10 } },
                  "children": null }
            ]
        }]);
        let decls = parse_decls(&nested).unwrap();
        let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["Thing", "run"], "a field is not sliced");
        assert_eq!((decls[1].start_line(), decls[1].end_line()), (4, 6));

        let flat = serde_json::json!([{ "name": "CONST", "kind": 14, "location": { "uri": "file:///a", "range": { "start": { "line": 2, "character": 0 }, "end": { "line": 2, "character": 9 } } } }]);
        let decls = parse_decls(&flat).unwrap();
        assert_eq!(decls[0].selection, decls[0].range);
        assert!(parse_decls(&serde_json::Value::Null).unwrap().is_empty());

        // An inverted range like the one the real Rust engine gives `mod config;`, on a symbol
        // the slicer never slices, does not cost the file its functions.
        let module = serde_json::json!([
            { "name": "config", "kind": 2,
              "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
              "selectionRange": { "start": { "line": 0, "character": 8 }, "end": { "line": 0, "character": 0 } } },
            { "name": "seed", "kind": 12,
              "range": { "start": { "line": 2, "character": 0 }, "end": { "line": 4, "character": 1 } },
              "selectionRange": { "start": { "line": 2, "character": 7 }, "end": { "line": 2, "character": 11 } } }
        ]);
        let decls = parse_decls(&module).unwrap();
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].name, "seed");

        for (answer, why) in [
            (
                serde_json::json!({ "name": "x" }),
                "expected a list of symbols",
            ),
            (serde_json::json!([{ "kind": 12 }]), "without a name"),
            (serde_json::json!([{ "name": "f" }]), "has no kind"),
            (
                serde_json::json!([{ "name": "f", "kind": 12 }]),
                "range is missing",
            ),
            (
                serde_json::json!([{ "name": "f", "kind": 12, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }, "selectionRange": { "start": { "line": 9999999999u64, "character": 0 }, "end": { "line": 0, "character": 1 } } }]),
                "out of range",
            ),
            (
                serde_json::json!([{ "name": "f", "kind": 12, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }, "children": {} }]),
                "not a list",
            ),
        ] {
            let err = parse_decls(&answer).expect_err(why);
            assert!(err.contains(why), "{answer}: {err}");
        }
    }

    fn item(name: &str, depth: u32, text: &str) -> SliceItem {
        SliceItem {
            file: "src/a.rs".into(),
            name: name.into(),
            kind: "function",
            start_line: 1,
            end_line: 3,
            depth,
            because: (depth > 0).then(|| "run".to_string()),
            text: text.into(),
        }
    }

    #[test]
    fn report_counts_bytes_and_reduction() {
        let report = SliceReport {
            seed: "run".into(),
            items: vec![item("run", 0, &"x".repeat(100))],
            source_bytes: 1000,
            external: vec!["HashMap".into(), "HashMap".into()],
            max_bytes: 120,
            truncated: 2,
            depth_limit: 2,
            unexpanded: 1,
            unsupported: vec!["String (jdt:)".into()],
            ..Default::default()
        };
        // Every lookup was answered, but the budget and the depth limit cut the walk.
        assert!(report.has_complete_evidence());
        assert!(report.is_bounded());
        assert!(!report.is_complete());
        assert_eq!(report.slice_bytes(), 100);
        assert!((report.reduction_percent() - 90.0).abs() < 0.001);
        let text = report.render();
        assert!(
            text.starts_with("BOUNDED slice of `run`: 1 item(s), 100 bytes from 1000 bytes of source (90% smaller); every dependency lookup was answered"),
            "{text}"
        );
        assert!(
            text.contains(
                "outside the workspace in a source of an unsupported URI scheme, not followed: \
                 String (jdt:)\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("byte budget of 120 bytes reached: 2 queued item(s) left out"),
            "{text}"
        );
        assert!(
            text.contains("depth limit 2 reached: the dependencies of 1 item(s) at depth 2"),
            "{text}"
        );
        assert!(text.contains("not followed: HashMap\n"), "{text}");
        assert!(text.contains("(the seed)"), "{text}");
        assert!(SliceReport::default().reduction_percent().abs() < f64::EPSILON);

        // Without the bounds, and with only names outside the workspace, it is complete.
        let whole = SliceReport {
            truncated: 0,
            unexpanded: 0,
            ..report
        };
        assert!(whole.is_complete());
        assert!(
            whole.render().starts_with("slice of `run`"),
            "{}",
            whole.render()
        );
    }

    #[test]
    fn an_incomplete_report_names_each_gap_and_claims_no_reduction() {
        let gaps: Vec<SliceGap> = (0..GAPS_SHOWN + 3)
            .map(|i| SliceGap {
                kind: [
                    GapKind::QueryFailed,
                    GapKind::Malformed,
                    GapKind::Unreadable,
                    GapKind::NameLimit,
                ][i % 4],
                item: "run".into(),
                detail: format!("gap {i}"),
            })
            .collect();
        let report = SliceReport {
            seed: "run".into(),
            items: vec![item("run", 0, "fn run() {}"), item("dep", 1, "fn dep() {}")],
            source_bytes: 100,
            unsliced: (0..EXTERNAL_SHOWN + 2)
                .map(|i| format!("m{i:02} (src/m.rs:1)"))
                .collect(),
            gaps,
            max_bytes: 4,
            seed_over_budget: true,
            ..Default::default()
        };
        assert!(!report.is_complete());
        let text = report.render();
        assert!(
            text.starts_with("INCOMPLETE slice of `run`: 2 item(s), 22 bytes from 100 bytes"),
            "{text}"
        );
        assert!(!text.contains("% smaller"), "{text}");
        assert!(
            text.contains("the seed alone is 11 bytes, over the byte budget of 4 bytes"),
            "{text}"
        );
        for label in [
            "definition query failed in `run`: gap 0",
            "malformed definition answer in `run`: gap 1",
            "target cannot be sliced in `run`: gap 2",
            "name limit in `run`: gap 3",
            "  - and 3 more",
            "outside any declaration the slicer includes, not followed: m00 (src/m.rs:1)",
            ", and 2 more",
            "(depth 1, used by run)",
        ] {
            assert!(text.contains(label), "{label}: {text}");
        }
    }

    #[test]
    fn test_render_with_dataflow_slice() {
        let code = r#"fn compute(x: i32) -> i32 {
    let a = x + 1;
    let unused = 99;
    let b = a * 2;
    b
}"#;
        let df = crate::dataflow::slice_intra_function(code, 1, 6, "compute", "src/lib.rs", Some(5), Some("b"));
        let report = SliceReport {
            seed: "compute".into(),
            dataflow_slice: Some(df),
            items: vec![
                item("compute", 0, "fn compute(x: i32) -> i32 { ... }"),
                item("DepType", 1, "struct DepType;"),
            ],
            source_bytes: 200,
            ..Default::default()
        };
        let text = report.render();
        assert!(text.contains("INTRA-FUNCTION DATA-FLOW SLICE: `compute`"));
        assert!(text.contains("Completeness: COMPLETE"));
        assert!(text.contains("let a = x + 1;"));
        assert!(text.contains("let b = a * 2;"));
        assert!(!text.contains("unused"));
        assert!(text.contains("(the seed, data-flow sliced)"));
    }
}

