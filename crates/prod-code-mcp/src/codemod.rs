//! Polyglot Structural AST Codemod Engine (Roadmap Item 8.7).
//!
//! Provides pattern-based structural code transformations across Go, TypeScript/JavaScript,
//! Python, C/C++, Swift, and Rust. Matches syntax trees regardless of whitespace, formatting,
//! or variable names, executing large-scale migrations and API upgrades across hundreds of files
//! in sub-second time.
//!
//! Rule syntax: `pattern ==>> replacement` with `$name` placeholders:
//! - `$a.unwrap() ==>> $a.expect("invariant")`
//! - `errors.Wrap($err, $msg) ==>> fmt.Errorf("%s: %w", $msg, $err)`
//! - `console.log($msg) ==>> logger.info($msg)`
//! - `os.path.join($a, $b) ==>> Path($a) / $b`
//! - `std::make_shared<$T>($args) ==>> std::allocate_shared<$T>(alloc, $args)`
//! - `print($x) ==>> os_log("\($x)")`

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Supported file extensions for polyglot structural codemods.
pub const CODE_EXTENSIONS: &[&str] = &[
    "rs", "go", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "cpp", "cc", "cxx", "c", "hpp", "h",
    "swift",
];

/// Directories to skip during workspace-wide codemod traversal.
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    ".build",
    ".cargo",
    "vendor",
    ".svn",
    ".hg",
];

/// Token kinds produced by the structural polyglot tokenizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Ident(String),
    StringLit(String),
    NumberLit(String),
    Punct(String),
    OpenDelim(char),
    CloseDelim(char),
}

/// A source token with exact byte offsets.
#[derive(Debug, Clone)]
pub struct SourceToken {
    pub kind: TokenKind,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// A token in a compiled pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternToken {
    Literal(TokenKind),
    Metavar(String),
}

/// A token in a compiled replacement template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplacementToken {
    Text(String),
    Metavar(String),
}

/// A compiled structural AST pattern for read-only search or codemod matching.
#[derive(Debug, Clone)]
pub struct CompiledPattern {
    pub raw: String,
    pub pattern_tokens: Vec<PatternToken>,
    /// Non-metavariable literal identifiers that MUST exist in a candidate file.
    pub required_literals: Vec<String>,
}

impl CompiledPattern {
    /// Parse and compile a structural pattern string (e.g. `$a.unwrap()`).
    pub fn parse(pattern: &str) -> Result<Self> {
        let pattern_raw = pattern.trim();
        if pattern_raw.is_empty() {
            bail!("pattern cannot be empty");
        }
        let pattern_tokens = tokenize_pattern(pattern_raw)?;
        let mut required_literals = Vec::new();
        for tok in &pattern_tokens {
            if let PatternToken::Literal(TokenKind::Ident(name)) = tok {
                if name.len() >= 2 && !name.starts_with('$') {
                    required_literals.push(name.clone());
                }
            }
        }
        required_literals.sort();
        required_literals.dedup();

        Ok(Self {
            raw: pattern.to_string(),
            pattern_tokens,
            required_literals,
        })
    }
}

/// A parsed and compiled structural codemod rule: `pattern ==>> replacement`.
#[derive(Debug, Clone)]
pub struct CodemodRule {
    pub raw: String,
    pub pattern_tokens: Vec<PatternToken>,
    pub replacement_tokens: Vec<ReplacementToken>,
    /// Non-metavariable literal identifiers that MUST exist in a candidate file.
    pub required_literals: Vec<String>,
}

impl CodemodRule {
    /// Parse a `pattern ==>> replacement` rule string.
    pub fn parse(rule: &str) -> Result<Self> {
        let parts: Vec<&str> = rule.split("==>>").collect();
        if parts.len() != 2 {
            bail!("a rule must be `pattern ==>> replacement`, got: {rule}");
        }
        let pattern_raw = parts[0].trim();
        let replacement_raw = parts[1].trim();
        if pattern_raw.is_empty() {
            bail!("pattern in rule cannot be empty");
        }

        let pattern = CompiledPattern::parse(pattern_raw)?;
        let replacement_tokens = parse_replacement(replacement_raw);

        Ok(Self {
            raw: rule.to_string(),
            pattern_tokens: pattern.pattern_tokens,
            replacement_tokens,
            required_literals: pattern.required_literals,
        })
    }
}

/// Tokenize a pattern string with support for metavariables (`$var`).
fn tokenize_pattern(pattern: &str) -> Result<Vec<PatternToken>> {
    let mut tokens = Vec::new();
    let bytes = pattern.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Metavariable: `$name` or `$$$args`
        if b == b'$' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i] == b'$' || bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let name = &pattern[start + 1..i];
            if name.is_empty() {
                tokens.push(PatternToken::Literal(TokenKind::Punct("$".to_string())));
            } else {
                tokens.push(PatternToken::Metavar(name.to_string()));
            }
            continue;
        }

        // Delimiters
        if matches!(b, b'(' | b'[' | b'{') {
            tokens.push(PatternToken::Literal(TokenKind::OpenDelim(b as char)));
            i += 1;
            continue;
        }
        if matches!(b, b')' | b']' | b'}') {
            tokens.push(PatternToken::Literal(TokenKind::CloseDelim(b as char)));
            i += 1;
            continue;
        }

        // String literals
        if matches!(b, b'"' | b'\'' | b'`') {
            let quote = b;
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < bytes.len() {
                i += 1; // closing quote
            }
            tokens.push(PatternToken::Literal(TokenKind::StringLit(
                pattern[start..i].to_string(),
            )));
            continue;
        }

        // Identifiers
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push(PatternToken::Literal(TokenKind::Ident(
                pattern[start..i].to_string(),
            )));
            continue;
        }

        // Numbers
        if b.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push(PatternToken::Literal(TokenKind::NumberLit(
                pattern[start..i].to_string(),
            )));
            continue;
        }

        // Multi-char punctuation
        if i + 1 < bytes.len() && b.is_ascii() && bytes[i + 1].is_ascii() {
            let pair = &pattern[i..i + 2];
            if matches!(
                pair,
                "==" | "!=" | "<=" | ">=" | "&&" | "||" | "->" | "::" | "+=" | "-=" | "*=" | "/=" | ":=" | "=>" | "??" | "?." | "<<" | ">>" | "**" | "//"
            ) {
                tokens.push(PatternToken::Literal(TokenKind::Punct(pair.to_string())));
                i += 2;
                continue;
            }
        }

        // Non-ASCII Unicode character
        if !b.is_ascii() {
            let ch = pattern[i..].chars().next().unwrap();
            let ch_len = ch.len_utf8();
            tokens.push(PatternToken::Literal(if ch.is_alphabetic() {
                TokenKind::Ident(ch.to_string())
            } else {
                TokenKind::Punct(ch.to_string())
            }));
            i += ch_len;
            continue;
        }

        tokens.push(PatternToken::Literal(TokenKind::Punct(
            (b as char).to_string(),
        )));
        i += 1;
    }

    Ok(tokens)
}

/// Parse a replacement string into literal chunks and `$var` placeholders.
fn parse_replacement(rep: &str) -> Vec<ReplacementToken> {
    let mut tokens = Vec::new();
    let bytes = rep.as_bytes();
    let mut i = 0;
    let mut text_start = 0;

    while i < bytes.len() {
        if bytes[i] == b'$' {
            if i > text_start {
                tokens.push(ReplacementToken::Text(rep[text_start..i].to_string()));
            }
            let var_start = i + 1;
            i += 1;
            while i < bytes.len() && (bytes[i] == b'$' || bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let name = &rep[var_start..i];
            if name.is_empty() {
                tokens.push(ReplacementToken::Text("$".to_string()));
            } else {
                tokens.push(ReplacementToken::Metavar(name.to_string()));
            }
            text_start = i;
        } else {
            i += 1;
        }
    }
    if text_start < bytes.len() {
        tokens.push(ReplacementToken::Text(rep[text_start..].to_string()));
    }
    tokens
}

/// Tokenize source code into structural tokens with byte spans, ignoring comments and whitespace.
pub fn tokenize_source(source: &str) -> Vec<SourceToken> {
    let mut tokens = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];

        // Whitespace
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Single-line comment `//`
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Single-line comment `#` (Python, Shell, etc.)
        if b == b'#' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Multi-line comment `/* ... */`
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < bytes.len() {
                i += 2;
            } else {
                i = bytes.len();
            }
            continue;
        }

        // Delimiters
        if matches!(b, b'(' | b'[' | b'{') {
            tokens.push(SourceToken {
                kind: TokenKind::OpenDelim(b as char),
                start_byte: i,
                end_byte: i + 1,
            });
            i += 1;
            continue;
        }
        if matches!(b, b')' | b']' | b'}') {
            tokens.push(SourceToken {
                kind: TokenKind::CloseDelim(b as char),
                start_byte: i,
                end_byte: i + 1,
            });
            i += 1;
            continue;
        }

        // String literals (", ', `)
        if matches!(b, b'"' | b'\'' | b'`') {
            let quote = b;
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < bytes.len() {
                i += 1;
            }
            tokens.push(SourceToken {
                kind: TokenKind::StringLit(source[start..i].to_string()),
                start_byte: start,
                end_byte: i,
            });
            continue;
        }

        // Identifiers
        if b.is_ascii_alphabetic() || b == b'_' || b == b'$' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$') {
                i += 1;
            }
            tokens.push(SourceToken {
                kind: TokenKind::Ident(source[start..i].to_string()),
                start_byte: start,
                end_byte: i,
            });
            continue;
        }

        // Numbers
        if b.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push(SourceToken {
                kind: TokenKind::NumberLit(source[start..i].to_string()),
                start_byte: start,
                end_byte: i,
            });
            continue;
        }

        // Multi-char punctuation
        let p_start = i;
        if i + 1 < bytes.len() && b.is_ascii() && bytes[i + 1].is_ascii() {
            let pair = &source[i..i + 2];
            if matches!(
                pair,
                "==" | "!=" | "<=" | ">=" | "&&" | "||" | "->" | "::" | "+=" | "-=" | "*=" | "/=" | ":=" | "=>" | "??" | "?." | "<<" | ">>" | "**" | "//"
            ) {
                tokens.push(SourceToken {
                    kind: TokenKind::Punct(pair.to_string()),
                    start_byte: p_start,
                    end_byte: p_start + 2,
                });
                i += 2;
                continue;
            }
        }

        // Non-ASCII Unicode character
        if !b.is_ascii() {
            let ch = source[i..].chars().next().unwrap();
            let ch_len = ch.len_utf8();
            tokens.push(SourceToken {
                kind: if ch.is_alphabetic() {
                    TokenKind::Ident(ch.to_string())
                } else {
                    TokenKind::Punct(ch.to_string())
                },
                start_byte: p_start,
                end_byte: p_start + ch_len,
            });
            i += ch_len;
            continue;
        }

        // Single-char punctuation (ASCII)
        tokens.push(SourceToken {
            kind: TokenKind::Punct((b as char).to_string()),
            start_byte: p_start,
            end_byte: p_start + 1,
        });
        i += 1;
    }

    tokens
}

/// Compute matching delimiter indices for fast balance checking.
fn compute_matching_delims(tokens: &[SourceToken]) -> Vec<Option<usize>> {
    let mut match_map = vec![None; tokens.len()];
    let mut stack: Vec<(char, usize)> = Vec::new();

    for (idx, tok) in tokens.iter().enumerate() {
        match tok.kind {
            TokenKind::OpenDelim(ch) => stack.push((ch, idx)),
            TokenKind::CloseDelim(ch) => {
                let expected_open = match ch {
                    ')' => '(',
                    ']' => '[',
                    '}' => '{',
                    _ => continue,
                };
                if let Some((open_ch, open_idx)) = stack.pop()
                    && open_ch == expected_open
                {
                    match_map[open_idx] = Some(idx);
                    match_map[idx] = Some(open_idx);
                }
            }
            _ => {}
        }
    }
    match_map
}

/// One matched AST span and its computed replacement string.
#[derive(Debug, Clone)]
pub struct CodemodMatch {
    pub start_byte: usize,
    pub end_byte: usize,
    pub replacement: String,
}

fn is_boundary_token(kind: &TokenKind, next_expected: &TokenKind) -> bool {
    if kind == next_expected {
        return false;
    }
    match kind {
        TokenKind::Punct(p) if p == ";" => true,
        TokenKind::Punct(p) if matches!(p.as_str(), "=" | ":=" | "+=" | "-=" | "*=" | "/=") => true,
        TokenKind::Punct(p) if p == "," => !matches!(next_expected, TokenKind::CloseDelim(_)),
        TokenKind::Ident(id) => matches!(
            id.as_str(),
            "let" | "var" | "const" | "return" | "fn" | "func" | "function" | "def"
                | "class" | "struct" | "enum" | "interface" | "import" | "export" | "package"
        ),
        _ => false,
    }
}

/// Attempt to match pattern tokens starting at `start_idx` in `tokens`.
/// Returns `(start_byte, end_byte, end_token_idx, bindings)` on success.
fn match_pattern_tokens(
    tokens: &[SourceToken],
    start_idx: usize,
    matching_delims: &[Option<usize>],
    pattern_tokens: &[PatternToken],
    source: &str,
) -> Option<(usize, usize, usize, BTreeMap<String, (usize, usize)>)> {
    let mut code_idx = start_idx;
    let mut pat_idx = 0;
    let mut bindings: BTreeMap<String, (usize, usize)> = BTreeMap::new();

    while pat_idx < pattern_tokens.len() {
        let pat_tok = &pattern_tokens[pat_idx];

        match pat_tok {
            PatternToken::Literal(expected_kind) => {
                if code_idx >= tokens.len() {
                    return None;
                }
                if &tokens[code_idx].kind != expected_kind {
                    return None;
                }
                code_idx += 1;
                pat_idx += 1;
            }
            PatternToken::Metavar(var_name) => {
                let next_pat_tok = pattern_tokens.get(pat_idx + 1);
                let var_start_token = code_idx;

                if code_idx >= tokens.len() {
                    return None;
                }

                // If next pattern token is a literal delimiter or punctuation,
                // consume tokens until we hit that token at the current nesting level.
                let var_end_token = match next_pat_tok {
                    Some(PatternToken::Literal(next_expected)) => {
                        let mut depth = 0;
                        let mut found = None;
                        let mut cur = code_idx;

                        while cur < tokens.len() {
                            match tokens[cur].kind {
                                TokenKind::OpenDelim(_) => depth += 1,
                                TokenKind::CloseDelim(_) => {
                                    if depth == 0 {
                                        // Reached an enclosing close delimiter
                                        if &tokens[cur].kind == next_expected {
                                            found = Some(cur);
                                        }
                                        break;
                                    }
                                    depth -= 1;
                                }
                                _ => {
                                    if depth == 0 {
                                        if &tokens[cur].kind == next_expected {
                                            found = Some(cur);
                                            break;
                                        }
                                        if is_boundary_token(&tokens[cur].kind, next_expected) {
                                            return None;
                                        }
                                    }
                                }
                            }
                            cur += 1;
                        }

                        let f = found?;
                        if f == var_start_token {
                            return None; // empty metavar match
                        }
                        code_idx = f; // next literal will be matched at `f`
                        f - 1
                    }
                    Some(PatternToken::Metavar(_)) | None => {
                        // Consumes a single balanced expression (token or delimited group)
                        let mut cur = code_idx;
                        if let TokenKind::OpenDelim(_) = tokens[cur].kind
                            && let Some(close_idx) = matching_delims[cur]
                        {
                            cur = close_idx;
                        }
                        code_idx = cur + 1;
                        cur
                    }
                };

                let start_b = tokens[var_start_token].start_byte;
                let end_b = tokens[var_end_token].end_byte;
                let snippet = source[start_b..end_b].trim();

                // Multi-occurrence consistency check
                if let Some(&(prev_start, prev_end)) = bindings.get(var_name) {
                    let prev_snippet = source[prev_start..prev_end].trim();
                    if prev_snippet != snippet {
                        return None; // Inconsistent metavar binding
                    }
                } else {
                    bindings.insert(var_name.clone(), (start_b, end_b));
                }

                pat_idx += 1;
            }
        }
    }

    if start_idx >= tokens.len() || code_idx == start_idx {
        return None;
    }

    let match_start_byte = tokens[start_idx].start_byte;
    let match_end_byte = tokens[code_idx - 1].end_byte;

    Some((match_start_byte, match_end_byte, code_idx, bindings))
}

/// Attempt to match a pattern starting at `start_idx` in `tokens`.
fn match_at(
    tokens: &[SourceToken],
    start_idx: usize,
    matching_delims: &[Option<usize>],
    rule: &CodemodRule,
    source: &str,
) -> Option<CodemodMatch> {
    let (match_start_byte, match_end_byte, _, bindings) =
        match_pattern_tokens(tokens, start_idx, matching_delims, &rule.pattern_tokens, source)?;

    // Synthesize replacement text
    let mut rep = String::new();
    for rep_tok in &rule.replacement_tokens {
        match rep_tok {
            ReplacementToken::Text(t) => rep.push_str(t),
            ReplacementToken::Metavar(name) => {
                if let Some(&(s, e)) = bindings.get(name) {
                    rep.push_str(&source[s..e]);
                } else {
                    // Unbound metavar in replacement, keep as is
                    rep.push('$');
                    rep.push_str(name);
                }
            }
        }
    }

    Some(CodemodMatch {
        start_byte: match_start_byte,
        end_byte: match_end_byte,
        replacement: rep,
    })
}

/// Find all non-overlapping structural matches in `source` and produce rewritten text if any matches occur.
pub fn rewrite_source(source: &str, rule: &CodemodRule) -> Option<String> {
    // Quick candidate rejection
    for lit in &rule.required_literals {
        if !source.contains(lit) {
            return None;
        }
    }

    let tokens = tokenize_source(source);
    if tokens.is_empty() {
        return None;
    }

    let matching_delims = compute_matching_delims(&tokens);
    let mut matches: Vec<CodemodMatch> = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        if let Some(m) = match_at(&tokens, i, &matching_delims, rule, source) {
            // Advance tokens past the end of the match
            while i < tokens.len() && tokens[i].end_byte <= m.end_byte {
                i += 1;
            }
            matches.push(m);
        } else {
            i += 1;
        }
    }

    if matches.is_empty() {
        return None;
    }

    // Reconstruct rewritten text
    let mut output = String::with_capacity(source.len());
    let mut last_end = 0;

    for m in matches {
        if m.start_byte >= last_end {
            output.push_str(&source[last_end..m.start_byte]);
            output.push_str(&m.replacement);
            last_end = m.end_byte;
        }
    }
    if last_end < source.len() {
        output.push_str(&source[last_end..]);
    }

    Some(output)
}

/// The result of executing a polyglot structural AST codemod.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemodOutcome {
    pub rule: String,
    pub files_scanned: usize,
    pub files_matched: usize,
    pub total_matches: usize,
    pub changed_lines: usize,
    pub diff: String,
    pub rewritten_files: Vec<(PathBuf, String)>,
    pub elapsed_ms: f64,
}

impl CodemodOutcome {
    /// Render human-readable summary and unified diff preview.
    pub fn render(&self, max_diff_chars: usize) -> String {
        let mut text = format!("`{}`\n", self.rule);
        text.push_str(&format!(
            "{} changed line(s) in {} file(s) ({} scanned in {:.2}ms)\n\n",
            self.changed_lines, self.files_matched, self.files_scanned, self.elapsed_ms
        ));

        if self.diff.is_empty() {
            text.push_str("matches nothing\n");
            return text;
        }

        if self.diff.len() > max_diff_chars {
            let cut: String = self.diff.chars().take(max_diff_chars).collect();
            text.push_str(&cut);
            text.push_str("\n… diff truncated\n");
        } else {
            text.push_str(&self.diff);
        }
        text
    }
}

/// Recursively collect all code files in `dir`.
fn collect_code_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();

        if path.is_dir() {
            if !SKIPPED_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                collect_code_files(&path, out);
            }
        } else if path.is_file()
            && let Some(ext) = path.extension().and_then(|e| e.to_str())
            && CODE_EXTENSIONS.contains(&ext)
        {
            out.push(path);
        }
    }
}

/// Run structural codemod across a file or workspace checkout.
pub fn run_codemod(
    workspace_root: &Path,
    rule_str: &str,
    scope: Option<&Path>,
    apply: bool,
) -> Result<CodemodOutcome> {
    let start = Instant::now();
    let rule = CodemodRule::parse(rule_str)?;

    let mut target_files = Vec::new();
    if let Some(target) = scope {
        if target.is_file() {
            target_files.push(target.to_path_buf());
        } else if target.is_dir() {
            collect_code_files(target, &mut target_files);
        } else {
            // Path might be relative to workspace root
            let abs = workspace_root.join(target);
            if abs.is_file() {
                target_files.push(abs);
            } else if abs.is_dir() {
                collect_code_files(&abs, &mut target_files);
            }
        }
    } else {
        collect_code_files(workspace_root, &mut target_files);
    }

    target_files.sort();

    let files_scanned = target_files.len();
    let mut rewritten_files = Vec::new();
    let mut unified_diffs = String::new();
    let mut changed_lines = 0;

    for path in &target_files {
        let Ok(old_text) = std::fs::read_to_string(path) else {
            continue;
        };

        if let Some(new_text) = rewrite_source(&old_text, &rule) {
            let rel = path
                .strip_prefix(workspace_root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned());

            let diff = similar::TextDiff::from_lines(&old_text, &new_text);
            let file_changed = diff
                .iter_all_changes()
                .filter(|c| c.tag() != similar::ChangeTag::Equal)
                .count();
            changed_lines += file_changed;

            unified_diffs.push_str(
                &diff
                    .unified_diff()
                    .context_radius(2)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );

            rewritten_files.push((path.clone(), new_text));
        }
    }

    let files_matched = rewritten_files.len();

    // If apply is requested and there are rewritten files, apply them atomically
    if apply && !rewritten_files.is_empty() {
        let mut document_changes = Vec::new();
        for (p, new_text) in &rewritten_files {
            let uri = url::Url::from_file_path(p)
                .map_err(|_| anyhow::anyhow!("invalid path {:?}", p))?
                .to_string();
            document_changes.push(serde_json::json!({
                "textDocument": { "uri": uri },
                "edits": [{
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 999999, "character": 0 },
                    },
                    "newText": new_text,
                }],
            }));
        }
        let workspace_edit = serde_json::json!({ "documentChanges": document_changes });
        crate::refactor::apply_workspace_edit(workspace_root, &workspace_edit)?;
    }

    let elapsed = start.elapsed();

    Ok(CodemodOutcome {
        rule: rule_str.to_string(),
        files_scanned,
        files_matched,
        total_matches: files_matched,
        changed_lines,
        diff: unified_diffs,
        rewritten_files,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
    })
}

/// Helper to convert a byte offset into 1-based (line, column).
pub fn byte_to_line_col(source: &str, byte_offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in source.char_indices() {
        if i >= byte_offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// One matched AST span in a structural search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StructuralMatchItem {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub matched_text: String,
    pub bindings: BTreeMap<String, String>,
}

/// The result of executing a read-only structural AST search across files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StructuralSearchResult {
    pub pattern: String,
    pub files_scanned: usize,
    pub files_matched: usize,
    pub total_matches: usize,
    pub matches: Vec<StructuralMatchItem>,
    pub elapsed_ms: f64,
}

impl StructuralSearchResult {
    /// Render human-readable summary of structural search matches.
    pub fn render(&self, max_items: usize) -> String {
        let mut text = format!("⚡ prod-code Structural AST Search: `{}`\n", self.pattern);
        text.push_str("────────────────────────────────────────────────────\n");
        text.push_str(&format!(
            "{} match(es) in {} file(s) ({} scanned in {:.2}ms)\n\n",
            self.total_matches, self.files_matched, self.files_scanned, self.elapsed_ms
        ));

        if self.matches.is_empty() {
            text.push_str("✓ No matches found for pattern.\n");
            return text;
        }

        for m in self.matches.iter().take(max_items) {
            let first_line = m.matched_text.lines().next().unwrap_or(&m.matched_text).trim();
            text.push_str(&format!("  • {}:{}:{}  {}\n", m.file, m.line, m.col, first_line));
            if !m.bindings.is_empty() {
                let binds: Vec<String> = m.bindings.iter().map(|(k, v)| format!("${k} = {v}")).collect();
                text.push_str(&format!("    └─ [{}]\n", binds.join(", ")));
            }
        }
        if self.matches.len() > max_items {
            text.push_str(&format!("\n  … and {} more match(es) truncated\n", self.matches.len() - max_items));
        }
        text
    }
}

/// Find all structural matches for a compiled pattern in a single source string.
pub fn find_structural_matches_in_source(
    rel_path: &str,
    source: &str,
    pattern: &CompiledPattern,
) -> Vec<StructuralMatchItem> {
    for lit in &pattern.required_literals {
        if !source.contains(lit) {
            return Vec::new();
        }
    }

    let tokens = tokenize_source(source);
    if tokens.is_empty() {
        return Vec::new();
    }

    let matching_delims = compute_matching_delims(&tokens);
    let mut matches = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        if let Some((start_b, end_b, _, bindings)) =
            match_pattern_tokens(&tokens, i, &matching_delims, &pattern.pattern_tokens, source)
        {
            let (line, col) = byte_to_line_col(source, start_b);
            let matched_text = source[start_b..end_b].to_string();
            let mut string_bindings = BTreeMap::new();
            for (k, (s, e)) in bindings {
                string_bindings.insert(k, source[s..e].trim().to_string());
            }

            matches.push(StructuralMatchItem {
                file: rel_path.to_string(),
                line,
                col,
                matched_text,
                bindings: string_bindings,
            });

            // Advance tokens past the end of the match
            while i < tokens.len() && tokens[i].end_byte <= end_b {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    matches
}

/// Run read-only structural AST search across files in the workspace.
pub fn run_structural_search(
    workspace_root: &Path,
    pattern_str: &str,
    scope: Option<&Path>,
) -> Result<StructuralSearchResult> {
    let start = Instant::now();
    let pattern = CompiledPattern::parse(pattern_str)?;

    let mut target_files = Vec::new();
    if let Some(target) = scope {
        if target.is_file() {
            target_files.push(target.to_path_buf());
        } else if target.is_dir() {
            collect_code_files(target, &mut target_files);
        } else {
            let abs = workspace_root.join(target);
            if abs.is_file() {
                target_files.push(abs);
            } else if abs.is_dir() {
                collect_code_files(&abs, &mut target_files);
            }
        }
    } else {
        collect_code_files(workspace_root, &mut target_files);
    }

    target_files.sort();
    let files_scanned = target_files.len();
    let mut all_matches = Vec::new();
    let mut matched_files_set = std::collections::HashSet::new();

    for path in &target_files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };

        let rel = path
            .strip_prefix(workspace_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());

        let file_matches = find_structural_matches_in_source(&rel, &source, &pattern);
        if !file_matches.is_empty() {
            matched_files_set.insert(rel);
            all_matches.extend(file_matches);
        }
    }

    let elapsed = start.elapsed();

    Ok(StructuralSearchResult {
        pattern: pattern_str.to_string(),
        files_scanned,
        files_matched: matched_files_set.len(),
        total_matches: all_matches.len(),
        matches: all_matches,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_go_error_wrapping_codemod() {
        let rule = CodemodRule::parse("errors.Wrap($err, $msg) ==>> fmt.Errorf(\"%s: %w\", $msg, $err)")
            .expect("valid rule");
        let src = r#"package main

import "errors"

func test() error {
    err := read()
    if err != nil {
        return errors.Wrap(err, "read failed")
    }
    return nil
}
"#;
        let rewritten = rewrite_source(src, &rule).expect("should match");
        assert!(rewritten.contains(r#"return fmt.Errorf("%s: %w", "read failed", err)"#));
    }

    #[test]
    fn test_typescript_logger_codemod() {
        let rule = CodemodRule::parse("console.log($msg) ==>> logger.info($msg)")
            .expect("valid rule");
        let src = r#"function login(user: User) {
    console.log("user logged in: " + user.id);
}
"#;
        let rewritten = rewrite_source(src, &rule).expect("should match");
        assert!(rewritten.contains(r#"logger.info("user logged in: " + user.id);"#));
    }

    #[test]
    fn test_python_pathlib_codemod() {
        let rule = CodemodRule::parse("os.path.join($a, $b) ==>> Path($a) / $b")
            .expect("valid rule");
        let src = r#"import os

def get_config():
    p = os.path.join(base_dir, "config.json")
    return p
"#;
        let rewritten = rewrite_source(src, &rule).expect("should match");
        assert!(rewritten.contains(r#"p = Path(base_dir) / "config.json""#));
    }

    #[test]
    fn test_cpp_smart_pointer_codemod() {
        let rule = CodemodRule::parse("std::make_shared<$T>($args) ==>> std::allocate_shared<$T>(alloc, $args)")
            .expect("valid rule");
        let src = r#"#include <memory>

void make() {
    auto ptr = std::make_shared<Widget>(42, "test");
}
"#;
        let rewritten = rewrite_source(src, &rule).expect("should match");
        assert!(rewritten.contains(r#"auto ptr = std::allocate_shared<Widget>(alloc, 42, "test");"#));
    }

    #[test]
    fn test_swift_os_log_codemod() {
        let rule = CodemodRule::parse("print($x) ==>> os_log($x)")
            .expect("valid rule");
        let src = r#"func log() {
    print("operation succeeded")
}
"#;
        let rewritten = rewrite_source(src, &rule).expect("should match");
        assert!(rewritten.contains(r#"os_log("operation succeeded")"#));
    }

    #[test]
    fn test_multi_occurrence_consistency() {
        let rule = CodemodRule::parse("compare($a, $a) ==>> 0").expect("valid rule");
        let match_src = "let res = compare(x, x);";
        let no_match_src = "let res = compare(x, y);";

        assert_eq!(rewrite_source(match_src, &rule), Some("let res = 0;".to_string()));
        assert_eq!(rewrite_source(no_match_src, &rule), None);
    }

    #[test]
    fn test_multiline_whitespace_invariance() {
        let rule = CodemodRule::parse("calc($a, $b) ==>> compute($b, $a)").expect("valid rule");
        let src = r#"let val = calc(
    firstArgument + 1,
    secondArgument * 2
);
"#;
        let rewritten = rewrite_source(src, &rule).expect("should match");
        assert!(rewritten.contains("compute(secondArgument * 2, firstArgument + 1)"));
    }

    #[test]
    fn test_structural_search_ast() {
        let pattern = CompiledPattern::parse("$a.unwrap()").expect("valid pattern");
        let src = r#"
fn run() {
    let x = opt.unwrap();
    let y = calc(1, 2);
    let z = map.get(&k).unwrap();
}
"#;
        let matches = find_structural_matches_in_source("test.rs", src, &pattern);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].bindings.get("a").unwrap(), "opt");
        assert_eq!(matches[1].bindings.get("a").unwrap(), "get(&k)");
    }

    #[test]
    fn test_structural_search_unicode_multibyte_chars() {
        // Multi-byte UTF-8 characters like '€' (3 bytes), '✓' (3 bytes), non-ASCII docstrings
        let pattern = CompiledPattern::parse("$x.price()").expect("valid pattern");
        let src = r#"
/// Price in €/kg or £/lb or ¥
fn test_currency() {
    let apple = item.price();
    let label = "Apple Price (€/kg)";
}
"#;
        let matches = find_structural_matches_in_source("test.rs", src, &pattern);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].bindings.get("x").unwrap(), "item");

        let tokens = tokenize_source("let symbol = €; let name = café;");
        assert!(tokens.iter().any(|t| matches!(&t.kind, TokenKind::Punct(p) if p == "€")));
        assert!(tokens.iter().any(|t| matches!(&t.kind, TokenKind::Ident(id) if id.contains("caf"))));

        // Pattern and codemod rule parsing with multi-byte Unicode characters
        let rule = CodemodRule::parse("$x.price(€) ==>> $x.cost(¥)").expect("valid rule with unicode");
        let src = "let r = item.price(€);";
        let rewritten = rewrite_source(src, &rule).expect("should match and rewrite");
        assert_eq!(rewritten, "let r = item.cost(¥);");
    }
}
