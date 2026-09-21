//! Intent search over declarations (roadmap 8.4, first step).
//!
//! An agent that knows what it is looking for but not what it is called cannot use the symbol
//! index: `workspace/symbol` matches names, and the words in a question ("where do we decide
//! which node runs a workspace") are usually in the doc comment rather than the identifier.
//! So the gateway keeps a second index over *declarations plus the prose attached to them*:
//! the doc-comment block immediately above a declaration, its signature line, its name split
//! into words, and its container.
//!
//! The index is lexical and language-agnostic: one pass over the workspace copy, per-language
//! patterns for what starts a declaration, and the comment lines directly above it. It is
//! built on first use, kept per workspace, and rebuilt for a file whose size or modification
//! time changed. Ranking is BM25 over those four fields with the name weighted highest.
//!
//! What this is not: it does not embed anything, so a query that shares no words with the
//! code or its comments will not find it. The dense half of 8.4 is still open.

use prod_code_protocol::{SearchHit, SearchRequest, SearchResponse};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// Declarations kept per file; a file with more is truncated (generated code, big tables).
const MAX_DECLS_PER_FILE: usize = 400;
/// Files larger than this are not indexed.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Doc-comment lines collected above a declaration.
const MAX_DOC_LINES: usize = 12;
/// Hits returned when the caller does not say.
pub const DEFAULT_LIMIT: usize = 10;

/// One indexed declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub file: String,
    pub line: u32,
    pub kind: String,
    pub name: String,
    pub container: Option<String>,
    pub signature: String,
    pub doc: String,
    /// The declaration belongs to a test: its name, its container, its file, or simply its
    /// position after the file's test module began.
    pub is_test: bool,
}

/// A declaration with its fields already tokenized: a query then costs one pass over the
/// index instead of retokenizing every declaration it looks at.
struct Indexed {
    decl: Declaration,
    fields: Fields,
    len: f64,
}

impl Indexed {
    fn new(decl: Declaration) -> Self {
        let fields = (
            tokenize(&decl.name),
            tokenize(decl.container.as_deref().unwrap_or("")),
            tokenize(&decl.signature),
            tokenize(&decl.doc),
        );
        let len = (fields.0.len() + fields.1.len() + fields.2.len() + fields.3.len()) as f64;
        Self { decl, fields, len }
    }
}

/// What a file contributed, with the stamp that tells us whether to redo it.
struct FileEntry {
    stamp: (u64, u64),
    decls: Vec<Indexed>,
}

#[derive(Default)]
pub struct WorkspaceIndex {
    files: HashMap<String, FileEntry>,
    /// Set until the first full walk; afterwards the index is kept current by the sync layer
    /// telling us which files it wrote, so a query never walks the tree.
    built: bool,
    /// Files the sync layer touched since the last query, to be reindexed on the next one.
    pending: Vec<String>,
}

impl WorkspaceIndex {
    fn declarations(&self) -> impl Iterator<Item = &Indexed> {
        self.files.values().flat_map(|f| f.decls.iter())
    }

    pub fn len(&self) -> usize {
        self.files.values().map(|f| f.decls.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Every workspace's index, built lazily and kept until the gateway stops.
#[derive(Default)]
pub struct SearchIndexes {
    by_workspace: Mutex<HashMap<PathBuf, WorkspaceIndex>>,
}

impl SearchIndexes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Refreshes the workspace's index against the files on disk and runs the query.
    /// Returns the hits, how many files were indexed and how many declarations they hold.
    pub fn search(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
        subpath: Option<&str>,
    ) -> (Vec<SearchHit>, usize, usize) {
        let mut guard = self.by_workspace.lock().unwrap_or_else(|e| e.into_inner());
        let index = guard.entry(root.to_path_buf()).or_default();
        if !index.built {
            refresh(root, index);
            index.built = true;
        } else if !index.pending.is_empty() {
            let pending = std::mem::take(&mut index.pending);
            for rel in pending {
                reindex_one(root, index, &rel);
            }
        }
        let hits = rank(index, query, limit, subpath);
        (hits, index.files.len(), index.len())
    }

    /// Records that these workspace-relative paths were written or deleted, so the next query
    /// reindexes exactly them instead of walking the tree.
    pub fn invalidate<I, S>(&self, root: &Path, paths: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut guard = self.by_workspace.lock().unwrap_or_else(|e| e.into_inner());
        let Some(index) = guard.get_mut(root) else {
            return;
        };
        for path in paths {
            let rel = path.as_ref().replace('\\', "/");
            if language_of(rel.rsplit('/').next().unwrap_or(&rel)).is_some() {
                index.pending.push(rel);
            }
        }
    }

    /// Drops a workspace's index (its engine was evicted or its directory pruned).
    pub fn forget(&self, root: &Path) {
        self.by_workspace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(root);
    }
}

/// Walks the workspace copy and reindexes files whose stamp changed.
fn refresh(root: &Path, index: &mut WorkspaceIndex) {
    let mut present = Vec::new();
    collect_source_files(root, root, &mut present);
    let mut seen: HashMap<String, ()> = HashMap::with_capacity(present.len());
    for (rel, path) in present {
        seen.insert(rel.clone(), ());
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let stamp = (
            meta.len(),
            meta.modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
        if index.files.get(&rel).map(|f| f.stamp) == Some(stamp) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let decls = declarations_in(&rel, &text)
            .into_iter()
            .map(Indexed::new)
            .collect();
        index.files.insert(rel, FileEntry { stamp, decls });
    }
    index.files.retain(|rel, _| seen.contains_key(rel));
}

/// Reindexes one file after the sync layer wrote or removed it.
fn reindex_one(root: &Path, index: &mut WorkspaceIndex, rel: &str) {
    let path = root.join(rel);
    let Ok(meta) = std::fs::metadata(&path) else {
        index.files.remove(rel);
        return;
    };
    if meta.len() > MAX_FILE_BYTES {
        index.files.remove(rel);
        return;
    }
    let stamp = (
        meta.len(),
        meta.modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
    let Ok(text) = std::fs::read_to_string(&path) else {
        index.files.remove(rel);
        return;
    };
    let decls = declarations_in(rel, &text)
        .into_iter()
        .map(Indexed::new)
        .collect();
    index
        .files
        .insert(rel.to_string(), FileEntry { stamp, decls });
}

/// Source files worth indexing, relative path first. Mirrors the sync layer's exclusions.
fn collect_source_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(
            name.as_str(),
            ".git"
                | "target"
                | "node_modules"
                | ".venv"
                | "venv"
                | "__pycache__"
                | ".pytest_cache"
                | ".mypy_cache"
                | ".build"
                | "dist"
                | "vendor"
        ) || name.starts_with('.') && name != ".config"
        {
            continue;
        }
        if path.is_dir() {
            collect_source_files(root, &path, out);
        } else if language_of(&name).is_some()
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push((rel.to_string_lossy().replace('\\', "/"), path));
        }
    }
}

/// The language a file name belongs to, or `None` when it is not source we index.
fn language_of(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1;
    Some(match ext {
        "rs" => "rust",
        "go" => "go",
        "ts" | "tsx" | "js" | "jsx" | "mjs" => "typescript",
        "py" => "python",
        "swift" => "swift",
        "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" => "cpp",
        _ => return None,
    })
}

/// A declaration this line starts: its kind and its name, or `None`.
///
/// Deliberately shallow. It recognises the shapes that carry a doc comment in the six
/// languages the gateway serves, and ignores everything else; a false negative costs a
/// missing hit, and the analyzer remains the authority on what a symbol actually is.
fn declaration_on(line: &str, language: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    fn strip<'a>(t: &'a str, prefixes: &[&str]) -> &'a str {
        let mut cur = t;
        loop {
            let mut moved = false;
            for p in prefixes {
                if let Some(rest) = cur.strip_prefix(p) {
                    cur = rest.trim_start();
                    moved = true;
                }
            }
            if !moved {
                return cur;
            }
        }
    }
    let ident = |s: &str| -> Option<String> {
        let name: String = s
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty() && !name.chars().next()?.is_numeric()).then_some(name)
    };
    let keyword = |t: &str, words: &[(&str, &str)]| -> Option<(String, String)> {
        for (word, kind) in words {
            if let Some(rest) = t.strip_prefix(*word)
                && rest.starts_with(|c: char| c.is_whitespace())
                && let Some(name) = ident(rest.trim_start())
            {
                return Some((kind.to_string(), name));
            }
        }
        None
    };
    match language {
        "rust" => {
            let t = strip(
                t,
                &[
                    "pub(crate)",
                    "pub(super)",
                    "pub",
                    "async",
                    "unsafe",
                    "const",
                    "default",
                ],
            );
            keyword(
                t,
                &[
                    ("fn", "function"),
                    ("struct", "struct"),
                    ("enum", "enum"),
                    ("trait", "trait"),
                    ("type", "type"),
                    ("static", "constant"),
                    ("macro_rules!", "macro"),
                    ("mod", "module"),
                ],
            )
            .or_else(|| {
                t.strip_prefix("impl").and_then(|rest| {
                    let body = rest.split_once(" for ").map(|(_, b)| b).unwrap_or(rest);
                    ident(body.trim_start().trim_start_matches('<'))
                        .map(|n| ("impl".to_string(), n))
                })
            })
        }
        "go" => keyword(
            t,
            &[
                ("func", "function"),
                ("type", "type"),
                ("const", "constant"),
                ("var", "variable"),
            ],
        )
        .map(|(kind, name)| {
            // func (r *Receiver) Name(...) — the name follows the receiver.
            if kind == "function" && name.is_empty() {
                (kind, name)
            } else if kind == "function"
                && let Some(rest) = t.strip_prefix("func")
                && rest.trim_start().starts_with('(')
                && let Some((_, after)) = rest.split_once(')')
                && let Some(real) = ident(after.trim_start())
            {
                ("method".to_string(), real)
            } else {
                (kind, name)
            }
        }),
        "python" => keyword(
            t,
            &[
                ("def", "function"),
                ("async def", "function"),
                ("class", "class"),
            ],
        )
        .or_else(|| {
            let t = strip(t, &["async"]);
            keyword(t, &[("def", "function")])
        }),
        "typescript" => {
            let t = strip(
                t,
                &[
                    "export",
                    "default",
                    "declare",
                    "abstract",
                    "async",
                    "public",
                    "private",
                    "protected",
                    "static",
                    "readonly",
                ],
            );
            keyword(
                t,
                &[
                    ("function", "function"),
                    ("class", "class"),
                    ("interface", "interface"),
                    ("type", "type"),
                    ("enum", "enum"),
                    ("const", "constant"),
                ],
            )
        }
        "swift" => {
            let t = strip(
                t,
                &[
                    "public",
                    "private",
                    "internal",
                    "fileprivate",
                    "open",
                    "final",
                    "static",
                    "override",
                    "@objc",
                ],
            );
            keyword(
                t,
                &[
                    ("func", "function"),
                    ("struct", "struct"),
                    ("class", "class"),
                    ("enum", "enum"),
                    ("protocol", "protocol"),
                    ("extension", "extension"),
                    ("let", "constant"),
                    ("var", "variable"),
                ],
            )
        }
        "cpp" => keyword(
            t,
            &[
                ("class", "class"),
                ("struct", "struct"),
                ("namespace", "module"),
                ("enum", "enum"),
            ],
        )
        .or_else(|| {
            // A definition line ending in `{` with a parameter list: `Type name(args) {`.
            let before = t.split_once('(')?.0.trim_end();
            (!before.is_empty() && t.contains('(') && !t.starts_with('#'))
                .then(|| ident(before.rsplit([' ', ':', '*', '&']).next()?))
                .flatten()
                .map(|n| ("function".to_string(), n))
        }),
        _ => None,
    }
}

/// Is this line a doc comment for whatever follows it?
fn doc_line(line: &str, language: &str) -> Option<String> {
    let t = line.trim();
    let strip_any = |t: &str, prefixes: &[&str]| -> Option<String> {
        for p in prefixes {
            if let Some(rest) = t.strip_prefix(p) {
                return Some(rest.trim().to_string());
            }
        }
        None
    };
    match language {
        "python" => strip_any(t, &["#"]),
        "rust" => strip_any(t, &["///", "//!", "//"]),
        _ => strip_any(t, &["///", "/**", "*/", "*", "//"]),
    }
}

/// Every declaration in a file with the prose attached to it.
pub fn declarations_in(rel_path: &str, text: &str) -> Vec<Declaration> {
    let Some(language) = language_of(rel_path.rsplit('/').next().unwrap_or(rel_path)) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut container: Option<String> = None;
    let mut container_indent = usize::MAX;
    // Once a file's test module starts, everything after it is test material. This also
    // covers the source fixtures tests keep in multi-line string literals, which a
    // line-based scanner cannot tell from real code.
    let mut in_test_module = false;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[cfg(test)]")
            || trimmed.starts_with("mod tests")
            || trimmed.starts_with("pub mod tests")
            || trimmed == "if __name__ == \"__main__\":"
        {
            in_test_module = true;
        }
        let indent = line.len() - line.trim_start().len();
        if indent <= container_indent && container.is_some() && !line.trim().is_empty() {
            let is_decl_here = declaration_on(line, language).is_some();
            if is_decl_here && indent <= container_indent {
                container = None;
                container_indent = usize::MAX;
            }
        }
        let Some((kind, name)) = declaration_on(line, language) else {
            continue;
        };
        let mut doc = Vec::new();
        let mut j = i;
        while j > 0 && doc.len() < MAX_DOC_LINES {
            j -= 1;
            let prev = lines[j].trim();
            if prev.is_empty() || prev.starts_with('#') && language != "python" {
                if prev.starts_with("#[") || prev.starts_with("#!") {
                    continue;
                }
                break;
            }
            match doc_line(lines[j], language) {
                Some(text) if !text.is_empty() => doc.push(text),
                _ => break,
            }
        }
        doc.reverse();
        if matches!(
            kind.as_str(),
            "impl" | "class" | "struct" | "extension" | "module"
        ) {
            container = Some(name.clone());
            container_indent = indent;
        }
        let is_test = in_test_module
            || name.to_lowercase().starts_with("test")
            || name.to_lowercase().ends_with("_test")
            || container
                .as_deref()
                .is_some_and(|c| c.to_lowercase().contains("test"))
            || rel_path.contains("/tests/")
            || rel_path.ends_with("_test.go")
            || rel_path.contains("/test_")
            || rel_path.ends_with(".test.ts")
            || rel_path.ends_with(".spec.ts");
        out.push(Declaration {
            file: rel_path.to_string(),
            line: i as u32 + 1,
            is_test,
            kind,
            container: container.clone().filter(|c| c != &name),
            name,
            signature: line.trim().trim_end_matches('{').trim().to_string(),
            doc: doc.join(" "),
        });
        if out.len() >= MAX_DECLS_PER_FILE {
            break;
        }
    }
    out
}

/// Lowercased word tokens: splits on non-alphanumerics, then on camelCase humps, and drops
/// the words that carry no signal in a query.
pub fn tokenize(text: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "a", "an", "of", "to", "in", "on", "for", "and", "or", "is", "are", "was", "were",
        "be", "we", "do", "does", "did", "our", "it", "its", "that", "this", "with", "how", "what",
        "where", "when", "which", "who", "why", "from", "by", "at", "as", "into", "out", "if",
        "then", "than", "so", "but", "not", "no", "yes", "can", "will", "would", "should", "get",
        "set", "new", "use", "used", "using",
    ];
    let mut out = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric()) {
        if raw.is_empty() {
            continue;
        }
        for part in split_humps(raw) {
            let lower = part.to_lowercase();
            if lower.len() > 1 && !STOP.contains(&lower.as_str()) {
                out.push(singular(&lower));
            }
        }
    }
    out
}

/// Folds a simple English plural so `nodes` and `node` are the same term. Deliberately crude:
/// no stemmer, just a trailing `s` on a word long enough for it to mean plural.
fn singular(word: &str) -> String {
    if word.len() > 3 && word.ends_with('s') && !word.ends_with("ss") && !word.ends_with("us") {
        word[..word.len() - 1].to_string()
    } else {
        word.to_string()
    }
}

/// `parseHTTPResponse` → [parse, HTTP, Response]; `snake_case` arrives already split.
fn split_humps(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let cur = chars[i];
        let boundary = (prev.is_lowercase() && cur.is_uppercase())
            || (prev.is_uppercase()
                && cur.is_uppercase()
                && chars.get(i + 1).is_some_and(|n| n.is_lowercase()));
        if boundary {
            parts.push(chars[start..i].iter().collect());
            start = i;
        }
    }
    parts.push(chars[start..].iter().collect());
    parts
        .into_iter()
        .filter(|p: &String| !p.is_empty())
        .collect()
}

/// A declaration's four searchable fields, tokenized: name, container, signature, doc.
type Fields = (Vec<String>, Vec<String>, Vec<String>, Vec<String>);

/// How much a declaration of this kind can answer a question about behaviour. A field named
/// `provider` matches the word, but "where do we decide which provider runs a task" is asking
/// for the code that decides, not for the place the answer is stored.
fn kind_weight(kind: &str) -> f64 {
    match kind {
        "function" | "method" => 1.0,
        "struct" | "class" | "enum" | "interface" | "trait" | "type" | "protocol" => 0.8,
        "impl" | "extension" | "module" => 0.6,
        _ => 0.4,
    }
}

/// Field weights: a query word in the name means more than the same word in a comment.
const W_NAME: f64 = 3.0;
const W_CONTAINER: f64 = 1.5;
const W_SIGNATURE: f64 = 1.2;
const W_DOC: f64 = 1.0;

/// BM25 over the four fields of every declaration, best first.
fn rank(
    index: &WorkspaceIndex,
    query: &str,
    limit: usize,
    subpath: Option<&str>,
) -> Vec<SearchHit> {
    let terms = tokenize(query);
    if terms.is_empty() {
        return Vec::new();
    }
    // A test's name repeats every word of the thing it tests, so on a question about that
    // thing it outranks the thing itself. Tests are searched only when the question is about
    // tests.
    let wants_tests = terms
        .iter()
        .any(|t| matches!(t.as_str(), "test" | "spec" | "fixture" | "mock"));
    let docs: Vec<&Indexed> = index
        .declarations()
        .filter(|d| subpath.is_none_or(|p| d.decl.file.starts_with(p)))
        .filter(|d| wants_tests || !d.decl.is_test)
        .collect();
    if docs.is_empty() {
        return Vec::new();
    }
    // Document frequency per term, over declarations rather than files.
    let mut df: HashMap<&str, usize> = HashMap::new();
    for (name, container, signature, doc) in docs.iter().map(|d| &d.fields) {
        let mut present: Vec<&str> = Vec::new();
        for t in name.iter().chain(container).chain(signature).chain(doc) {
            if !present.contains(&t.as_str()) {
                present.push(t.as_str());
            }
        }
        for t in present {
            for term in &terms {
                if term == t {
                    *df.entry(term.as_str()).or_insert(0) += 1;
                }
            }
        }
    }
    let n = docs.len() as f64;
    let avg_len: f64 = docs.iter().map(|d| d.len).sum::<f64>() / n;
    const K1: f64 = 1.2;
    const B: f64 = 0.45;
    let mut scored: Vec<(f64, &Declaration)> = Vec::new();
    for doc in docs.iter() {
        let (name, container, signature, docs_t) = &doc.fields;
        let len = doc.len;
        let mut score = 0.0;
        let mut matched = 0usize;
        for term in &terms {
            let tf = W_NAME * count(name, term)
                + W_CONTAINER * count(container, term)
                + W_SIGNATURE * count(signature, term)
                + W_DOC * count(docs_t, term);
            if tf == 0.0 {
                continue;
            }
            matched += 1;
            let df_t = *df.get(term.as_str()).unwrap_or(&1) as f64;
            let idf = ((n - df_t + 0.5) / (df_t + 0.5) + 1.0).ln();
            score += idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * len / avg_len.max(1.0)));
        }
        if matched == 0 {
            continue;
        }
        // A declaration matching more of the question beats one matching one word often.
        score *= 1.0 + 0.35 * (matched - 1) as f64;
        score *= kind_weight(&doc.decl.kind);
        scored.push((score, &doc.decl));
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.file.cmp(&b.1.file))
            .then_with(|| a.1.line.cmp(&b.1.line))
    });
    scored
        .into_iter()
        .take(limit.max(1))
        .map(|(_score, d)| SearchHit {
            file: d.file.clone(),
            line: d.line,
            kind: d.kind.clone(),
            name: d.name.clone(),
            container: d.container.clone(),
            signature: d.signature.clone(),
            doc: first_sentence(&d.doc),
        })
        .collect()
}

fn count(tokens: &[String], term: &str) -> f64 {
    tokens.iter().filter(|t| t.as_str() == term).count() as f64
}

/// The first sentence of a doc block, for a one-line result.
fn first_sentence(doc: &str) -> String {
    let trimmed = doc.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match trimmed.find(". ") {
        Some(i) if i < 200 => trimmed[..=i].trim().to_string(),
        _ => trimmed
            .chars()
            .take(200)
            .collect::<String>()
            .trim()
            .to_string(),
    }
}

/// Answers a `SearchRequest` against the workspace copy.
pub fn run_search(
    indexes: &SearchIndexes,
    storage_root: &Path,
    req: &SearchRequest,
) -> SearchResponse {
    let workspace = crate::workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let workspace_str = workspace.to_string_lossy().to_string();
    if !workspace.is_dir() {
        return SearchResponse {
            server_workspace_root: workspace_str.clone(),
            hits: Vec::new(),
            indexed_files: 0,
            indexed_declarations: 0,
            took_ms: 0,
            error: Some(format!(
                "workspace {workspace_str} is not synced to this gateway"
            )),
        };
    }
    if req.query.trim().is_empty() {
        return SearchResponse {
            server_workspace_root: workspace_str,
            hits: Vec::new(),
            indexed_files: 0,
            indexed_declarations: 0,
            took_ms: 0,
            error: Some("empty query".to_string()),
        };
    }
    let started = Instant::now();
    let limit = if req.limit == 0 {
        DEFAULT_LIMIT
    } else {
        req.limit
    };
    let subpath = req.subpath.as_deref().filter(|p| !p.is_empty());
    let (hits, files, decls) = indexes.search(&workspace, &req.query, limit, subpath);
    SearchResponse {
        server_workspace_root: workspace_str,
        hits,
        indexed_files: files,
        indexed_declarations: decls,
        took_ms: started.elapsed().as_millis() as u64,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_decls(rel: &str, text: &str) -> Vec<Indexed> {
        declarations_in(rel, text)
            .into_iter()
            .map(Indexed::new)
            .collect()
    }

    #[test]
    fn tokenize_splits_humps_and_drops_stopwords() {
        assert_eq!(
            tokenize("where do we decide which node runs a workspace"),
            vec!["decide", "node", "run", "workspace"]
        );
        assert_eq!(
            tokenize("parseHTTPResponse"),
            vec!["parse", "http", "response"]
        );
        assert_eq!(
            tokenize("server_workspace_path"),
            vec!["server", "workspace", "path"]
        );
    }

    #[test]
    fn rust_declarations_carry_their_doc_comment() {
        let src = "\
/// Decides which node should hold a workspace.
/// Falls back to the quietest live node.
pub async fn place(&self, name: &str) -> Option<String> {
    None
}

pub struct Metrics {
    pub count: u64,
}

impl Metrics {
    /// Records one event.
    pub fn record(&self, ev: Event) {}
}
";
        let decls = declarations_in("src/main.rs", src);
        let place = decls.iter().find(|d| d.name == "place").expect("place");
        assert_eq!(place.kind, "function");
        assert_eq!(place.line, 3);
        assert!(
            place
                .doc
                .starts_with("Decides which node should hold a workspace.")
        );
        assert!(place.signature.starts_with("pub async fn place"));
        let record = decls.iter().find(|d| d.name == "record").expect("record");
        assert_eq!(record.container.as_deref(), Some("Metrics"));
        assert_eq!(record.doc, "Records one event.");
        assert!(
            decls
                .iter()
                .any(|d| d.name == "Metrics" && d.kind == "struct")
        );
    }

    #[test]
    fn other_languages_are_recognised() {
        let go = declarations_in(
            "pkg/x.go",
            "// Serve starts the listener.\nfunc Serve(addr string) error {\n}\n",
        );
        assert_eq!(go[0].name, "Serve");
        assert_eq!(go[0].doc, "Serve starts the listener.");
        let py = declarations_in(
            "app/x.py",
            "# Compute the signal.\ndef compute_signal(x):\n    pass\n",
        );
        assert_eq!(py[0].name, "compute_signal");
        assert_eq!(py[0].doc, "Compute the signal.");
        let ts = declarations_in(
            "src/x.ts",
            "/** Sends a frame. */\nexport function sendFrame(f: Frame) {}\n",
        );
        assert_eq!(ts[0].name, "sendFrame");
        assert!(ts[0].doc.contains("Sends a frame."));
        let swift = declarations_in(
            "Sources/x.swift",
            "/// Reloads the view.\npublic func reload() {}\n",
        );
        assert_eq!(swift[0].name, "reload");
    }

    #[test]
    fn a_question_finds_the_declaration_whose_prose_answers_it() {
        let mut index = WorkspaceIndex::default();
        index.files.insert(
            "src/place.rs".to_string(),
            FileEntry {
                stamp: (0, 0),
                decls: index_decls(
                    "src/place.rs",
                    "/// Decides which node runs a workspace: the one already holding it, else the quietest.\npub fn place(name: &str) -> Node { todo!() }\n",
                ),
            },
        );
        index.files.insert(
            "src/sync.rs".to_string(),
            FileEntry {
                stamp: (0, 0),
                decls: index_decls(
                    "src/sync.rs",
                    "/// Uploads changed files to the gateway.\npub fn push_sync(files: Vec<File>) {}\n",
                ),
            },
        );
        let hits = rank(
            &index,
            "where do we decide which node runs a workspace",
            5,
            None,
        );
        assert!(!hits.is_empty(), "the question should match something");
        assert_eq!(hits[0].name, "place", "got {:?}", hits[0]);
        assert!(
            hits[0]
                .doc
                .starts_with("Decides which node runs a workspace")
        );

        let scoped = rank(&index, "decide node workspace", 5, Some("src/sync.rs"));
        assert!(scoped.iter().all(|h| h.file == "src/sync.rs"));
    }

    #[test]
    fn the_implementation_outranks_the_test_that_names_it() {
        let mut index = WorkspaceIndex::default();
        index.files.insert(
            "src/shadow.rs".to_string(),
            FileEntry {
                stamp: (0, 0),
                decls: [
                    Declaration {
                        file: "src/shadow.rs".into(),
                        line: 10,
                        kind: "function".into(),
                        name: "run_overlay".into(),
                        container: None,
                        signature: "pub async fn run_overlay(job: Job)".into(),
                        doc: "Runs one hypothesis as an overlay shadow.".into(),
                        is_test: false,
                    },
                    Declaration {
                        file: "src/shadow.rs".into(),
                        line: 200,
                        kind: "function".into(),
                        name: "overlay_hypotheses_run_in_parallel_and_leave_the_workspace_untouched".into(),
                        container: Some("tests".into()),
                        signature: "fn overlay_hypotheses_run_in_parallel_and_leave_the_workspace_untouched()".into(),
                        doc: String::new(),
                        is_test: true,
                    },
                ]
                .into_iter()
                .map(Indexed::new)
                .collect(),
            },
        );
        let hits = rank(&index, "where are hypotheses run in an overlay", 5, None);
        assert_eq!(
            hits[0].name,
            "run_overlay",
            "got {:?}",
            hits.iter().map(|h| &h.name).collect::<Vec<_>>()
        );
        assert!(
            hits.iter()
                .all(|h| !h.name.starts_with("overlay_hypotheses")),
            "a test should not answer a question about the implementation"
        );
        // Asking about the test finds it.
        let about_tests = rank(
            &index,
            "test that overlay hypotheses leave the workspace untouched",
            5,
            None,
        );
        assert!(
            about_tests
                .iter()
                .any(|h| h.name.starts_with("overlay_hypotheses")),
            "got {:?}",
            about_tests.iter().map(|h| &h.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_query_sharing_no_words_finds_nothing() {
        let mut index = WorkspaceIndex::default();
        index.files.insert(
            "a.rs".to_string(),
            FileEntry {
                stamp: (0, 0),
                decls: index_decls(
                    "a.rs",
                    "/// Adds two numbers.\npub fn add(a: i32, b: i32) -> i32 { a + b }\n",
                ),
            },
        );
        assert!(rank(&index, "kubernetes ingress certificate rotation", 5, None).is_empty());
        assert!(rank(&index, "", 5, None).is_empty());
    }

    #[test]
    fn first_sentence_is_bounded() {
        assert_eq!(first_sentence("One. Two."), "One.");
        assert_eq!(first_sentence(""), "");
        let long = "x".repeat(400);
        assert_eq!(first_sentence(&long).len(), 200);
    }
}
