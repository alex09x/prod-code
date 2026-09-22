//! Changing a declared type, and reporting the whole shape of what that breaks.
//!
//! This is the first half of a type migration and says so: it moves the declaration and then
//! tells the truth about the size of the job, site by site, before any of it is done. Where a
//! site's error is exactly the old type meeting the new one it says what conversion would fix
//! it — says it, rather than writing it, because a wrong `.into()` inserted at forty call sites
//! is the kind of plausible damage the rest of these tools exist to avoid.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
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
    /// How many diagnostics landed on an attribute rather than on code: the analyzer reports
    /// inside a derive it did not expand, and none of them is a place anyone can edit.
    pub in_attributes: usize,
    pub applied: bool,
}

impl Migration {
    /// The report: the declaration, then the work the change creates.
    pub fn render(&self, budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: `{}`\n- now: `{}`\n",
            self.symbol, self.file, self.was, self.now
        );
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
                 cannot see plainly, and nothing suggested is written.\n",
            );
        }
        if self.in_attributes > 0 {
            out.push_str(&format!(
                "\n{} further diagnostic(s) landed on a `#[derive(…)]` line rather than on code. \
                 The analyzer reports inside a derive it cannot expand here, and there is nothing \
                 at those positions to edit; they are left out of the list above.\n",
                self.in_attributes
            ));
        }
        if self.applied {
            out.push_str("\n[the declaration was written; the sites above were not]\n");
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
    /// The analyzer names a type as it is in scope, not as the declaration spells it:
    /// `std::time::Duration` comes back as `Duration`.
    fn short(ty: &str) -> &str {
        ty.rsplit("::").next().unwrap_or(ty).trim()
    }
    let (expected, found) = parse_mismatch(message)?;
    let (expected, found) = (short(&expected).to_string(), short(&found).to_string());
    let (was, now) = (short(was), short(now));
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

    let mut sites = Vec::new();
    for report in &reports {
        let source_of = |line: u32| -> String {
            let path = root.join(&report.file);
            let text = if path == *file {
                rewritten.get(file).cloned().unwrap_or_default()
            } else {
                std::fs::read_to_string(&path).unwrap_or_default()
            };
            text.lines()
                .nth(line.saturating_sub(1) as usize)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        for item in &report.items {
            if item.severity != "error" {
                continue;
            }
            let message = item.message.lines().next().unwrap_or("").to_string();
            sites.push(Site {
                file: report.file.clone(),
                line: item.line,
                col: item.col,
                suggestion: suggest(&message, &was, to),
                message,
                code: item.code.clone(),
                source: source_of(item.line),
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

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            sites.is_empty() || force,
            "{} site(s) do not fit the new type; nothing was written. Read them first, then \
             pass `force: true` to write the declaration and migrate the sites yourself",
            sites.len()
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
        applied,
    })
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
            }],
            in_attributes: 0,
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
            applied: false,
        };
        let text = migration.render(3);
        assert!(text.contains("… 7 more site(s)"), "{text}");
    }
}
