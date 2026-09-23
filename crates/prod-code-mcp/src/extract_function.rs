//! Extracting a function, and replacing the selection's duplicates with the same call.
//!
//! rust-analyzer's `extract_function` turns one selection into a call to a new `fun_name`. The
//! same lines often stand elsewhere: in the file, and with `other_files` in the crate's other
//! files. A place whose tokens are the selection's becomes the same call; with `parameterize`, so
//! does one that differs only in literals, each of which becomes a parameter of the new function
//! (#212). A place is kept only if the result type-checks with it, and not at all when the code
//! after it reads a name the selection binds and the call does not return (#189): the type check
//! alone cannot tell a shadowed name. rust-analyzer does not check borrows either, so a result
//! with a replaced copy is compiled before it is written.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// The name rust-analyzer gives the function it extracts.
const PLACEHOLDER: &str = "fun_name";

/// More duplicates than this are reported, not tried: each one costs a type check.
const MAX_DUPLICATES: usize = 8;

/// One other place with the selection's text, and what became of it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Duplicate {
    /// The file it is in, relative to the checkout.
    pub file: String,
    pub line: u32,
    pub replaced: bool,
    pub reason: Option<String>,
    /// The literals it passes, when it differs from the selection only in literals.
    pub passes: Vec<String>,
}

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Extracted {
    pub name: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    /// What the selection now reads.
    pub call: String,
    /// Parameters the new function gained for literals the duplicates differ in, `name: type`.
    pub parameters: Vec<String>,
    pub duplicates: Vec<Duplicate>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Extracted {
    /// How many duplicates now call the new function.
    pub fn replaced(&self) -> usize {
        self.duplicates.iter().filter(|d| d.replaced).count()
    }

    pub fn render(&self) -> String {
        let mut diff = String::new();
        for (path, new_text) in &self.rewritten {
            let path = Path::new(path);
            let old_text = if self.applied {
                crate::refactor::text_before_apply(path)
            } else {
                std::fs::read_to_string(path).unwrap_or_default()
            };
            let shown = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            diff.push_str(
                &similar::TextDiff::from_lines(old_text.as_str(), new_text)
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{shown}"), &format!("b/{shown}"))
                    .to_string(),
            );
        }
        let mut out = format!(
            "`fn {}` extracted ({}); the selection now reads `{}`\n",
            self.name,
            self.file,
            self.call.trim()
        );
        if !self.parameters.is_empty() {
            out.push_str(&format!(
                "- new parameter(s) for the literals the copies differ in: {}\n",
                self.parameters.join(", ")
            ));
        }
        if self.duplicates.is_empty() {
            out.push_str("- no other place in the file has the selection's text\n");
        }
        for d in &self.duplicates {
            let at = if d.file == self.file {
                format!("line {}", d.line)
            } else {
                format!("{}:{}", d.file, d.line)
            };
            let passing = if d.passes.is_empty() {
                String::new()
            } else {
                format!(" passing {}", d.passes.join(", "))
            };
            match (&d.reason, d.replaced) {
                (_, true) => out.push_str(&format!(
                    "- {at}: the same code, now the same call{passing}\n"
                )),
                (Some(reason), false) => {
                    out.push_str(&format!("- {at}: left as it is: {reason}\n"))
                }
                (None, false) => out.push_str(&format!("- {at}: left as it is\n")),
            }
        }
        out.push('\n');
        out.push_str(&diff);
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        out.push_str(if self.applied {
            "\n[applied]\n"
        } else {
            "\nnothing was written; pass `apply: true` to make this edit\n"
        });
        out
    }

    /// Writes the result, unless the analyzer rejects it and `force` is not given.
    pub fn write(&mut self, force: bool) -> Result<()> {
        anyhow::ensure!(
            self.diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            self.diagnostics.len(),
            self.diagnostics.join("\n  ")
        );
        let files: BTreeMap<PathBuf, String> = self
            .rewritten
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.clone()))
            .collect();
        crate::refactor::apply_workspace_edit(
            &self.root,
            &crate::signature::whole_file_edit(&files),
        )?;
        self.applied = true;
        Ok(())
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `text` holds `word` as a whole identifier.
fn mentions(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        !text[..i].chars().next_back().is_some_and(is_ident)
            && !text[i + word.len()..].chars().next().is_some_and(is_ident)
    })
}

/// The names the `let` statements in `code` bind: `let x`, `let mut x`, and the names in a
/// tuple or struct pattern (`let (a, b)`, `let P { x, y: py }` binds `x` and `py`).
pub fn bound_names(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in code.match_indices("let") {
        let whole = !code[..i].chars().next_back().is_some_and(is_ident)
            && code[i + 3..].starts_with(char::is_whitespace);
        if !whole {
            continue;
        }
        let rest = &code[i + 3..];
        let pattern_end = rest.find(['=', ';']).unwrap_or(rest.len());
        let mut pattern = &rest[..pattern_end];
        // `let x: T`: the type is not part of the pattern. A `:` inside braces is a field.
        let mut depth = 0i32;
        for (j, c) in pattern.char_indices() {
            match c {
                '(' | '{' | '[' | '<' => depth += 1,
                ')' | '}' | ']' | '>' => depth -= 1,
                ':' if depth == 0 => {
                    pattern = &pattern[..j];
                    break;
                }
                _ => {}
            }
        }
        let chars: Vec<(usize, char)> = pattern.char_indices().collect();
        let mut k = 0;
        while k < chars.len() {
            let (s, c) = chars[k];
            if !is_ident(c) {
                k += 1;
                continue;
            }
            let mut e = k;
            while e < chars.len() && is_ident(chars[e].1) {
                e += 1;
            }
            let end = chars.get(e).map_or(pattern.len(), |(i, _)| *i);
            let word = &pattern[s..end];
            let field = pattern[end..].trim_start().starts_with(':');
            let named = !matches!(word, "mut" | "ref" | "_")
                && !word.starts_with(|c: char| c.is_uppercase() || c.is_ascii_digit());
            if named && !field {
                out.push(word.to_string());
            }
            k = e;
        }
    }
    out.sort();
    out.dedup();
    out
}

/// A name the selection binds, the call does not bind again, and the code after the place
/// ending at `to` in its function reads (#189). Such a place cannot take the call: the read
/// would find an outer binding of the name, or none, and only the second is an error the
/// analyzer reports.
pub fn read_after_but_not_returned(
    text: &str,
    selection: &str,
    call: &str,
    to: usize,
) -> Option<String> {
    let returned = bound_names(call);
    let (_, close) = crate::introduce_variable::enclosing_body(text, to.saturating_sub(1))?;
    let after = &text[to.min(close)..close];
    bound_names(selection)
        .into_iter()
        .filter(|name| !returned.contains(name))
        .find(|name| mentions(after, name))
}

/// How the extraction rewrote the file: `new` is `old[..start]`, then `call`, then
/// `old[end..inserted_at]` (the rest of the function that held the selection), then the new
/// function (`function_len` bytes), then `old[inserted_at..]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Rewrite {
    pub call: String,
    pub inserted_at: usize,
    pub function_len: usize,
}

impl Rewrite {
    /// Where `old[at..to]`, which the extraction did not touch, is in the new text.
    pub fn mapped(&self, start: usize, end: usize, at: usize, to: usize) -> Option<usize> {
        let grown = self.call.len() as isize - (end - start) as isize;
        if to <= start {
            Some(at)
        } else if end <= at && to <= self.inserted_at {
            Some((at as isize + grown) as usize)
        } else if self.inserted_at <= at {
            Some((at as isize + grown) as usize + self.function_len)
        } else {
            None
        }
    }
}

/// Reads [`Rewrite`] off the text before and after rust-analyzer extracted `old[start..end]`
/// into `fun_name`. A line diff cannot: the new function's body is the selection, so the diff
/// pairs the selection with it and calls the call the change. `None` when the extraction did
/// more than replace the selection and add one function after it: the text before the
/// selection differs, the end of the file after the new function differs, or the rest of the
/// function that held the selection is not found before the new one.
pub fn rewrite_of(old: &str, new: &str, start: usize, end: usize) -> Option<Rewrite> {
    if !old.is_char_boundary(start) || !new.is_char_boundary(start) || old[..start] != new[..start]
    {
        return None;
    }
    let def = new
        .match_indices(&format!("fn {PLACEHOLDER}"))
        .map(|(i, _)| i)
        .find(|i| {
            !new[i + 3 + PLACEHOLDER.len()..]
                .chars()
                .next()
                .is_some_and(is_ident)
        })?;
    if def < start {
        return None;
    }
    let open = def + new[def..].find('{')?;
    let function_end = crate::parameter_object::matching_bracket(new, open)? + 1;
    // After the new function the two texts agree to the end.
    let tail = new.len() - function_end;
    if tail > old.len() || old[old.len() - tail..] != new[function_end..] {
        return None;
    }
    let inserted_at = old.len() - tail;
    if inserted_at < end {
        return None;
    }
    let rest = &old[end..inserted_at];
    // The new function starts after the rest, with whatever blank lines and modifiers precede
    // `fn`: the latest point before the definition's line where the rest ends.
    let def_line = new[..def].rfind('\n').map_or(0, |i| i + 1);
    let function_start = (start..=def_line)
        .rev()
        .filter(|x| new.is_char_boundary(*x))
        .find(|x| new[..*x].ends_with(rest) && *x >= start + rest.len())?;
    let call = new[start..function_start - rest.len()].to_string();
    let rewrite = Rewrite {
        call,
        inserted_at,
        function_len: function_end - function_start,
    };
    // The checks above already pin every part to `new`; joined again they are `new`, so there is
    // nothing left to compare (#192).
    (!rewrite.call.trim().is_empty()).then_some(rewrite)
}

/// The indentation of the line that holds `at`.
fn indent_at(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let width = text[line..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(char::len_utf8)
        .sum::<usize>();
    &text[line..line + width]
}

/// `text` with every whole `fun_name` made `name`.
fn rename_placeholder(text: &str, name: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (i, _) in text.match_indices(PLACEHOLDER) {
        let whole = !text[..i].chars().next_back().is_some_and(is_ident)
            && !text[i + PLACEHOLDER.len()..]
                .chars()
                .next()
                .is_some_and(is_ident);
        if whole && i >= at {
            out.push_str(&text[at..i]);
            out.push_str(name);
            at = i + PLACEHOLDER.len();
        }
    }
    out.push_str(&text[at..]);
    out
}

/// What a token of Rust source is, as far as matching copies needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Word,
    Number,
    Str,
    Char,
    Punct,
}

/// The tokens of `text`, as (kind, start, end): names and keywords, number, string and char
/// literals, and every other character on its own. Whitespace and `//` comments are skipped.
pub fn tokens(text: &str) -> Vec<(Token, usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = text[i..].chars().next().unwrap_or(' ');
        let len = c.len_utf8();
        if c.is_whitespace() {
            i += len;
        } else if text[i..].starts_with("//") {
            i = text[i..].find('\n').map_or(text.len(), |n| i + n);
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'.')
            {
                // `1..3` is two numbers and a range, not one number.
                if bytes[j] == b'.' && bytes.get(j + 1) == Some(&b'.') {
                    break;
                }
                j += 1;
            }
            out.push((Token::Number, i, j));
            i = j;
        } else if c == '"' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'"' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let j = (j + 1).min(bytes.len());
            out.push((Token::Str, i, j));
            i = j;
        } else if c == '\'' {
            // A char literal closes within a few bytes; a lifetime does not close at all.
            let close = text[i + 1..]
                .char_indices()
                .take(4)
                .find(|(k, ch)| *ch == '\'' && *k > 0)
                .map(|(k, _)| i + 1 + k);
            match close {
                Some(end)
                    if !text[i + 1..end].starts_with(|ch: char| ch.is_alphabetic())
                        || end - i <= 3 =>
                {
                    out.push((Token::Char, i, end + 1));
                    i = end + 1;
                }
                _ => {
                    let mut j = i + 1;
                    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
                    {
                        j += 1;
                    }
                    out.push((Token::Word, i, j));
                    i = j;
                }
            }
        } else if is_ident(c) {
            let mut j = i;
            while let Some(ch) = text[j..].chars().next() {
                if !is_ident(ch) {
                    break;
                }
                j += ch.len_utf8();
            }
            out.push((Token::Word, i, j));
            i = j;
        } else {
            out.push((Token::Punct, i, i + len));
            i += len;
        }
    }
    out
}

/// A place in a file whose tokens are the selection's, as byte range, with the token index and
/// text of every literal at which it differs.
#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub from: usize,
    pub to: usize,
    pub differs: Vec<(usize, String)>,
}

/// The places in `text` whose tokens are those of `selection`, outside `exclude`. With
/// `literals`, a place may differ from it in literals of the same kind (a number for a
/// number, a string for a string); without, it must be the same token for token. When the
/// selection is statements (it ends with `;` or `}`), a place must start a statement too.
pub fn copies_of(
    selection: &str,
    text: &str,
    exclude: Option<(usize, usize)>,
    literals: bool,
) -> Vec<Occurrence> {
    let wanted = tokens(selection);
    let have = tokens(text);
    if wanted.is_empty() || wanted.len() > have.len() {
        return Vec::new();
    }
    let statements = selection.trim_end().ends_with(';') || selection.trim_end().ends_with('}');
    let token = |src: &str, (_, s, e): (Token, usize, usize)| -> String { src[s..e].to_string() };
    let mut out = Vec::new();
    let mut k = 0;
    while k + wanted.len() <= have.len() {
        let window = &have[k..k + wanted.len()];
        let mut differs = Vec::new();
        let same = wanted.iter().zip(window).enumerate().all(|(n, (w, h))| {
            let (wt, ht) = (token(selection, *w), token(text, *h));
            if wt == ht {
                return true;
            }
            let literal = matches!(w.0, Token::Number | Token::Str | Token::Char);
            if literals && literal && w.0 == h.0 {
                differs.push((n, ht));
                return true;
            }
            false
        });
        let (from, to) = (window[0].1, window[window.len() - 1].2);
        let overlaps = exclude.is_some_and(|(s, e)| from < e && s < to);
        let starts_statement = !statements
            || text[..from]
                .trim_end()
                .chars()
                .next_back()
                .is_none_or(|c| matches!(c, '{' | ';' | '}'));
        if same && !overlaps && starts_statement {
            out.push(Occurrence { from, to, differs });
            k += wanted.len();
        } else {
            k += 1;
        }
    }
    out
}

/// The type in a hover on a literal: the first code block, when it is one line that reads like
/// a type (`u32`, `&str`, `f64`).
pub fn literal_type(hover: &str) -> Option<String> {
    let block = hover.split("```").nth(1)?;
    let body = block.strip_prefix("rust").unwrap_or(block).trim();
    let one_line = !body.contains('\n') && !body.is_empty();
    let looks_like_type = body.chars().all(|c| {
        c.is_alphanumeric() || matches!(c, '_' | '&' | '\'' | ':' | '<' | '>' | ' ' | ',')
    }) && !body.contains("fn ")
        && !body.starts_with("let ");
    (one_line && looks_like_type).then(|| body.to_string())
}

/// `text` with every edit (byte range of `text`, replacement) applied; the edits do not overlap.
fn apply_edits(text: &str, edits: &[(usize, usize, String)]) -> String {
    let mut sorted = edits.to_vec();
    sorted.sort_by_key(|(from, _, _)| std::cmp::Reverse(*from));
    let mut out = text.to_string();
    for (from, to, replacement) in sorted {
        out.replace_range(from..to, &replacement);
    }
    out
}

/// `call` (`let net = fun_name(o);`) with `extra` appended to the arguments of `fun_name`.
pub fn with_arguments(call: &str, extra: &[String]) -> Option<String> {
    if extra.is_empty() {
        return Some(call.to_string());
    }
    let at = call.find(&format!("{PLACEHOLDER}("))? + PLACEHOLDER.len();
    let close = crate::parameter_object::matching_bracket(call, at)?;
    let inside = call[at + 1..close].trim();
    let joined = extra.join(", ");
    let args = if inside.is_empty() {
        joined
    } else {
        format!("{inside}, {joined}")
    };
    Some(format!("{}{args}{}", &call[..at + 1], &call[close..]))
}

/// The errors the analyzer finds in `files` checked together.
async fn errors_in(
    remote: SocketAddr,
    root: &Path,
    files: &[(PathBuf, String)],
) -> Result<Vec<String>> {
    let reports = crate::diagnostics::validate_texts(remote, root, files, &[]).await?;
    Ok(reports
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
        .collect())
}

/// Every `.rs` file under the `src` of the crate that holds `file`, but `file`.
fn crate_sources(file: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let Some(src) = file.ancestors().find(|d| {
        d.file_name().is_some_and(|n| n == "src")
            && d.parent().is_some_and(|p| p.join("Cargo.toml").is_file())
    }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk(src, &mut out);
    out.retain(|p| p != file);
    out.sort();
    out
}

/// Extracts the selection `line`:`col` .. `end_line`:`end_col` of `file` into `fn name`, and,
/// with `duplicates`, replaces every other place in the file that has the same text with the
/// same call, where the result type-checks. Nothing is written; see [`Extracted::write`].
#[allow(clippy::too_many_arguments)]
pub async fn extract_function(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    (line, col): (u32, u32),
    (end_line, end_col): (u32, u32),
    name: &str,
    duplicates: bool,
    parameterize: bool,
    other_files: bool,
) -> Result<Extracted> {
    anyhow::ensure!(
        !name.is_empty()
            && name.chars().all(is_ident)
            && !name.starts_with(|c: char| c.is_ascii_digit()),
        "`{name}` is not an identifier"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    anyhow::ensure!(
        !mentions(&text, PLACEHOLDER),
        "the file already uses `{PLACEHOLDER}`, the name rust-analyzer gives the function it \
         extracts; rename that first"
    );
    anyhow::ensure!(
        !mentions(&text, name),
        "the file already has something called `{name}`; choose another name"
    );
    let start =
        crate::signature::offset_of(&text, line, col).context("the start is not in the file")?;
    let end = crate::signature::offset_of(&text, end_line, end_col)
        .context("the end is not in the file")?;
    anyhow::ensure!(start < end, "the selection is empty");

    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    let edit = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "prodCode/applyAssist",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": line - 1, "character": col - 1 },
                "end": { "line": end_line - 1, "character": end_col - 1 }
            },
            "id": "extract_function",
        }),
    )
    .await
    .context("rust-analyzer cannot extract a function from this selection")?;
    let (planned, _) = crate::refactor::planned_texts(root, &edit)?;
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let extracted = planned
        .into_iter()
        .find(|(p, _)| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == canonical)
        .map(|(_, t)| t)
        .context("the assist did not rewrite the file")?;

    let rewrite = rewrite_of(&text, &extracted, start, end);
    let call = rewrite.as_ref().map(|r| r.call.clone());
    let call_indent = indent_at(&text, start).to_string();
    let selection = text[start..end].trim().to_string();
    let selection_at = start + (text[start..end].len() - text[start..end].trim_start().len());
    let sel_tokens = tokens(&selection);

    // Every copy of the selection: in this file, and with `other_files` in the crate's other
    // files; with `parameterize`, also the ones that differ only in literals.
    let mut copies: Vec<(PathBuf, String, Occurrence)> = Vec::new();
    if duplicates {
        for c in copies_of(&selection, &text, Some((start, end)), parameterize) {
            copies.push((file.to_path_buf(), text.clone(), c));
        }
        if other_files {
            for other in crate_sources(file) {
                let Ok(other_text) = std::fs::read_to_string(&other) else {
                    continue;
                };
                for c in copies_of(&selection, &other_text, None, parameterize) {
                    copies.push((other.clone(), other_text.clone(), c));
                }
            }
        }
    }

    // A literal some copy differs in becomes a parameter, typed as the analyzer types the
    // selection's own literal there.
    let mut varying: Vec<usize> = copies
        .iter()
        .flat_map(|(_, _, c)| c.differs.iter().map(|(k, _)| *k))
        .collect();
    varying.sort_unstable();
    varying.dedup();
    let mut parameters: Vec<(String, String)> = Vec::new();
    let mut untyped: Option<String> = None;
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    for (n, k) in varying.iter().enumerate() {
        let (_, ts, te) = sel_tokens[*k];
        let literal = &selection[ts..te];
        let (l, c) = crate::signature::line_col_at(&text, selection_at + ts);
        let hover = crate::tools::execute_lsp_query(
            remote,
            root,
            file,
            "textDocument/hover",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": l - 1, "character": c - 1 },
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
        let Some(ty) = literal_type(&hover) else {
            untyped = Some(literal.to_string());
            break;
        };
        let name = if varying.len() == 1 {
            "value".to_string()
        } else {
            format!("value{}", n + 1)
        };
        parameters.push((name, ty));
    }
    if untyped.is_some() {
        parameters.clear();
    }

    // The new function, in the extracted text: its range, where its parameter list closes,
    // and where the selection's literals are in its body.
    let function = rewrite.as_ref().map(|r| {
        let at = start + r.call.len() + (r.inserted_at - end);
        (at, at + r.function_len)
    });
    let mut base_edits: Vec<(usize, usize, String)> = Vec::new();
    let mut parameterized = parameters.is_empty();
    if let (Some((fs, fe)), false) = (function, parameters.is_empty()) {
        let body = &extracted[fs..fe];
        let def = body.find(&format!("fn {PLACEHOLDER}")).map(|i| fs + i + 3);
        let list = def.and_then(|d| crate::signature::param_span(&extracted, d));
        let window = tokens(body).windows(sel_tokens.len()).position(|w| {
            w.iter()
                .zip(&sel_tokens)
                .all(|(a, b)| body[a.1..a.2] == selection[b.1..b.2])
        });
        if let (Some((_, open, close)), Some(w)) = (list, window) {
            let body_tokens = tokens(body);
            let declared = parameters
                .iter()
                .map(|(n, ty)| format!("{n}: {ty}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sep = if extracted[open..close].trim().is_empty() {
                ""
            } else {
                ", "
            };
            base_edits.push((close, close, format!("{sep}{declared}")));
            for (n, k) in varying.iter().enumerate() {
                let (_, bs, be) = body_tokens[w + k];
                base_edits.push((fs + bs, fs + be, parameters[n].0.clone()));
            }
            parameterized = true;
        }
    }
    // The selection's own call passes the selection's literals.
    let own_literals: Vec<String> = varying
        .iter()
        .map(|k| selection[sel_tokens[*k].1..sel_tokens[*k].2].to_string())
        .collect();
    let call_for = |literals: &[String]| -> Option<String> {
        let c = call.as_deref()?;
        if parameters.is_empty() {
            Some(c.to_string())
        } else {
            with_arguments(c, literals)
        }
    };
    if let (Some(c), false) = (call.as_deref(), parameters.is_empty()) {
        if let Some(own) = call_for(&own_literals) {
            base_edits.push((start, start + c.len(), own));
        }
    }

    // How another file names the new function: through its module, and it has to be at least
    // `pub(crate)` then.
    // Only a copy in another file needs the module; a checkout without the ordinary layout
    // can still have its copies in this file replaced.
    let home = if other_files {
        Some(crate::move_item::module_of(file)?.1)
    } else {
        None
    };
    let is_method = call
        .as_deref()
        .is_some_and(|c| c.contains("self.") || c.contains("Self::"));

    type Target = (PathBuf, usize, usize, String, String);
    let mut found: Vec<(Duplicate, Option<Target>)> = Vec::new();
    for (n, (path, path_text, c)) in copies.iter().enumerate() {
        let shown = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let line = crate::signature::line_col_at(path_text, c.from).0;
        let literals: Vec<String> = varying
            .iter()
            .map(|k| {
                c.differs
                    .iter()
                    .find(|(dk, _)| dk == k)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| selection[sel_tokens[*k].1..sel_tokens[*k].2].to_string())
            })
            .collect();
        let same_file = path == file;
        let place = if same_file {
            rewrite
                .as_ref()
                .and_then(|r| r.mapped(start, end, c.from, c.to))
                .map(|at| (at, at + (c.to - c.from)))
        } else {
            Some((c.from, c.to))
        };
        let qualify = |t: String| match &home {
            Some(home) if !same_file => {
                let (_, module) = crate::move_item::module_of(path)
                    .unwrap_or_else(|_| (PathBuf::new(), home.clone()));
                t.replace(
                    &format!("{PLACEHOLDER}("),
                    &format!("{}::{PLACEHOLDER}(", home.spelled_from(&module.krate)),
                )
            }
            _ => t,
        };
        // The call without the literal parameters, and with them (the copy's own literals).
        let plain_call = call.clone().map(qualify);
        let param_call = call_for(&literals).map(qualify);
        let reason = if n >= MAX_DUPLICATES {
            Some(format!("not tried: only the first {MAX_DUPLICATES} are"))
        } else if call.is_none() {
            Some("the extraction changed code around the selection too, so its call does not stand on its own".to_string())
        } else if !c.differs.is_empty() && !parameterized {
            Some(match &untyped {
                Some(lit) => {
                    format!("it differs in literals, and the type of `{lit}` is not known")
                }
                None => "it differs in literals the new function could not take".to_string(),
            })
        } else if !same_file && is_method {
            Some("the new function is a method; another file cannot call it through `self`".into())
        } else if place.is_none() {
            Some("the extraction itself changed this code".to_string())
        } else {
            call.as_deref()
                .and_then(|call| read_after_but_not_returned(path_text, &selection, call, c.to))
                .map(|name| {
                    format!(
                        "the code after it reads `{name}`, which the new function does not \
                         return; with the call there, `{name}` would be whatever it is before \
                         it, or nothing"
                    )
                })
        };
        let target = match (reason.is_none(), place, plain_call, param_call) {
            (true, Some((from, to)), Some(plain), Some(with)) => {
                Some((path.clone(), from, to, plain, with))
            }
            _ => None,
        };
        found.push((
            Duplicate {
                file: shown,
                line,
                replaced: false,
                reason,
                passes: if c.differs.is_empty() {
                    Vec::new()
                } else {
                    literals
                },
            },
            target,
        ));
    }

    // Every file with its accepted places, built from the extraction and the base edits.
    let build = |base: &[(usize, usize, String)],
                 accepted: &[(PathBuf, usize, usize, String)]|
     -> Vec<(PathBuf, String)> {
        let mut main_edits = base.to_vec();
        let mut others: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
        for (path, from, to, call_text) in accepted {
            let here = if path == file {
                indent_at(&extracted, *from).to_string()
            } else {
                indent_at(&std::fs::read_to_string(path).unwrap_or_default(), *from).to_string()
            };
            let placed = call_text
                .replace(&format!("\n{call_indent}"), &format!("\n{here}"))
                .trim()
                .to_string();
            if path == file {
                main_edits.push((*from, *to, placed));
            } else {
                others
                    .entry(path.clone())
                    .or_default()
                    .push((*from, *to, placed));
            }
        }
        // A copy in another file needs the function visible there.
        if !others.is_empty()
            && let Some((fs, _)) = function
            && let Some(def) = extracted[fs..].find(&format!("fn {PLACEHOLDER}"))
            && !extracted[fs..fs + def].contains("pub")
        {
            main_edits.push((fs + def, fs + def, "pub(crate) ".to_string()));
        }
        let mut files = vec![(
            file.to_path_buf(),
            rename_placeholder(&apply_edits(&extracted, &main_edits), name),
        )];
        for (path, edits) in others {
            let original = std::fs::read_to_string(&path).unwrap_or_default();
            files.push((
                path,
                rename_placeholder(&apply_edits(&original, &edits), name),
            ));
        }
        files
    };

    // First the exact copies, with the function as rust-analyzer extracted it; then, when some
    // copy differs in literals, the same with the literals as parameters. The parameters stay
    // only if a copy that needs them is kept.
    let mut accepted: Vec<(PathBuf, usize, usize, String)> = Vec::new();
    let mut result = build(&[], &accepted);
    let mut diagnostics = errors_in(remote, root, &result).await?;
    let base_clean = diagnostics.is_empty();
    let mut kept: Vec<usize> = Vec::new();
    for (n, (duplicate, target)) in found.iter_mut().enumerate() {
        let Some((path, from, to, plain, _)) = target.clone() else {
            continue;
        };
        if !duplicate.passes.is_empty() {
            continue;
        }
        if !base_clean {
            duplicate.reason = Some("not tried: the extraction itself does not type-check".into());
            continue;
        }
        let mut trial = accepted.clone();
        trial.push((path, from, to, plain));
        let candidate = build(&[], &trial);
        let found_errors = errors_in(remote, root, &candidate).await?;
        if found_errors.is_empty() {
            accepted = trial;
            result = candidate;
            duplicate.replaced = true;
            kept.push(n);
        } else {
            duplicate.reason = Some(format!(
                "with the call there the result does not type-check: {}",
                found_errors[0]
            ));
        }
    }
    let near: Vec<usize> = found
        .iter()
        .enumerate()
        .filter(|(_, (d, t))| !d.passes.is_empty() && t.is_some())
        .map(|(n, _)| n)
        .collect();
    let mut with_parameters = false;
    if base_clean && parameterized && !parameters.is_empty() && !near.is_empty() {
        // The exact copies kept so far, now passing the selection's literals.
        let mut accepted_p: Vec<(PathBuf, usize, usize, String)> = kept
            .iter()
            .filter_map(|n| found[*n].1.clone())
            .map(|(p, f, t, _, with)| (p, f, t, with))
            .collect();
        let mut result_p = build(&base_edits, &accepted_p);
        if errors_in(remote, root, &result_p).await?.is_empty() {
            for n in near {
                let Some((path, from, to, _, with)) = found[n].1.clone() else {
                    continue;
                };
                let mut trial = accepted_p.clone();
                trial.push((path, from, to, with));
                let candidate = build(&base_edits, &trial);
                let found_errors = errors_in(remote, root, &candidate).await?;
                if found_errors.is_empty() {
                    accepted_p = trial;
                    result_p = candidate;
                    found[n].0.replaced = true;
                    with_parameters = true;
                } else {
                    found[n].0.reason = Some(format!(
                        "with the call there the result does not type-check: {}",
                        found_errors[0]
                    ));
                }
            }
        }
        if with_parameters {
            result = result_p;
        }
    }
    let call = if with_parameters {
        call_for(&own_literals)
    } else {
        call
    };
    if !with_parameters {
        parameters.clear();
    }
    if found.iter().any(|(d, _)| d.replaced) {
        diagnostics = Vec::new();
    }

    Ok(Extracted {
        name: name.to_string(),
        root: root.to_path_buf(),
        file: file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned(),
        call: rename_placeholder(call.as_deref().unwrap_or(&text[start..end]), name),
        parameters: parameters
            .iter()
            .map(|(n, ty)| format!("{n}: {ty}"))
            .collect(),
        duplicates: found.into_iter().map(|(d, _)| d).collect(),
        rewritten: result
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREE: &str = "pub fn invoice(o: &Order) -> u32 {\n    let gross = o.qty * o.price;\n    let net = gross - gross * o.discount / 100;\n    net + 5\n}\n\npub fn quote(o: &Order) -> u32 {\n    let gross = o.qty * o.price;\n        let net = gross - gross *  o.discount / 100;\n    net\n}\n\npub fn other(o: &Order) -> u32 {\n    let grossly = o.qty * o.price;\n    ungross(o)\n}\n";

    #[test]
    fn a_name_the_call_does_not_return_is_not_read_after_a_duplicate() {
        assert_eq!(
            bound_names(
                "let gross = a; let mut n: u32 = 1; let (x, _y) = p; let P { f, g: h } = q; let _ = z;"
            ),
            vec!["_y", "f", "gross", "h", "n", "x"]
        );
        let selection = "let gross = o.qty * o.price;\n    let net = gross - 1;";
        let call = "let net = net_price(o);";
        let shadowed = "fn s(o: &O) -> u32 {\n    let gross = 1000;\n    let gross = o.qty * o.price;\n    let net = gross - 1;\n    gross - net\n}\n";
        let to = shadowed.find("gross - 1;").unwrap() + "gross - 1;".len();
        assert_eq!(
            read_after_but_not_returned(shadowed, selection, call, to).as_deref(),
            Some("gross")
        );
        let fine = "fn q(o: &O) -> u32 {\n    let gross = o.qty * o.price;\n    let net = gross - 1;\n    net\n}\nfn later() { gross(); }\n";
        let to = fine.find("gross - 1;").unwrap() + "gross - 1;".len();
        assert_eq!(read_after_but_not_returned(fine, selection, call, to), None);
    }

    #[test]
    fn copies_are_found_token_for_token_and_only_whole() {
        let start = THREE.find("let gross").unwrap();
        let end = THREE.find("/ 100;").unwrap() + "/ 100;".len();
        let found = copies_of(&THREE[start..end], THREE, Some((start, end)), false);
        assert_eq!(found.len(), 1, "{found:?}");
        let c = &found[0];
        assert!(THREE[c.from..c.to].starts_with("let gross"));
        assert!(THREE[c.from..c.to].ends_with("/ 100;"));
        assert!(c.from > end && c.differs.is_empty());
        // An expression glued to a longer name is not the same code.
        let g = "let a = gross + 1; let b = ungross + 1; let c = (gross + 1);";
        let s = g.find("gross + 1").unwrap();
        let e = s + "gross + 1".len();
        let found = copies_of(&g[s..e], g, Some((s, e)), false);
        assert_eq!(found.len(), 1);
        assert_eq!(&g[found[0].from..found[0].to], "gross + 1");
        assert!(found[0].from > g.find("ungross").unwrap());
        // Statements must start a statement: `a.y = 1;` holds `y = 1;` and is not it.
        let h = "fn h() { y = 1; a.y = 1; if c { y = 1; } }";
        let s = h.find("y = 1;").unwrap();
        let found = copies_of("y = 1;", h, Some((s, s + 6)), false);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(h[..found[0].from].ends_with("{ "));
        assert!(copies_of("", "fn e() {}", None, false).is_empty());
    }

    #[test]
    fn a_copy_may_differ_in_literals_of_the_same_kind_only_when_asked() {
        let text = "a; x / 50; y / 100; x / \"s\"; x / 7;";
        let exact = copies_of("x / 100;", text, None, false);
        assert!(exact.is_empty(), "{exact:?}");
        let near = copies_of("x / 100;", text, None, true);
        let differs: Vec<_> = near.iter().map(|c| c.differs.clone()).collect();
        assert_eq!(
            differs,
            vec![vec![(2, "50".to_string())], vec![(2, "7".to_string())]]
        );
    }

    #[test]
    fn tokens_tell_literals_names_and_lifetimes_apart() {
        let text = "let s: &'a str = \"a \\\" b\"; let c = 'x'; for i in 1..3 { f(2.5u8) } // done";
        let kinds: Vec<(Token, &str)> = tokens(text)
            .iter()
            .map(|(k, s, e)| (*k, &text[*s..*e]))
            .collect();
        assert!(kinds.contains(&(Token::Word, "'a")), "{kinds:?}");
        assert!(kinds.contains(&(Token::Str, "\"a \\\" b\"")), "{kinds:?}");
        assert!(kinds.contains(&(Token::Char, "'x'")), "{kinds:?}");
        assert!(kinds.contains(&(Token::Number, "1")) && kinds.contains(&(Token::Number, "3")));
        assert!(kinds.contains(&(Token::Number, "2.5u8")), "{kinds:?}");
        assert!(
            !kinds.iter().any(|(_, s)| *s == "done"),
            "comments are skipped"
        );
    }

    #[test]
    fn a_literal_s_type_comes_from_the_hover_and_calls_gain_arguments() {
        assert_eq!(
            literal_type("```rust\nu32\n```\n---\n\nvalue of literal: ` 100 `").as_deref(),
            Some("u32")
        );
        assert_eq!(literal_type("```rust\n&str\n```").as_deref(), Some("&str"));
        assert_eq!(
            literal_type("```rust\nfn div(self, other: u32) -> u32\n```"),
            None
        );
        assert_eq!(literal_type("no code"), None);
        assert_eq!(
            with_arguments("let n = fun_name(o);", &["100".into()]).as_deref(),
            Some("let n = fun_name(o, 100);")
        );
        assert_eq!(
            with_arguments("fun_name()", &["1".into(), "2".into()]).as_deref(),
            Some("fun_name(1, 2)")
        );
        assert_eq!(with_arguments("x", &[]).as_deref(), Some("x"));
        assert_eq!(with_arguments("no call", &["1".into()]), None);
        let edited = apply_edits("abcdef", &[(1, 2, "X".into()), (4, 4, "Y".into())]);
        assert_eq!(edited, "aXcdYef");
    }

    #[test]
    fn the_rewrite_is_read_even_when_the_body_repeats_the_selection() {
        let old =
            "fn f(o: &O) -> u32 {\n    let g = o.a;\n    let n = g + 1;\n    n\n}\n\nfn z() {}\n";
        let new = "fn f(o: &O) -> u32 {\n    let n = fun_name(o);\n    n\n}\n\nfn fun_name(o: &O) -> u32 {\n    let g = o.a;\n    let n = g + 1;\n    n\n}\n\nfn z() {}\n";
        let start = old.find("let g").unwrap();
        let end = old.find("+ 1;").unwrap() + 4;
        let rewrite = rewrite_of(old, new, start, end).expect("a plain extraction");
        assert_eq!(rewrite.call, "let n = fun_name(o);");
        // Text after the selection and after the new function lands where it is in `new`.
        let n_at = old.find("    n\n}").unwrap();
        let mapped = rewrite.mapped(start, end, n_at, n_at + 5).unwrap();
        assert_eq!(&new[mapped..mapped + 5], "    n");
        let z = old.find("fn z").unwrap();
        let mapped = rewrite.mapped(start, end, z, z + 4).unwrap();
        assert_eq!(&new[mapped..mapped + 4], "fn z");
        // Inside the selection nothing maps; before it everything does, as it was.
        assert_eq!(rewrite.mapped(start, end, start, end), None);
        assert_eq!(rewrite.mapped(start, end, 0, 2), Some(0));
        // A change before the selection: the call does not stand on its own.
        let moved = new.replacen("fn f(o", "fn f(mut o", 1);
        assert_eq!(rewrite_of(old, &moved, start, end), None);
        // No new function, or one before the selection: not an extraction this can read.
        assert_eq!(rewrite_of(old, old, start, end), None);
    }

    #[test]
    fn the_placeholder_is_renamed_only_whole() {
        assert_eq!(
            rename_placeholder("fun_name(x) + fun_names", "f"),
            "f(x) + fun_names"
        );
        assert!(mentions("a fun_name b", PLACEHOLDER) && !mentions("fun_names", PLACEHOLDER));
    }
}
