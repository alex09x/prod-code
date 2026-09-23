//! Changing a declared type, and reporting the whole shape of what that breaks.
//!
//! It moves the declaration and then tells the truth about the size of the job, site by site,
//! before any of it is done. Where a site's error is exactly the old type meeting the new one it
//! says what conversion would fix it. With `convert` it goes one step further and writes
//! `.into()` at those sites — but only where the analyzer, checking the whole overlay again,
//! accepts it. A wrong `.into()` inserted at forty call sites is the kind of plausible damage the
//! rest of these tools exist to avoid, so a conversion that does not type-check is taken back and
//! its site stays in the report, and a set of conversions that breaks anything else is dropped.

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// One place the new type does not fit, with enough context to judge it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Site {
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// The analyzer's message, first line.
    pub message: String,
    pub code: Option<String>,
    /// The line of source, trimmed.
    pub source: String,
    /// What would fix this site, when the shape of the error says so plainly.
    pub suggestion: Option<String>,
    /// Where the analyzer's range for it ends, 1-based line and column.
    #[serde(skip)]
    pub end: Option<(u32, u32)>,
}

/// A conversion written at a site: the value that was there, and the value that is there now.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Conversion {
    pub file: String,
    pub line: u32,
    pub was: String,
    pub now: String,
}

/// What the migration would do, and what it would leave to be done.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Migration {
    pub symbol: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub was: String,
    pub now: String,
    /// The declaration's file, rewritten.
    pub rewritten: Vec<(String, String)>,
    /// Every site the new type does not fit, in file order.
    pub sites: Vec<Site>,
    /// How many diagnostics the change caused on an attribute rather than on code: an error
    /// inside what a derive generates is reported at the derive, and none of them is a place
    /// anyone can edit. Ones the file already had are not counted at all (#79).
    pub in_attributes: usize,
    /// The sites `convert` turned into `.into()` calls the analyzer accepts.
    pub converted: Vec<Conversion>,
    /// Why `convert` wrote nothing although it had candidates, when it did not.
    pub conversion_note: Option<String>,
    pub applied: bool,
}

impl Migration {
    /// The report: the declaration, then the work the change creates.
    pub fn render(&self, budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: `{}`\n- now: `{}`\n",
            self.symbol, self.file, self.was, self.now
        );
        if !self.converted.is_empty() {
            out.push_str(&format!(
                "\n{} site(s) converted with `.into()`, each accepted by the analyzer:\n",
                self.converted.len()
            ));
            for c in &self.converted {
                out.push_str(&format!(
                    "  {}:{}  `{}` → `{}`\n",
                    c.file, c.line, c.was, c.now
                ));
            }
        }
        if let Some(note) = &self.conversion_note {
            out.push_str(&format!("\n{note}\n"));
        }
        if self.sites.is_empty() {
            out.push_str("\nnothing else has to change: the analyzer accepts the new type\n");
        } else {
            let files = self
                .sites
                .iter()
                .map(|s| s.file.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            out.push_str(&format!(
                "\n{} site(s) in {files} file(s) do not fit the new type:\n",
                self.sites.len()
            ));
            let mut current = String::new();
            for (written, site) in self.sites.iter().enumerate() {
                if site.file != current {
                    current = site.file.clone();
                    out.push_str(&format!("\n{current}\n"));
                }
                if written >= budget {
                    out.push_str(&format!(
                        "  … {} more site(s)\n",
                        self.sites.len() - written
                    ));
                    break;
                }
                out.push_str(&format!(
                    "  {}:{}  {}{}\n      {}\n",
                    site.line,
                    site.col,
                    site.message,
                    site.code
                        .as_deref()
                        .map(|c| format!(" [{c}]"))
                        .unwrap_or_default(),
                    site.source
                ));
                if let Some(fix) = &site.suggestion {
                    out.push_str(&format!("      try: {fix}\n"));
                }
            }
            out.push_str(
                "\nthese are not failures, they are the migration: each site needs a decision \
                 about how the old value becomes the new one. Nothing is suggested that this \
                 cannot see plainly, and a conversion is written only when `convert` is set and \
                 the analyzer accepts it.\n",
            );
        }
        if self.in_attributes > 0 {
            out.push_str(&format!(
                "\n{} further diagnostic(s) landed on a `#[derive(…)]` line rather than on code. \
                 An error inside what a derive generates is reported at the derive, and there \
                 is nothing at those positions to edit; they are left out of the list above.\n",
                self.in_attributes
            ));
        }
        if self.applied && !self.converted.is_empty() {
            let rest = if self.sites.is_empty() {
                ""
            } else {
                "; the sites above were not"
            };
            out.push_str(&format!(
                "\n[the declaration and {} conversion(s) were written{rest}]\n",
                self.converted.len()
            ));
        } else if self.applied {
            out.push_str("\n[the declaration was written; the sites above were not]\n");
        } else if !self.converted.is_empty() {
            out.push_str(
                "\nnothing was written; pass `apply: true` to write the declaration and the \
                 conversions, and `force: true` while sites remain\n",
            );
        } else {
            out.push_str(
                "\nnothing was written; pass `apply: true` to write the declaration alone, and \
                 `force: true` while sites remain\n",
            );
        }
        out
    }
}

/// The span of the type in a declaration, given the offset of the declared name.
///
/// Four shapes cover what can be migrated: a field or a parameter or an annotated `let`, which
/// are `name: Type` and end at the first `,`, `)`, `;` or `=` that is not inside brackets; and
/// a function, whose type is what follows `->`.
pub fn declared_type_span(text: &str, name_offset: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = name_offset;
    while i < bytes.len() && (bytes[i] == b'_' || (bytes[i] as char).is_alphanumeric()) {
        i += 1;
    }
    let after_name = i;
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    match bytes.get(i) {
        // A function: its type is the return type, and a function without one has nothing to
        // migrate.
        Some(b'(') => {
            let close = matching(text, i)?;
            let arrow = text[close..].find("->")? + close;
            // Only the arrow of this signature, not one inside a later body.
            let body = text[close..].find('{').map(|b| b + close);
            if body.is_some_and(|b| b < arrow) {
                return None;
            }
            let start = arrow + 2;
            let start = start + text[start..].len() - text[start..].trim_start().len();
            let end = end_of_type(text, start, b"{")?;
            Some((start, end))
        }
        Some(b':') => {
            let start = i + 1;
            let start = start + text[start..].len() - text[start..].trim_start().len();
            let end = end_of_type(text, start, b",);=")?;
            Some((start, end))
        }
        _ => {
            let _ = after_name;
            None
        }
    }
}

/// The offset just past the `)` that closes the `(` at `open`.
fn matching(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    for (i, c) in bytes.iter().enumerate().skip(open) {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Where a type written at `start` ends: the first terminator at bracket depth zero.
fn end_of_type(text: &str, start: usize, terminators: &[u8]) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        let prev = if i > 0 { bytes[i - 1] } else { b' ' };
        let next = bytes.get(i + 1).copied().unwrap_or(b' ');
        // The terminator is read before the depth is touched: `{` ends a return type and also
        // opens a block, and deciding in the other order loses every function's type.
        if depth == 0 && terminators.contains(&c) && !(c == b'=' && (next == b'=' || next == b'>'))
        {
            return Some(text[..i].trim_end().len());
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            // `<` and `>` nest a type, but `->` and `=>` are not brackets.
            b'<' if prev != b'-' && prev != b'=' => depth += 1,
            b'>' if prev != b'-' && prev != b'=' && depth > 0 => depth -= 1,
            _ => {}
        }
        if depth == 0 && c == b'\n' && terminators.contains(&b',') {
            // A field or parameter written without its trailing comma still ends at its line.
            return Some(text[..i].trim_end().len());
        }
        i += 1;
    }
    None
}

/// What would make the old value fit the new type, when the error says so plainly.
///
/// Only the two shapes that are unambiguous: the new type meeting the old one, either way
/// round. Anything else gets no suggestion rather than a guess.
pub fn suggest(message: &str, was: &str, now: &str) -> Option<String> {
    let (expected, found) = parse_mismatch(message)?;
    let (expected, found) = (type_name(&expected), type_name(&found));
    let (was, now) = (type_name(was), type_name(now));
    if expected == now && found == was {
        return Some(format!(
            "the value here is still `{was}`; convert it to `{now}`"
        ));
    }
    if expected == was && found == now {
        return Some(format!(
            "this place still wants `{was}` and is now given `{now}`; migrate it too, or convert \
             back here"
        ));
    }
    None
}

/// A type as the analyzer names it. It names a type as it is in scope, not as the declaration
/// spells it — `std::time::Duration` comes back as `Duration`, at any depth — and it spells out
/// the default allocator: `Box<str>` comes back as `Box<str, Global>`.
pub fn type_name(ty: &str) -> String {
    let mut out = String::new();
    for c in ty.trim().replace(", Global>", ">").chars() {
        out.push(c);
        if out.ends_with("::") {
            out.truncate(out.len() - 2);
            while out.ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                out.pop();
            }
        }
    }
    out
}

/// `expected X, found Y` out of an analyzer message.
fn parse_mismatch(message: &str) -> Option<(String, String)> {
    let rest = message.split("expected ").nth(1)?;
    let (expected, rest) = rest.split_once(", found ")?;
    let found = rest
        .split(['\n', ' '])
        .next()
        .unwrap_or(rest)
        .trim_end_matches(['.', ',']);
    Some((expected.trim().to_string(), found.trim().to_string()))
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Changes the declared type of the symbol at `file:line:col` and reports what no longer fits.
#[allow(clippy::too_many_arguments)]
pub async fn migrate(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    to: &str,
    convert: bool,
    apply: bool,
    force: bool,
) -> Result<Migration> {
    anyhow::ensure!(!to.trim().is_empty(), "the new type is empty");
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset = crate::signature::offset_of(&text, line, col)
        .context("the declaration is not at the resolved position")?;
    let name: String = text[offset..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    anyhow::ensure!(
        !name.is_empty(),
        "there is no declared name at that position"
    );
    let (start, end) = declared_type_span(&text, offset).with_context(|| {
        format!(
            "`{name}` has no declared type this understands: a field, a parameter, an annotated \
             `let` or a function's return type"
        )
    })?;
    let was = text[start..end].to_string();
    anyhow::ensure!(
        was.trim() != to.trim(),
        "`{name}` is already declared as `{to}`"
    );

    let mut new_text = text.clone();
    new_text.replace_range(start..end, to);
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    rewritten.insert(file.to_path_buf(), new_text);

    // Everything that mentions the symbol is worth checking, not only the file it lives in.
    let mut also: Vec<PathBuf> = crate::signature::references(remote, root, file, line, col)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(path, _, _)| path)
        .filter(|path| path != file)
        .collect();
    also.sort();
    also.dedup();

    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &also).await?;

    let (mut sites, mut in_attributes) = collect_sites(root, &rewritten, &reports, &was, to);

    let mut converted = Vec::new();
    let mut conversion_note = None;
    if convert {
        let outcome = convert_sites(remote, root, &rewritten, &also, &sites, &was, to).await?;
        match outcome {
            Converted::Accepted {
                texts,
                conversions,
                reports,
                tried,
            } => {
                let (after, attrs) = collect_sites(root, &texts, &reports, &was, to);
                sites = after;
                in_attributes = attrs;
                rewritten = texts;
                converted = conversions;
                mark_tried(&mut sites, &tried);
            }
            Converted::Nothing { tried } => mark_tried(&mut sites, &tried),
            Converted::Dropped { note, tried } => {
                conversion_note = Some(note);
                mark_tried(&mut sites, &tried);
            }
        }
    }

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            sites.is_empty() || force,
            "{} site(s) do not fit the new type; nothing was written. Read them first, then \
             pass `force: true` to write the declaration{} and migrate the sites yourself",
            sites.len(),
            if converted.is_empty() {
                ""
            } else {
                " and the conversions"
            }
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(Migration {
        symbol: name,
        root: root.to_path_buf(),
        file: display(root, file),
        was,
        now: to.to_string(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        sites,
        in_attributes,
        converted,
        conversion_note,
        applied,
    })
}

/// The errors of `reports` as sites, collapsed and with the ones on attributes counted apart.
fn collect_sites(
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
    reports: &[crate::diagnostics::DiagnosticsReport],
    was: &str,
    to: &str,
) -> (Vec<Site>, usize) {
    let mut sites = Vec::new();
    for report in reports {
        let source_of = |line: u32| -> String {
            let path = root.join(&report.file);
            let text = rewritten
                .iter()
                .find(|(p, _)| **p == path || display(root, p) == report.file)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| std::fs::read_to_string(&path).unwrap_or_default());
            text.lines()
                .nth(line.saturating_sub(1) as usize)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        // A derive the analyzer cannot type is set aside by validation (#159); it still lands
        // on an attribute line here, and is counted there.
        for item in report.items.iter().chain(&report.in_derive) {
            if item.severity != "error" {
                continue;
            }
            let message = item.message.lines().next().unwrap_or("").to_string();
            sites.push(Site {
                file: report.file.clone(),
                line: item.line,
                col: item.col,
                suggestion: suggest(&message, was, to),
                message,
                code: item.code.clone(),
                source: source_of(item.line),
                end: item.end,
            });
        }
    }
    // The same position is reported once per expansion of the same derive, and every one of
    // them says the same thing about a line nobody can edit. Collapse the repeats, then set
    // the attribute ones aside so the list is the work and not the noise.
    sites.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.col.cmp(&b.col))
            .then(a.message.cmp(&b.message))
    });
    sites.dedup_by(|a, b| {
        a.file == b.file && a.line == b.line && a.col == b.col && a.message == b.message
    });
    let before = sites.len();
    sites.retain(|s| !s.source.trim_start().starts_with("#["));
    let in_attributes = before - sites.len();
    (sites, in_attributes)
}

/// Whether `.into()` can go on this site's expression: an E0308 where the old and the new type
/// meet, either way round, with the whole expression on one line.
fn is_candidate(site: &Site, was: &str, now: &str) -> bool {
    if site.code.as_deref() != Some("E0308") {
        return false;
    }
    let Some((expected, found)) = parse_mismatch(&site.message) else {
        return false;
    };
    let (expected, found) = (type_name(&expected), type_name(&found));
    let (was, now) = (type_name(was), type_name(now));
    let meet = (expected == now && found == was) || (expected == was && found == now);
    meet && site.end.is_some_and(|(line, _)| line == site.line)
}

/// `expr.into()`, with parentheses unless the expression is a path, a call chain or a literal
/// that a method call binds to as a whole.
pub fn into_call(expr: &str) -> String {
    let mut depth = 0i32;
    let mut simple = !expr.is_empty();
    for c in expr.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if depth > 0 => {}
            c if c.is_alphanumeric() || c == '_' || c == '.' || c == ':' => {}
            _ => simple = false,
        }
    }
    let starts_well = expr
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_');
    if simple && starts_well {
        format!("{expr}.into()")
    } else {
        format!("({expr}).into()")
    }
}

/// A site's expression in its file's text, as a byte range.
///
/// For a method call the analyzer's range is the method's name alone — `to_string` in
/// `label.to_string()` — so a range followed by an argument list is extended over it. A range
/// that starts after a `.` is the last link of a chain, and `.into()` can go after it only as it
/// is: parentheses would cut the chain in two, so such a site is not converted at all.
pub fn expression_span(text: &str, site: &Site) -> Option<(usize, usize)> {
    let (end_line, end_col) = site.end?;
    let start = crate::signature::offset_of(text, site.line, site.col)?;
    let mut end = crate::signature::offset_of(text, end_line, end_col)?;
    if text[end..].starts_with('(') {
        let mut depth = 0i32;
        for (i, c) in text[end..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end += i + 1;
                        break;
                    }
                }
                '\n' => return None,
                _ => {}
            }
        }
    }
    let expr = text.get(start..end)?;
    if start >= end || expr.contains('\n') || expr.trim().is_empty() {
        return None;
    }
    if text[..start].ends_with('.') && into_call(expr).starts_with('(') {
        return None;
    }
    Some((start, end))
}

enum Converted {
    /// Some conversions type-check, and nothing else broke.
    Accepted {
        texts: BTreeMap<PathBuf, String>,
        conversions: Vec<Conversion>,
        reports: Vec<crate::diagnostics::DiagnosticsReport>,
        tried: BTreeSet<(String, u32, u32)>,
    },
    /// No site could take a conversion.
    Nothing { tried: BTreeSet<(String, u32, u32)> },
    /// The conversions that type-check where they are caused an error somewhere else.
    Dropped {
        note: String,
        tried: BTreeSet<(String, u32, u32)>,
    },
}

/// Writes `.into()` at every candidate site, checks the overlay, takes back each conversion
/// whose line still has an error, and checks again — until what is left type-checks.
async fn convert_sites(
    remote: SocketAddr,
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
    also: &[PathBuf],
    sites: &[Site],
    was: &str,
    now: &str,
) -> Result<Converted> {
    let known: BTreeSet<(String, u32, String)> = sites
        .iter()
        .map(|s| (s.file.clone(), s.line, s.message.clone()))
        .collect();
    let mut candidates: Vec<&Site> = sites.iter().filter(|s| is_candidate(s, was, now)).collect();
    let mut tried: BTreeSet<(String, u32, u32)> = BTreeSet::new();
    for _round in 0..4 {
        if candidates.is_empty() {
            return Ok(Converted::Nothing { tried });
        }
        let mut texts = rewritten.clone();
        let mut spans: BTreeMap<PathBuf, Vec<(usize, usize, &Site)>> = BTreeMap::new();
        for site in &candidates {
            let path = root.join(&site.file);
            let key = texts
                .keys()
                .find(|p| **p == path || display(root, p) == site.file)
                .cloned()
                .unwrap_or(path);
            let text = match texts.get(&key) {
                Some(t) => t.clone(),
                None => std::fs::read_to_string(&key)
                    .with_context(|| format!("cannot read {}", key.display()))?,
            };
            texts.entry(key.clone()).or_insert(text.clone());
            if let Some((start, end)) = expression_span(&text, site) {
                spans.entry(key).or_default().push((start, end, site));
            }
        }
        let mut conversions = Vec::new();
        for (path, mut list) in spans {
            list.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
            let text = texts.get_mut(&path).expect("read above");
            for (start, end, site) in list {
                let expr = text[start..end].to_string();
                let call = into_call(&expr);
                text.replace_range(start..end, &call);
                conversions.push(Conversion {
                    file: site.file.clone(),
                    line: site.line,
                    was: expr,
                    now: call,
                });
            }
        }
        conversions.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        let edits: Vec<(PathBuf, String)> =
            texts.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
        let others: Vec<PathBuf> = also
            .iter()
            .filter(|p| !texts.contains_key(*p))
            .cloned()
            .collect();
        let reports = crate::diagnostics::validate_texts(remote, root, &edits, &others).await?;
        let failing: BTreeSet<(String, u32)> = reports
            .iter()
            .flat_map(|r| {
                r.items
                    .iter()
                    .filter(|d| d.severity == "error")
                    .map(move |d| (r.file.clone(), d.line))
            })
            .collect();
        let before = candidates.len();
        candidates.retain(|s| {
            let bad = failing.contains(&(s.file.clone(), s.line));
            if bad {
                tried.insert((s.file.clone(), s.line, s.col));
            }
            !bad
        });
        if candidates.len() < before {
            continue;
        }
        // Every conversion left type-checks where it is. Anything new elsewhere is theirs.
        let converted_lines: BTreeSet<(String, u32)> = conversions
            .iter()
            .map(|c| (c.file.clone(), c.line))
            .collect();
        let new: Vec<String> = reports
            .iter()
            .flat_map(|r| {
                r.items
                    .iter()
                    .filter(|d| d.severity == "error")
                    .map(move |d| (r.file.clone(), d.line, d.message.clone()))
            })
            .filter(|(f, l, m)| {
                let first = m.lines().next().unwrap_or("").to_string();
                !known.contains(&(f.clone(), *l, first))
                    && !converted_lines.contains(&(f.clone(), *l))
            })
            .map(|(f, l, m)| format!("{f}:{l} {}", m.lines().next().unwrap_or("")))
            .collect();
        if !new.is_empty() {
            return Ok(Converted::Dropped {
                note: format!(
                    "{} conversion(s) type-check where they are but cause an error elsewhere, so \
                     none was kept:\n  {}",
                    conversions.len(),
                    new.join("\n  ")
                ),
                tried,
            });
        }
        return Ok(Converted::Accepted {
            texts,
            conversions,
            reports,
            tried,
        });
    }
    Ok(Converted::Dropped {
        note: "the conversions did not settle in four rounds of checking, so none was kept"
            .to_string(),
        tried,
    })
}

/// A site where `.into()` was tried and rejected says so, instead of suggesting it.
fn mark_tried(sites: &mut [Site], tried: &BTreeSet<(String, u32, u32)>) {
    for site in sites {
        if site.code.as_deref() == Some("E0308")
            && tried.contains(&(site.file.clone(), site.line, site.col))
        {
            site.suggestion = Some(
                "`.into()` was tried here and does not type-check: there is no conversion the \
                 analyzer accepts, so this one is a decision (a narrowing, a fallible \
                 conversion, or a place that should be migrated too)"
                    .to_string(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span_of(text: &str, name: &str) -> String {
        let at = text.find(name).expect("the name is in the text");
        let (s, e) = declared_type_span(text, at).expect("a declared type");
        text[s..e].to_string()
    }

    #[test]
    fn a_declared_type_is_found_in_every_shape_that_can_have_one() {
        assert_eq!(
            span_of("    pub timeout_secs: u64,\n", "timeout_secs"),
            "u64"
        );
        assert_eq!(
            span_of("    pub map: BTreeMap<String, Vec<u8>>,\n", "map"),
            "BTreeMap<String, Vec<u8>>"
        );
        assert_eq!(span_of("fn f(a: &str, b: u32) {}", "b"), "u32");
        assert_eq!(
            span_of("fn compute(a: &str) -> Result<u32, Error> {\n", "compute"),
            "Result<u32, Error>"
        );
        assert_eq!(span_of("    let x: Vec<u8> = go();\n", "x"), "Vec<u8>");
        // The last field of a struct, written without a trailing comma.
        assert_eq!(span_of("    pub n: usize\n}\n", "n"), "usize");
    }

    #[test]
    fn a_function_without_a_return_type_has_nothing_to_migrate() {
        let text = "fn compute(a: u8) {\n    a;\n}\n";
        let at = text.find("compute").expect("the name");
        assert_eq!(declared_type_span(text, at), None);
    }

    #[test]
    fn a_suggestion_is_made_only_when_the_error_names_both_types() {
        assert_eq!(
            suggest("expected u64, found u32", "u32", "u64").as_deref(),
            Some("the value here is still `u32`; convert it to `u64`")
        );
        assert!(
            suggest("expected u32, found u64", "u32", "u64")
                .is_some_and(|s| s.contains("still wants"))
        );
        // Neither type is ours: no guess.
        assert_eq!(suggest("expected String, found &str", "u32", "u64"), None);
        // The declaration spells a path; the analyzer spells the name that is in scope.
        assert_eq!(
            suggest("expected Duration, found u64", "u64", "std::time::Duration").as_deref(),
            Some("the value here is still `u64`; convert it to `Duration`")
        );
        assert_eq!(
            suggest("cannot find value `n` in this scope", "u32", "u64"),
            None
        );
    }

    #[test]
    fn the_report_calls_the_sites_work_rather_than_failure() {
        let migration = Migration {
            symbol: "timeout_secs".into(),
            root: PathBuf::from("/root"),
            file: "src/lib.rs".into(),
            was: "u64".into(),
            now: "std::time::Duration".into(),
            rewritten: Vec::new(),
            sites: vec![Site {
                file: "src/other.rs".into(),
                line: 12,
                col: 5,
                message: "expected Duration, found u64".into(),
                code: Some("E0308".into()),
                source: "cfg.timeout_secs = 30;".into(),
                suggestion: Some("the value here is still `u64`".into()),
                end: None,
            }],
            in_attributes: 0,
            converted: Vec::new(),
            conversion_note: None,
            applied: false,
        };
        let text = migration.render(50);
        assert!(text.contains("1 site(s) in 1 file(s)"), "{text}");
        assert!(text.contains("src/other.rs"), "{text}");
        assert!(text.contains("cfg.timeout_secs = 30;"), "{text}");
        assert!(
            text.contains("try: the value here is still `u64`"),
            "{text}"
        );
        assert!(text.contains("they are the migration"), "{text}");
        assert!(text.contains("nothing was written"), "{text}");

        let clean = Migration {
            sites: Vec::new(),
            applied: true,
            ..migration
        };
        let text = clean.render(50);
        assert!(text.contains("nothing else has to change"), "{text}");
        assert!(text.contains("the declaration was written"), "{text}");
    }

    #[test]
    fn a_long_list_is_cut_at_the_budget_and_says_how_many_are_left() {
        let site = |line: u32| Site {
            file: "src/lib.rs".into(),
            line,
            col: 1,
            message: "expected A, found B".into(),
            code: None,
            source: "x".into(),
            suggestion: None,
            end: None,
        };
        let migration = Migration {
            symbol: "f".into(),
            root: PathBuf::from("/root"),
            file: "src/lib.rs".into(),
            was: "A".into(),
            now: "B".into(),
            rewritten: Vec::new(),
            sites: (1..=10).map(site).collect(),
            in_attributes: 0,
            converted: Vec::new(),
            conversion_note: None,
            applied: false,
        };
        let text = migration.render(3);
        assert!(text.contains("… 7 more site(s)"), "{text}");
    }

    #[test]
    fn into_goes_on_a_path_or_call_and_around_anything_else() {
        assert_eq!(into_call("secs"), "secs.into()");
        assert_eq!(into_call("l.timeout"), "l.timeout.into()");
        assert_eq!(
            into_call("build(1, \"x\").timeout"),
            "build(1, \"x\").timeout.into()"
        );
        assert_eq!(into_call("secs as u32"), "(secs as u32).into()");
        assert_eq!(into_call("l.timeout * 2"), "(l.timeout * 2).into()");
        assert_eq!(into_call("&name"), "(&name).into()");
        assert_eq!(into_call("-1"), "(-1).into()");
    }

    #[test]
    fn a_candidate_is_the_two_types_meeting_on_one_line() {
        let site = |message: &str, code: &str, end: Option<(u32, u32)>| Site {
            file: "src/lib.rs".into(),
            line: 4,
            col: 5,
            message: message.into(),
            code: Some(code.into()),
            source: String::new(),
            suggestion: None,
            end,
        };
        let one_line = Some((4, 9));
        assert!(is_candidate(
            &site("expected u64, found u32", "E0308", one_line),
            "u32",
            "u64"
        ));
        assert!(is_candidate(
            &site("expected u32, found u64", "E0308", one_line),
            "u32",
            "u64"
        ));
        assert!(!is_candidate(
            &site("expected u64, found u16", "E0308", one_line),
            "u32",
            "u64"
        ));
        assert!(!is_candidate(
            &site("expected u64, found u32", "E0277", one_line),
            "u32",
            "u64"
        ));
        assert!(!is_candidate(
            &site("expected u64, found u32", "E0308", Some((6, 2))),
            "u32",
            "u64"
        ));
        assert!(!is_candidate(
            &site("expected u64, found u32", "E0308", None),
            "u32",
            "u64"
        ));
    }

    #[test]
    fn a_converted_migration_lists_what_it_wrote() {
        let migration = Migration {
            symbol: "timeout".into(),
            root: PathBuf::from("/root"),
            file: "src/lib.rs".into(),
            was: "u32".into(),
            now: "u64".into(),
            rewritten: Vec::new(),
            sites: Vec::new(),
            in_attributes: 0,
            converted: vec![Conversion {
                file: "src/lib.rs".into(),
                line: 9,
                was: "secs".into(),
                now: "secs.into()".into(),
            }],
            conversion_note: Some("a note".into()),
            applied: true,
        };
        let text = migration.render(10);
        assert!(
            text.contains("1 site(s) converted with `.into()`"),
            "{text}"
        );
        assert!(
            text.contains("src/lib.rs:9  `secs` → `secs.into()`"),
            "{text}"
        );
        assert!(text.contains("a note"), "{text}");
        assert!(
            text.contains("the declaration and 1 conversion(s) were written"),
            "{text}"
        );
    }

    #[test]
    fn a_type_is_named_the_way_the_analyzer_names_it() {
        assert_eq!(type_name("std::time::Duration"), "Duration");
        assert_eq!(type_name("Vec<std::string::String>"), "Vec<String>");
        assert_eq!(type_name("Box<str, Global>"), "Box<str>");
        assert_eq!(type_name("std::boxed::Box<str>"), "Box<str>");
        assert_eq!(
            type_name("HashMap<u32, Vec<u8, Global>>"),
            "HashMap<u32, Vec<u8>>"
        );
    }

    #[test]
    fn a_method_name_range_takes_its_arguments_and_a_chain_is_not_cut() {
        let site = |line: u32, col: u32, end_col: u32| Site {
            file: "src/lib.rs".into(),
            line,
            col,
            message: String::new(),
            code: None,
            source: String::new(),
            suggestion: None,
            end: Some((line, end_col)),
        };
        let text = "    name: label.to_string(),\n    x: a.b + c,\n";
        let (s, e) = expression_span(text, &site(1, 17, 26)).expect("a span");
        assert_eq!(&text[s..e], "to_string()");
        assert!(expression_span(text, &site(2, 10, 15)).is_none());
        let (s, e) = expression_span(text, &site(2, 8, 11)).expect("a span");
        assert_eq!(&text[s..e], "a.b");
    }
}
