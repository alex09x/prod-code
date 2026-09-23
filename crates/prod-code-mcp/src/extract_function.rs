//! Extracting a function, and replacing the selection's duplicates with the same call.
//!
//! rust-analyzer's `extract_function` turns one selection into a call to a new `fun_name`. The
//! same lines often stand elsewhere in the file. Each place whose text is the selection's
//! (whitespace aside) becomes the same call, if the result still type-checks with it. That
//! check is what makes it the same program: the call passes the same names, and they must mean
//! there what they meant at the selection. rust-analyzer does not check borrows. So a value the
//! call takes by value, which the code after a duplicate still uses, is seen only by the
//! compiler. A result with a replaced duplicate is compiled before it is written.

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
    pub line: u32,
    pub replaced: bool,
    pub reason: Option<String>,
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
        let old_text = if self.applied {
            crate::refactor::text_before_apply(&self.root.join(&self.file))
        } else {
            std::fs::read_to_string(self.root.join(&self.file)).unwrap_or_default()
        };
        let new_text = self
            .rewritten
            .first()
            .map(|(_, t)| t.as_str())
            .unwrap_or("");
        let diff = similar::TextDiff::from_lines(old_text.as_str(), new_text)
            .unified_diff()
            .context_radius(1)
            .header(&format!("a/{}", self.file), &format!("b/{}", self.file))
            .to_string();
        let mut out = format!(
            "`fn {}` extracted ({}); the selection now reads `{}`\n",
            self.name,
            self.file,
            self.call.trim()
        );
        if self.duplicates.is_empty() {
            out.push_str("- no other place in the file has the selection's text\n");
        }
        for d in &self.duplicates {
            match (&d.reason, d.replaced) {
                (_, true) => out.push_str(&format!(
                    "- line {}: the same code, now the same call\n",
                    d.line
                )),
                (Some(reason), false) => {
                    out.push_str(&format!("- line {}: left as it is: {reason}\n", d.line))
                }
                (None, false) => out.push_str(&format!("- line {}: left as it is\n", d.line)),
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

/// `text` with every whitespace run made one space, and for each byte of it the offset of the
/// byte of `text` it stands for.
pub fn normalized(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut map = Vec::with_capacity(text.len());
    let mut in_space = false;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
                map.push(i);
            }
            in_space = true;
            continue;
        }
        in_space = false;
        out.push(c);
        map.extend(std::iter::repeat_n(i, c.len_utf8()));
    }
    (out, map)
}

/// The other places in `text` whose text is `text[start..end]`, whitespace aside, as byte
/// ranges. A place glued to an identifier is not one, and neither is one that overlaps the
/// selection. When the selection is statements (it ends with `;` or `}`), a place must start a
/// statement too.
pub fn duplicates_of(text: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let selection = normalized(text[start..end].trim()).0;
    if selection.is_empty() {
        return Vec::new();
    }
    let statements = selection.ends_with(';') || selection.ends_with('}');
    let (flat, map) = normalized(text);
    let mut out = Vec::new();
    for (i, _) in flat.match_indices(selection.as_str()) {
        let from = map[i];
        let last = map[i + selection.len() - 1];
        let to = last + text[last..].chars().next().map_or(1, char::len_utf8);
        let glued_before = text[..from].chars().next_back().is_some_and(is_ident)
            && selection.chars().next().is_some_and(is_ident);
        let glued_after = text[to..].chars().next().is_some_and(is_ident)
            && selection.chars().next_back().is_some_and(is_ident);
        let overlaps = from < end && start < to;
        let starts_statement = !statements
            || text[..from]
                .trim_end()
                .chars()
                .next_back()
                .is_none_or(|c| matches!(c, '{' | ';' | '}'));
        if !glued_before && !glued_after && !overlaps && starts_statement {
            out.push((from, to));
        }
    }
    out
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

/// `new` with each of `places` (byte ranges of `new`) replaced by `call`, indented as the place
/// is, and the placeholder name replaced by `name`.
fn with_calls(
    new: &str,
    places: &[(usize, usize)],
    call: &str,
    call_indent: &str,
    name: &str,
) -> String {
    let mut out = new.to_string();
    let mut sorted = places.to_vec();
    sorted.sort_by_key(|p| std::cmp::Reverse(p.0));
    for (from, to) in sorted {
        let here = indent_at(new, from);
        let text = call.replace(&format!("\n{call_indent}"), &format!("\n{here}"));
        out.replace_range(from..to, text.trim());
    }
    rename_placeholder(&out, name)
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

/// The errors the analyzer finds in `text` as the new content of `file`.
async fn errors(remote: SocketAddr, root: &Path, file: &Path, text: &str) -> Result<Vec<String>> {
    let reports = crate::diagnostics::validate_texts(
        remote,
        root,
        &[(file.to_path_buf(), text.to_string())],
        &[],
    )
    .await?;
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
    let mut found: Vec<(Duplicate, Option<(usize, usize)>)> = Vec::new();
    if duplicates {
        for (n, (from, to)) in duplicates_of(&text, start, end).into_iter().enumerate() {
            let line = crate::signature::line_col_at(&text, from).0;
            let place = rewrite
                .as_ref()
                .and_then(|r| r.mapped(start, end, from, to))
                .map(|at| (at, at + (to - from)));
            let reason = if n >= MAX_DUPLICATES {
                Some(format!("not tried: only the first {MAX_DUPLICATES} are"))
            } else if call.is_none() {
                Some("the extraction changed code around the selection too, so its call does not stand on its own".to_string())
            } else if place.is_none() {
                Some("the extraction itself changed this code".to_string())
            } else {
                call.as_deref()
                    .and_then(|call| {
                        read_after_but_not_returned(&text, &text[start..end], call, to)
                    })
                    .map(|name| {
                        format!(
                            "the code after it reads `{name}`, which the new function does not \
                             return; with the call there, `{name}` would be whatever it is before \
                             it, or nothing"
                        )
                    })
            };
            let place = if reason.is_none() { place } else { None };
            found.push((
                Duplicate {
                    line,
                    replaced: false,
                    reason,
                },
                place,
            ));
        }
    }

    let call_text = call.clone().unwrap_or_default();
    let mut accepted: Vec<(usize, usize)> = Vec::new();
    let mut result = rename_placeholder(&extracted, name);
    let mut diagnostics = errors(remote, root, file, &result).await?;
    let base_clean = diagnostics.is_empty();
    for (duplicate, place) in found.iter_mut() {
        let Some(place) = *place else { continue };
        if !base_clean {
            duplicate.reason = Some("not tried: the extraction itself does not type-check".into());
            continue;
        }
        let mut trial = accepted.clone();
        trial.push(place);
        let candidate = with_calls(&extracted, &trial, &call_text, &call_indent, name);
        let found_errors = errors(remote, root, file, &candidate).await?;
        if found_errors.is_empty() {
            accepted = trial;
            result = candidate;
            duplicate.replaced = true;
        } else {
            duplicate.reason = Some(format!(
                "with the call there the result does not type-check: {}",
                found_errors[0]
            ));
        }
    }
    if !accepted.is_empty() {
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
        duplicates: found.into_iter().map(|(d, _)| d).collect(),
        rewritten: vec![(file.to_string_lossy().into_owned(), result)],
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
    fn duplicates_are_found_whitespace_aside_and_only_whole() {
        let start = THREE.find("let gross").unwrap();
        let end = THREE.find("/ 100;").unwrap() + "/ 100;".len();
        let found = duplicates_of(THREE, start, end);
        assert_eq!(found.len(), 1, "{found:?}");
        let (from, to) = found[0];
        assert!(THREE[from..to].starts_with("let gross"));
        assert!(THREE[from..to].ends_with("/ 100;"));
        assert!(from > end);
        // An expression glued to a longer name is not the same code.
        let g = "let a = gross + 1; let b = ungross + 1; let c = (gross + 1);";
        let s = g.find("gross + 1").unwrap();
        let found = duplicates_of(g, s, s + "gross + 1".len());
        assert_eq!(found.len(), 1);
        assert_eq!(&g[found[0].0..found[0].1], "gross + 1");
        assert!(found[0].0 > g.find("ungross").unwrap());
        // Statements must start a statement: `a.y = 1;` holds `y = 1;` and is not it.
        let h = "fn h() { y = 1; a.y = 1; if c { y = 1; } }";
        let s = h.find("y = 1;").unwrap();
        let found = duplicates_of(h, s, s + "y = 1;".len());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(h[..found[0].0].ends_with("{ "));
        assert!(duplicates_of("fn e() {}", 3, 3).is_empty());
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
    fn duplicates_become_the_call_at_their_own_indentation() {
        let new = "fn a() {\n    let n = fun_name(o);\n}\nfn b() {\n        let g = o.a; let n = g + 1;\n}\nfn fun_name() {}\n";
        let from = new.find("let g").unwrap();
        let to = new.find("+ 1;").unwrap() + 4;
        let out = with_calls(new, &[(from, to)], "let n = fun_name(o);", "    ", "net");
        assert!(
            out.contains("fn b() {\n        let n = net(o);\n}"),
            "{out}"
        );
        assert!(out.contains("fn net() {}"), "{out}");
        assert_eq!(
            rename_placeholder("fun_name(x) + fun_names", "f"),
            "f(x) + fun_names"
        );
        assert!(mentions("a fun_name b", PLACEHOLDER) && !mentions("fun_names", PLACEHOLDER));
    }

    #[test]
    fn whitespace_runs_become_one_space() {
        let (flat, map) = normalized("a  \n b");
        assert_eq!(flat, "a b");
        assert_eq!(map, vec![0, 1, 5]);
    }
}
