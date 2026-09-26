//! Blast radius of a change (roadmap 8.1): the functions a diff touches, the callers that
//! reach them through the analyzer's call hierarchy, and the tests among those callers,
//! with the command that runs only the affected tests.
//!
//! What the analysis could not establish (a deleted file, a change whose lines cannot be placed,
//! a request that failed, an answer it cannot read, a walk cut short by the depth limit) is
//! reported as a gap, never read as "no callers": a selection with a gap is not trusted, and
//! `impact --ci` runs the whole suite (#434).

use crate::session::LspSession;
use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use url::Url;

/// A function/method the change touches or reaches.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Symbol {
    pub name: String,
    /// Checkout-relative path.
    pub file: String,
    /// 1-based line of the name.
    pub line: u32,
    /// 1-based column of the name.
    pub col: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactReport {
    pub language: String,
    pub base: String,
    pub changed_files: Vec<String>,
    /// Functions whose bodies the diff touches.
    pub changed: Vec<Symbol>,
    /// Callers reached from the changed functions (transitively), tests excluded.
    pub callers: Vec<Symbol>,
    /// Test functions reached.
    pub tests: Vec<Symbol>,
    /// Command that runs only the affected tests, when the language supports selection.
    pub test_command: Option<Vec<String>>,
    /// Files whose changed lines lie outside any function (module-level code, manifests).
    pub unattributed_files: Vec<String>,
    /// How the analyzer's index was brought up to date first, when it had to be (Swift), and
    /// whether that worked: when it did not, no callers means unknown callers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexBuild>,
    /// Which changed function reaches which test, in how many calls: the suspects of a failure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reaches: Vec<Reach>,
    /// What the analysis could not establish: a test beyond those listed may reach the change,
    /// so only the whole suite can be trusted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incomplete: Vec<Gap>,
}

/// Something the analysis could not establish, so a test that reaches the change may be
/// missing from the selection.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Gap {
    /// A changed file is gone: what called into it can no longer be asked.
    Deleted { file: String },
    /// A changed source file whose change cannot be placed in its lines: a binary file, or a
    /// path or a hunk the diff could not be read for.
    Diff { file: String, error: String },
    /// The functions of a changed source file could not be listed.
    Symbols { file: String, error: String },
    /// The callers of a function could not be asked, or the answer could not be read.
    Callers { symbol: Symbol, error: String },
    /// The walk stopped at the depth limit while a function still had callers.
    Depth { symbol: Symbol, depth: usize },
}

impl Gap {
    pub fn describe(&self) -> String {
        match self {
            Gap::Deleted { file } => {
                format!("{file} was deleted, so what called into it is unknown")
            }
            Gap::Diff { file, error } => {
                format!("the lines the change to {file} touches are unknown: {error}")
            }
            Gap::Symbols { file, error } => {
                format!("the functions of {file} could not be listed: {error}")
            }
            Gap::Callers { symbol, error } => format!(
                "the callers of {} ({}:{}) are unknown: {error}",
                symbol.name, symbol.file, symbol.line
            ),
            Gap::Depth { symbol, depth } => format!(
                "the walk stopped at depth {depth} while {} ({}:{}) still had callers",
                symbol.name, symbol.file, symbol.line
            ),
        }
    }
}

/// What `impact --ci` runs for a report, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiDecision {
    pub run: CiRun,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiRun {
    /// The language's whole test suite.
    WholeSuite,
    /// This command, which runs only the tests that reach the change.
    Selected(Vec<String>),
    /// Nothing: no function changed, or no test reaches one.
    Nothing,
}

/// A test the walk from a changed function reached, and in how many calls.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct Reach {
    pub test: Symbol,
    pub changed: Symbol,
    pub hops: usize,
}

/// The build that gives sourcekit-lsp its index: Swift 5 finds callers only through it (#166).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct IndexBuild {
    pub command: String,
    pub ok: bool,
    pub duration_ms: u64,
}

impl ImpactReport {
    /// Why the selection cannot be trusted and the whole suite should run instead: an index
    /// that could not be built, a gap in the analysis, or lines changed outside any function.
    /// `None` when it can.
    pub fn full_suite_reason(&self) -> Option<String> {
        if self.index.as_ref().is_some_and(|b| !b.ok) {
            return Some("the analyzer's index could not be built, so callers are unknown".into());
        }
        if let Some(first) = self.incomplete.first() {
            let more = self.incomplete.len() - 1;
            return Some(if more == 0 {
                format!("the analysis is incomplete: {}", first.describe())
            } else {
                format!(
                    "the analysis is incomplete: {} (and {more} more)",
                    first.describe()
                )
            });
        }
        if !self.unattributed_files.is_empty() {
            return Some(format!(
                "lines changed outside any function in {}",
                self.unattributed_files.join(", ")
            ));
        }
        None
    }

    /// What `impact --ci` runs: the selection only when it can be trusted, the whole suite when
    /// it cannot or the language cannot select tests, nothing when no test can be affected
    /// (#201, #434).
    pub fn ci_decision(&self) -> CiDecision {
        let (run, why) = match (self.full_suite_reason(), &self.test_command) {
            (Some(reason), _) => (
                CiRun::WholeSuite,
                format!("the whole suite, because {reason}"),
            ),
            (None, Some(selected)) => (
                CiRun::Selected(selected.clone()),
                format!("{} test(s) that reach the change", self.tests.len()),
            ),
            (None, None) if self.changed.is_empty() => {
                (CiRun::Nothing, "no function changed".to_string())
            }
            (None, None) if self.tests.is_empty() => (
                CiRun::Nothing,
                "no test reaches the changed functions".to_string(),
            ),
            (None, None) => (
                CiRun::WholeSuite,
                "the whole suite, because this language's tests cannot be selected".to_string(),
            ),
        };
        CiDecision { run, why }
    }

    /// The Markdown a CI job shows for this analysis and the command it ran (`why` says why
    /// that command), for `$GITHUB_STEP_SUMMARY`.
    pub fn ci_summary(&self, command: Option<&[String]>, why: &str) -> String {
        let mut out = format!(
            "### prod-code impact of `{}`\n\n{} changed file(s), {} changed function(s), {} test(s) reached.\n\n",
            self.base,
            self.changed_files.len(),
            self.changed.len(),
            self.tests.len()
        );
        if !self.changed.is_empty() {
            out.push_str("| changed function | file |\n|---|---|\n");
            for s in &self.changed {
                out.push_str(&format!("| `{}` | `{}:{}` |\n", s.name, s.file, s.line));
            }
            out.push('\n');
        }
        if !self.tests.is_empty() {
            out.push_str("Tests that reach them:\n\n");
            for s in &self.tests {
                out.push_str(&format!("- `{}` (`{}:{}`)\n", s.name, s.file, s.line));
            }
            out.push('\n');
        }
        if !self.incomplete.is_empty() {
            out.push_str("The analysis is incomplete, so the selection cannot be trusted:\n\n");
            for gap in &self.incomplete {
                out.push_str(&format!("- {}\n", gap.describe()));
            }
            out.push('\n');
        }
        match command {
            Some(c) => out.push_str(&format!("Ran `{}`: {why}.\n", c.join(" "))),
            None => out.push_str(&format!("Ran nothing: {why}.\n")),
        }
        out
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let unindexed = self.index.as_ref().is_some_and(|b| !b.ok);
        let reached = if unindexed {
            "callers and tests unknown".to_string()
        } else if !self.incomplete.is_empty() {
            format!(
                "{} caller(s), {} test(s), incomplete",
                self.callers.len(),
                self.tests.len()
            )
        } else {
            format!(
                "{} caller(s), {} test(s)",
                self.callers.len(),
                self.tests.len()
            )
        };
        out.push_str(&format!(
            "impact of {} ({} changed file(s), {} changed function(s), {reached})\n",
            self.base,
            self.changed_files.len(),
            self.changed.len(),
        ));
        if let Some(b) = &self.index {
            out.push_str(&if b.ok {
                format!(
                    "index: `{}` ({:.1}s) before the call hierarchy\n",
                    b.command,
                    b.duration_ms as f64 / 1000.0
                )
            } else {
                format!(
                    "index: `{}` failed, so the analyzer has no index; callers and tests below are unknown, not none\n",
                    b.command
                )
            });
        }
        if !self.changed.is_empty() {
            out.push_str("changed functions:\n");
            for s in &self.changed {
                out.push_str(&format!(
                    "  • {}  {}:{}:{}\n",
                    s.name, s.file, s.line, s.col
                ));
            }
        }
        if !self.callers.is_empty() {
            out.push_str("reached callers:\n");
            for s in &self.callers {
                out.push_str(&format!(
                    "  • {}  {}:{}:{}\n",
                    s.name, s.file, s.line, s.col
                ));
            }
        }
        if !self.tests.is_empty() {
            out.push_str("affected tests:\n");
            for s in &self.tests {
                out.push_str(&format!(
                    "  • {}  {}:{}:{}\n",
                    s.name, s.file, s.line, s.col
                ));
            }
        } else if unindexed {
            out.push_str("affected tests: unknown (no index); run the full suite\n");
        } else if !self.incomplete.is_empty() {
            out.push_str(
                "affected tests: unknown (the analysis is incomplete); run the full suite\n",
            );
        } else if !self.changed.is_empty() {
            out.push_str("affected tests: none reach the changed functions\n");
        }
        if !self.incomplete.is_empty() {
            out.push_str(
                "incomplete analysis (tests beyond those listed may be affected; run the full suite):\n",
            );
            for gap in &self.incomplete {
                out.push_str(&format!("  • {}\n", gap.describe()));
            }
        }
        if !self.unattributed_files.is_empty() {
            out.push_str(&format!(
                "changes outside functions (run the full suite for these): {}\n",
                self.unattributed_files.join(", ")
            ));
        }
        if let Some(cmd) = &self.test_command {
            out.push_str(&format!("run: {}\n", shell_words(cmd)));
        }
        out
    }
}

fn shell_words(cmd: &[String]) -> String {
    cmd.iter()
        .map(|w| {
            if w.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./=:,".contains(c))
            {
                w.clone()
            } else {
                format!("'{}'", w.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Changed line ranges per file: the working tree against `base` (HEAD by default), plus
/// untracked files as fully changed. A deleted file, or one whose mode alone changed, is listed
/// with no ranges, since none of its lines changed or are left; a change whose lines cannot be
/// placed (a binary file) is listed as the whole file.
pub fn changed_lines(root: &Path, base: Option<&str>) -> Result<BTreeMap<String, Vec<(u32, u32)>>> {
    Ok(diff_hunks(root, base)?
        .into_iter()
        .map(|(file, change)| {
            let ranges = match change {
                Change::Hunks(hunks) => hunks
                    .iter()
                    .map(|h| {
                        // A pure removal still touches the line it follows.
                        let end = h.start.saturating_add(h.added.max(1) - 1);
                        (h.start.max(1), end.max(1))
                    })
                    .collect(),
                Change::Unknown(_) => vec![(1, u32::MAX)],
            };
            (file, ranges)
        })
        .collect())
}

/// What the diff says about one changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Change {
    /// Every hunk of it, placed in the new text; none when the file was deleted or git counts
    /// no changed line (its mode alone changed).
    Hunks(Vec<Hunk>),
    /// A change whose lines cannot be placed: a binary file, or a path or a hunk that cannot be
    /// read. What it touches is unknown, never nothing.
    Unknown(String),
}

/// One hunk of the diff, placed in the new text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hunk {
    /// 1-based first line it adds or rewrites; for a pure removal, the line the removed lines
    /// followed (0 at the top of the file).
    start: u32,
    /// Lines it adds or rewrites; 0 for a pure removal.
    added: u32,
    /// Lines it removes or rewrites; 0 for a pure addition.
    removed: u32,
}

impl Hunk {
    /// Whether it changes the function spanning lines `from..=to`.
    fn touches(&self, from: u32, to: u32) -> bool {
        if self.added == 0 {
            // Removed between `start` and the next line: inside only when both are.
            return from <= self.start && self.start < to;
        }
        let end = self.start.saturating_add(self.added - 1);
        self.start <= to && end >= from
    }

    /// Whether everything it changes lies inside the functions spanning `spans`, given the new
    /// text's `lines`. A pure removal must sit between two lines of one function: removed
    /// between two functions, it took something at module level with it. Of the lines a pure
    /// addition adds, a blank one changes nothing.
    fn inside(&self, spans: &[(u32, u32)], lines: &[&str]) -> bool {
        if self.added == 0 {
            return spans.iter().any(|&(from, to)| self.touches(from, to));
        }
        let last = self
            .start
            .saturating_add(self.added - 1)
            .min(lines.len() as u32);
        (self.start..=last).all(|n| {
            let blank = (n as usize)
                .checked_sub(1)
                .and_then(|i| lines.get(i))
                .is_some_and(|l| l.trim().is_empty());
            (self.removed == 0 && blank) || spans.iter().any(|&(from, to)| from <= n && n <= to)
        })
    }
}

/// Runs git in `root` and returns what it printed.
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| format!("git {} failed to start", args.join(" ")))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// A path git printed verbatim, and why its change cannot be placed when it is not UTF-8 (no
/// file can be opened by the name it would be reported under).
fn path_text(bytes: &[u8]) -> (String, Option<String>) {
    match std::str::from_utf8(bytes) {
        Ok(path) => (path.to_string(), None),
        Err(_) => (
            String::from_utf8_lossy(bytes).into_owned(),
            Some("its path is not UTF-8".to_string()),
        ),
    }
}

/// A path as a diff header prints it: bare (a name with a space is followed by a tab that is
/// not part of it), or in double quotes with C escapes (`\t`, `\"`, `\\`, `\303\274` for the
/// bytes of `ü`) when it holds anything else. `None` when the quoting cannot be read.
fn git_path(field: &[u8]) -> Option<Vec<u8>> {
    let Some(quoted) = field.strip_prefix(b"\"") else {
        let end = field.iter().rposition(|&b| b != b'\t').map_or(0, |i| i + 1);
        return Some(field[..end].to_vec());
    };
    let mut out = Vec::new();
    let mut bytes = quoted.iter().copied();
    while let Some(b) = bytes.next() {
        match b {
            b'"' => return Some(out),
            b'\\' => {
                let escaped = bytes.next()?;
                out.push(match escaped {
                    b'a' => 0x07,
                    b'b' => 0x08,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'v' => 0x0b,
                    b'f' => 0x0c,
                    b'r' => b'\r',
                    b'"' | b'\\' => escaped,
                    b'0'..=b'3' => {
                        let (second, third) = (bytes.next()?, bytes.next()?);
                        if !second.is_ascii_digit() || second > b'7' {
                            return None;
                        }
                        if !third.is_ascii_digit() || third > b'7' {
                            return None;
                        }
                        (escaped - b'0') * 64 + (second - b'0') * 8 + (third - b'0')
                    }
                    _ => return None,
                });
            }
            _ => out.push(b),
        }
    }
    None
}

/// The path a `--- ` or `+++ ` line names after its `prefix`: `Some(None)` for `/dev/null`,
/// `None` when it cannot be read.
fn header_path(field: &[u8], prefix: &[u8]) -> Option<Option<String>> {
    let path = git_path(field)?;
    if path == b"/dev/null" {
        return Some(None);
    }
    let path = path.strip_prefix(prefix)?;
    Some(Some(String::from_utf8_lossy(path).into_owned()))
}

/// The hunk a `@@ -a,b +c,d @@` header describes, given what follows its first `@@ `; `None`
/// when it is not one.
fn hunk_header(rest: &[u8]) -> Option<Hunk> {
    let mut words = rest.split(|&b| b == b' ');
    let (old, new, end) = (words.next()?, words.next()?, words.next()?);
    if end != b"@@" {
        return None;
    }
    let span = |word: &[u8], sign: u8| -> Option<(u32, u32)> {
        let spec = std::str::from_utf8(word.strip_prefix(&[sign])?).ok()?;
        let digits = |s: &str| {
            (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse::<u32>().ok())
                .flatten()
        };
        Some(match spec.split_once(',') {
            Some((start, count)) => (digits(start)?, digits(count)?),
            None => (digits(spec)?, 1),
        })
    };
    let ((_, removed), (start, added)) = (span(old, b'-')?, span(new, b'+')?);
    Some(Hunk {
        start,
        added,
        removed,
    })
}

/// The change of every file in the working tree against `base` (HEAD by default), untracked
/// files as one hunk that adds everything. Every file the diff names is listed: a deleted one
/// or one whose mode alone changed with no hunks, and one whose lines cannot be placed (a binary
/// file, a path or hunk that cannot be read) as [`Change::Unknown`], never as no change.
fn diff_hunks(root: &Path, base: Option<&str>) -> Result<BTreeMap<String, Change>> {
    let base = base.unwrap_or("HEAD");
    // Without rename detection a moved file is its old path deleted and its new path added:
    // what called into the old path must be accounted for. External diff drivers, text
    // conversion and the configured prefixes would change what is printed.
    let common = [
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--no-relative",
    ];
    // Every changed path with the lines git counts for it, NUL-terminated and so never quoted:
    // the list the hunks below must account for in full.
    let numstat = git(
        root,
        &[&["diff", "--numstat", "-z"][..], &common, &[base, "--"]].concat(),
    )?;
    let mut changes: BTreeMap<String, Change> = BTreeMap::new();
    let mut counted: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for record in numstat.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let mut fields = record.splitn(3, |&b| b == b'\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            anyhow::bail!(
                "git diff --numstat printed a record it cannot read: {}",
                String::from_utf8_lossy(record)
            );
        };
        if path.is_empty() {
            anyhow::bail!("git diff --numstat reported a rename despite --no-renames");
        }
        let (path, unreadable_path) = path_text(path);
        let count = |field: &[u8]| std::str::from_utf8(field).ok()?.parse::<u64>().ok();
        let why = match (count(added), count(removed)) {
            _ if unreadable_path.is_some() => unreadable_path,
            (Some(added), Some(removed)) => {
                let total = counted.entry(path.clone()).or_default();
                total.0 += added;
                total.1 += removed;
                None
            }
            _ if added == b"-" && removed == b"-" => Some("it is a binary file".to_string()),
            _ => Some(format!(
                "git diff --numstat counts it as {}",
                String::from_utf8_lossy(record)
            )),
        };
        match why {
            Some(why) => {
                changes.insert(path, Change::Unknown(why));
            }
            None => {
                changes
                    .entry(path)
                    .or_insert_with(|| Change::Hunks(Vec::new()));
            }
        }
    }

    let diff = git(
        root,
        &[
            &[
                "diff",
                "-U0",
                "--no-color",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ][..],
            &common,
            &[base, "--"],
        ]
        .concat(),
    )?;
    let mut hunks: BTreeMap<String, Vec<Hunk>> = BTreeMap::new();
    let mut read: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    // The file the hunks that follow belong to, and whether it is left to place them in.
    let mut current: Option<(String, bool)> = None;
    let mut old: Option<String> = None;
    // Between `diff --git` and the first `@@`: a removed line reading `-- a/x` is not a header.
    let mut in_header = false;
    for line in diff.split(|&b| b == b'\n') {
        if line.starts_with(b"diff --git ") {
            in_header = true;
            current = None;
            old = None;
        } else if in_header && let Some(field) = line.strip_prefix(b"--- ") {
            old = header_path(field, b"a/").flatten();
        } else if in_header && let Some(field) = line.strip_prefix(b"+++ ") {
            current = match header_path(field, b"b/") {
                Some(Some(new)) => Some((new, true)),
                // Deleted: its hunks are counted, with no text left to place them in.
                Some(None) => old.take().map(|old| (old, false)),
                // Unreadable: its hunks go uncounted, so its file is not complete below.
                None => None,
            };
        } else if let Some(rest) = line.strip_prefix(b"@@ ") {
            in_header = false;
            let Some((file, placed)) = &current else {
                continue;
            };
            let Some(hunk) = hunk_header(rest) else {
                let why = format!(
                    "a hunk header cannot be read: {}",
                    String::from_utf8_lossy(line)
                );
                changes.insert(file.clone(), Change::Unknown(why));
                continue;
            };
            let total = read.entry(file.clone()).or_default();
            total.0 += u64::from(hunk.added);
            total.1 += u64::from(hunk.removed);
            if *placed {
                hunks.entry(file.clone()).or_default().push(hunk);
            }
        }
    }
    // A file whose hunks do not add up to the lines git counted for it lost some to a header
    // that could not be read.
    for (file, change) in changes.iter_mut() {
        let Change::Hunks(placed) = change else {
            continue;
        };
        let want = counted.get(file).copied().unwrap_or_default();
        let got = read.get(file).copied().unwrap_or_default();
        if want == got {
            *placed = hunks.remove(file).unwrap_or_default();
        } else {
            *change = Change::Unknown(format!(
                "git counts {} added and {} removed line(s), but its hunks that could be read add up to {} and {}",
                want.0, want.1, got.0, got.1
            ));
        }
    }
    for file in read.keys() {
        if !changes.contains_key(file) {
            let why = "the diff has hunks for it, but git's list of changed files does not name it";
            changes.insert(file.clone(), Change::Unknown(why.to_string()));
        }
    }

    // Untracked files count as entirely new. NUL-terminated, so never quoted.
    let status = git(
        root,
        &["status", "--porcelain=v1", "-z", "-uall", "--no-renames"],
    )?;
    let mut entries = status.split(|&b| b == 0);
    while let Some(entry) = entries.next() {
        let (Some(xy), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        // A rename or a copy names its source in a field of its own.
        if xy.contains(&b'R') || xy.contains(&b'C') {
            entries.next();
            continue;
        }
        if xy != b"??" {
            continue;
        }
        let (path, unreadable_path) = path_text(path);
        changes
            .entry(path)
            .or_insert_with(|| match unreadable_path {
                Some(why) => Change::Unknown(why),
                None => Change::Hunks(vec![Hunk {
                    start: 1,
                    added: u32::MAX,
                    removed: 0,
                }]),
            });
    }
    Ok(changes)
}

fn is_source_file(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|e| e.to_str()),
        Some(
            "rs" | "go"
                | "py"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "c"
                | "cc"
                | "cpp"
                | "cxx"
                | "h"
                | "hpp"
                | "hh"
                | "swift"
                | "m"
                | "mm"
        )
    )
}

/// The 1-based line or column an LSP position's 0-based `field` gives, when it is a number
/// that fits: a negative, fractional or oversized one is unreadable, not line 1.
pub(crate) fn one_based(position: &serde_json::Value, field: &str) -> Option<u32> {
    u32::try_from(position.get(field)?.as_u64()?)
        .ok()?
        .checked_add(1)
}

fn symbol_range(sym: &serde_json::Value) -> Option<(u32, u32, u32, u32)> {
    let range = sym
        .get("range")
        .or_else(|| sym.get("location").and_then(|l| l.get("range")))?;
    let sel = sym.get("selectionRange").unwrap_or(range);
    let start = one_based(range.get("start")?, "line")?;
    let end = one_based(range.get("end")?, "line")?;
    let sl = one_based(sel.get("start")?, "line")?;
    let sc = one_based(sel.get("start")?, "character")?;
    (start <= end).then_some((start, end, sl, sc))
}

/// Functions and methods (LSP kinds 6, 9, 12) in a document, flattened with their ranges. An
/// entry that is not a symbol (no name or kind, children that are not a list, a function
/// without a readable range) is an error: skipped, it would hide a changed function.
fn collect_functions(
    symbols: &[serde_json::Value],
    out: &mut Vec<(String, u32, u32, u32, u32)>,
) -> std::result::Result<(), String> {
    for sym in symbols {
        let malformed = || unreadable("textDocument/documentSymbol", sym);
        let (Some(name), Some(kind)) = (
            sym.get("name").and_then(|n| n.as_str()),
            sym.get("kind").and_then(|k| k.as_u64()),
        ) else {
            return Err(malformed());
        };
        if name.is_empty() || !(1..=26).contains(&kind) {
            return Err(malformed());
        }
        if matches!(kind, 6 | 9 | 12) && !name.is_empty() {
            let (start, end, sl, sc) = symbol_range(sym).ok_or_else(malformed)?;
            out.push((name.to_string(), start, end, sl, sc));
        }
        match sym.get("children") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(children)) => collect_functions(children, out)?,
            Some(_) => return Err(malformed()),
        }
    }
    Ok(())
}

/// What the text around a caller's declaration says about it being a test, beyond its name:
/// a test attribute above it (`#[test]`, `#[tokio::test]`, `@Test`), a test registration it
/// sits in (`TEST(Suite, Name)`, `TEST_F`, `TEST_CASE("…")`), or, for Python, a `test*` method
/// of a `unittest.TestCase` class. `Some(name)` is a test, under the name its runner selects
/// it by (`Suite.Name` for a gtest registration); `None` is not one as far as the text shows.
pub fn test_marker(language: &str, text: &str, line: u32, name: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let at = (line as usize).checked_sub(1)?;
    let here = *lines.get(at)?;
    let bare = name
        .split('(')
        .next()
        .unwrap_or(name)
        .rsplit(['.', ':'])
        .next()
        .unwrap_or(name);
    // The attribute lines right above the declaration, nearest first.
    let above = || {
        lines[..at].iter().rev().map(|l| l.trim()).take_while(|l| {
            let attribute = l.starts_with("#[") || l.starts_with('@') || l.starts_with("///");
            // `@Test func adds()` is a declaration of its own, not an attribute of the next.
            let declares = [" fn ", "func ", "def "].iter().any(|k| l.contains(k));
            attribute && !declares
        })
    };
    match language {
        "rust" => {
            let attribute = |l: &str| {
                l.starts_with("#[")
                    && (l.contains("test]")
                        || l.contains("test(")
                        || l.contains("::test")
                        || l.starts_with("#[rstest")
                        || l.starts_with("#[test_case"))
            };
            (above().any(attribute) || attribute(here.trim())).then(|| name.to_string())
        }
        "swift" => (above().any(|l| l.starts_with("@Test")) || here.contains("@Test"))
            .then(|| name.to_string()),
        "cpp" => lines[at.saturating_sub(2)..=at]
            .iter()
            .rev()
            .find_map(|l| registration(l)),
        "python" => {
            if !bare.starts_with("test") {
                return None;
            }
            let indent = here.len() - here.trim_start().len();
            lines[..at]
                .iter()
                .rev()
                .find(|l| {
                    let t = l.trim_start();
                    t.starts_with("class ") && l.len() - t.len() < indent
                })
                .filter(|class| class.contains("TestCase"))
                .map(|_| name.to_string())
        }
        _ => None,
    }
}

/// The test a gtest or Catch2 registration on `line` declares: `TEST(Suite, Name)` → `Suite.Name`,
/// `TEST_CASE("adds")` → `adds`.
pub fn registration(line: &str) -> Option<String> {
    let t = line.trim_start();
    for macro_name in ["TYPED_TEST_P", "TYPED_TEST", "TEST_F", "TEST_P", "TEST"] {
        if let Some(rest) = t.strip_prefix(macro_name)
            && let Some(args) = rest.trim_start().strip_prefix('(')
        {
            let args = args.split(')').next()?;
            let (suite, test) = args.split_once(',')?;
            let (suite, test) = (suite.trim(), test.trim());
            let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
            return (ok(suite) && ok(test)).then(|| format!("{suite}.{test}"));
        }
    }
    for macro_name in ["TEST_CASE", "SCENARIO"] {
        if let Some(rest) = t.strip_prefix(macro_name)
            && let Some(args) = rest.trim_start().strip_prefix('(')
        {
            let quoted = args.trim_start().strip_prefix('"')?;
            return quoted.split('"').next().map(str::to_string);
        }
    }
    None
}

/// Whether a caller is a test by name or file conventions of `language`.
pub fn looks_like_test(language: &str, name: &str, file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    // `SignalTests.testDoubles()` / `pkg.TestX`: judge the unqualified name.
    let name = name
        .split('(')
        .next()
        .unwrap_or(name)
        .rsplit('.')
        .next()
        .unwrap_or(name);
    match language {
        "rust" => {
            name.starts_with("test") || lower.contains("/tests/") || lower.ends_with("_test.rs")
        }
        "go" => name.starts_with("Test") && lower.ends_with("_test.go"),
        "python" => {
            let base = lower.rsplit('/').next().unwrap_or("");
            name.starts_with("test")
                && (base.starts_with("test_")
                    || base.ends_with("_test.py")
                    || lower.contains("/tests/"))
        }
        "typescript" => {
            lower.contains(".test.") || lower.contains(".spec.") || lower.contains("/__tests__/")
        }
        "swift" => name.starts_with("test") && lower.ends_with("tests.swift"),
        "cpp" => lower.contains("test"),
        _ => name.to_ascii_lowercase().contains("test"),
    }
}

/// The command that runs only `tests` for `language`, when selection is possible.
pub fn test_command(
    language: &str,
    tools: &crate::verify::ProjectTools,
    tests: &[Symbol],
) -> Option<Vec<String>> {
    if tests.is_empty() {
        return None;
    }
    let names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
    let go_name = |n: &str| n.split('.').next_back().unwrap_or(n).to_string();
    Some(match language {
        "rust" => {
            let mut c = vec![
                "cargo".to_string(),
                "test".to_string(),
                "--workspace".to_string(),
                "--".to_string(),
            ];
            c.extend(names.iter().map(|n| n.to_string()));
            c
        }
        "go" => {
            // The packages that hold the tests, not `./...`: that builds every package's test
            // binary to run a filter most of them never match, 25 s against 0.7 s for one
            // package of a large module (#371).
            let mut packages: Vec<String> = tests
                .iter()
                .map(|t| {
                    let dir = std::path::Path::new(&t.file)
                        .parent()
                        .map(|d| d.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();
                    if dir.is_empty() {
                        ".".to_string()
                    } else {
                        format!("./{dir}")
                    }
                })
                .collect();
            packages.sort_unstable();
            packages.dedup();
            let mut c = vec!["go".to_string(), "test".to_string()];
            c.extend(packages);
            c.push("-run".to_string());
            c.push(format!(
                "^({})$",
                names
                    .iter()
                    .map(|n| go_name(n))
                    .collect::<Vec<_>>()
                    .join("|")
            ));
            c
        }
        "python" => {
            let mut c = crate::verify::plan_command_with(
                tools,
                "python",
                crate::verify::VerifyKind::Test,
                None,
            )
            .ok()?;
            // A test file pytest would not collect by its name (a `TestCase` in
            // `checks/check_price.py`) is collected when it is named on the command line.
            let mut files: Vec<&str> = tests.iter().map(|t| t.file.as_str()).collect();
            files.sort_unstable();
            files.dedup();
            c.extend(files.iter().map(|f| f.to_string()));
            c.push("-k".to_string());
            c.push(names.join(" or "));
            c
        }
        "typescript" => {
            let mut c = crate::verify::plan_command_with(
                tools,
                "typescript",
                crate::verify::VerifyKind::Test,
                None,
            )
            .ok()?;
            let selectable = c
                .iter()
                .any(|w| w == "vitest" || w == "jest" || w == "test");
            // Module-level test files are selected by path, named tests by pattern.
            let (files, named): (Vec<&str>, Vec<&str>) =
                names.iter().partition(|n| n.contains('/'));
            if selectable {
                c.extend(files.iter().map(|f| f.to_string()));
                if !named.is_empty() {
                    c.push("-t".to_string());
                    c.push(named.join("|"));
                }
            }
            c
        }
        "swift" => {
            let mut c = vec!["swift".to_string(), "test".to_string()];
            for n in &names {
                c.push("--filter".to_string());
                c.push(n.trim_end_matches("()").to_string());
            }
            c
        }
        // ctest selects by the registered name (`Suite.Name`, a Catch2 description).
        "cpp" => crate::verify::plan_command_with(
            tools,
            "cpp",
            crate::verify::VerifyKind::Test,
            Some(&format!("^({})$", names.join("|"))),
        )
        .ok()?,
        _ => return None,
    })
}

fn rel(root: &Path, uri: &str) -> String {
    let path = crate::remote_fs::uri_to_path(uri);
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    Path::new(&path)
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(path)
}

/// Runs the analysis against the checkout at `root` placed on `remote`.
pub async fn analyze(
    remote: SocketAddr,
    root: &Path,
    base: Option<&str>,
    depth: usize,
) -> Result<ImpactReport> {
    let language = crate::sync::expected_engine(root)
        .ok_or_else(|| anyhow!("no project manifest at {}", root.display()))?
        .to_string();
    let tools = crate::verify::detect_tools(root);
    let changes = diff_hunks(root, base)?;
    // sourcekit-lsp 5 finds a caller in another file only through the index store a build
    // leaves; without one every answer is empty and reads like "nothing calls this" (#166).
    let index = if language == "swift" && !changes.is_empty() {
        let command = vec![
            "swift".to_string(),
            "build".to_string(),
            "--build-tests".to_string(),
        ];
        let outcome = crate::exec::run_remote(
            remote,
            root,
            None,
            command.clone(),
            Vec::new(),
            900,
            false,
            |_, _| {},
        )
        .await;
        Some(IndexBuild {
            command: command.join(" "),
            ok: outcome
                .as_ref()
                .is_ok_and(|o| o.exit.exit_code == Some(0) && !o.exit.timed_out),
            duration_ms: outcome.as_ref().map_or(0, |o| o.exit.duration_ms),
        })
    } else {
        None
    };
    let mut session = LspSession::open(remote, root, None).await?;
    let changed_files: Vec<String> = changes.keys().cloned().collect();
    let mut changed: Vec<Symbol> = Vec::new();
    let mut unattributed: Vec<String> = Vec::new();
    let mut incomplete: Vec<Gap> = Vec::new();

    for (file, change) in &changes {
        if !is_source_file(file) {
            unattributed.push(file.clone());
            continue;
        }
        let file_hunks = match change {
            Change::Hunks(hunks) => hunks,
            Change::Unknown(error) => {
                let (file, error) = (file.clone(), error.clone());
                note(&mut incomplete, Gap::Diff { file, error });
                continue;
            }
        };
        let abs: PathBuf = root.join(file);
        // A deleted file's functions are gone, and whoever called them changed with them.
        if !abs.exists() {
            note(&mut incomplete, Gap::Deleted { file: file.clone() });
            continue;
        }
        if file_hunks.is_empty() {
            continue; // git counts no changed line: only its mode changed
        }
        let text = match std::fs::read_to_string(&abs) {
            Ok(text) => text,
            Err(e) => {
                let error = format!("it cannot be read: {e}");
                note(
                    &mut incomplete,
                    Gap::Symbols {
                        file: file.clone(),
                        error,
                    },
                );
                continue;
            }
        };
        let uri = Url::from_file_path(&abs)
            .map_err(|_| anyhow!("bad path {file}"))?
            .to_string();
        let symbols = match session
            .query(
                &abs,
                "textDocument/documentSymbol",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await
        {
            Ok(serde_json::Value::Array(symbols)) => symbols,
            // The protocol's "no result": it does not say the file has no functions.
            Ok(serde_json::Value::Null) => {
                let error =
                    "textDocument/documentSymbol answered null, so its functions are unknown"
                        .to_string();
                note(
                    &mut incomplete,
                    Gap::Symbols {
                        file: file.clone(),
                        error,
                    },
                );
                continue;
            }
            Ok(other) => {
                let error = unreadable("textDocument/documentSymbol", &other);
                note(
                    &mut incomplete,
                    Gap::Symbols {
                        file: file.clone(),
                        error,
                    },
                );
                continue;
            }
            Err(e) => {
                let error = format!("{e:#}");
                note(
                    &mut incomplete,
                    Gap::Symbols {
                        file: file.clone(),
                        error,
                    },
                );
                continue;
            }
        };
        let mut functions = Vec::new();
        if let Err(error) = collect_functions(&symbols, &mut functions) {
            let file = file.clone();
            note(&mut incomplete, Gap::Symbols { file, error });
            continue;
        }
        for (name, start, end, sl, sc) in &functions {
            if file_hunks.iter().any(|h| h.touches(*start, *end)) {
                let sym = Symbol {
                    name: name.clone(),
                    file: file.clone(),
                    line: *sl,
                    col: *sc,
                };
                if !changed.contains(&sym) {
                    changed.push(sym);
                }
            }
        }
        // Each hunk on its own: a changed function does not vouch for a changed import or
        // constant elsewhere in the same file.
        let spans: Vec<(u32, u32)> = functions.iter().map(|f| (f.1, f.2)).collect();
        let lines: Vec<&str> = text.lines().collect();
        if file_hunks.iter().any(|h| !h.inside(&spans, &lines)) {
            unattributed.push(file.clone());
        }
    }

    // A changed test is affected by its own change, whatever calls it.
    let mut tests: BTreeSet<Symbol> = BTreeSet::new();
    let mut reaches: Vec<Reach> = Vec::new();
    let mut origins: Vec<(Symbol, bool)> = Vec::with_capacity(changed.len());
    for sym in &changed {
        let test =
            test_name(root, &language, &sym.name, &sym.file, sym.line, false).map(|name| Symbol {
                name,
                ..sym.clone()
            });
        if let Some(test) = &test {
            tests.insert(test.clone());
            reaches.push(Reach {
                test: test.clone(),
                changed: sym.clone(),
                hops: 0,
            });
        }
        origins.push((sym.clone(), test.is_some()));
    }

    // Walk incoming calls breadth-first from each changed function on its own, so every test
    // reached knows which changed functions reach it and in how many hops. The answers are
    // cached: a function two walks pass through is asked once.
    let key = |s: &Symbol| (s.file.clone(), s.line, s.col);
    let changed_keys: HashSet<(String, u32, u32)> = changed.iter().map(key).collect();
    let mut cache: HashMap<(String, u32, u32), Incoming> = HashMap::new();
    // A language server that has just started answers the call hierarchy with nothing until it
    // has read the project, and "no callers" would then read as "no test is affected" (#202).
    // For the managed servers, an empty answer for the first changed function is asked again a
    // few times before it is believed; rust-analyzer answers from a database already loaded.
    if language != "rust"
        && let Some(first) = changed.first()
    {
        let mut answer = incoming_calls(&mut session, root, &language, first).await;
        for _ in 0..COLD_RETRIES {
            if matches!(&answer, Incoming::Callers(found) if !found.is_empty()) {
                break;
            }
            tokio::time::sleep(COLD_WAIT).await;
            answer = incoming_calls(&mut session, root, &language, first).await;
        }
        cache.insert(key(first), answer);
    }
    let mut callers: BTreeSet<Symbol> = BTreeSet::new();
    for (origin, origin_is_test) in &origins {
        let mut seen: HashSet<(String, u32, u32)> = HashSet::from([key(origin)]);
        let mut queue: VecDeque<(Symbol, bool, usize)> =
            VecDeque::from([(origin.clone(), *origin_is_test, 0)]);
        while let Some((sym, is_test, level)) = queue.pop_front() {
            if let std::collections::hash_map::Entry::Vacant(slot) = cache.entry(key(&sym)) {
                slot.insert(incoming_calls(&mut session, root, &language, &sym).await);
            }
            let found = match &cache[&key(&sym)] {
                Incoming::Callers(found) => found.clone(),
                // A test is selected whatever calls it, and module-level test code (a test
                // file's top-level `it(...)`) has no item of its own.
                Incoming::NoItem if is_test => Vec::new(),
                Incoming::NoItem => {
                    let error = "the analyzer has no call-hierarchy item at its name".to_string();
                    note(&mut incomplete, Gap::Callers { symbol: sym, error });
                    continue;
                }
                Incoming::Failed(error) => {
                    let error = error.clone();
                    note(&mut incomplete, Gap::Callers { symbol: sym, error });
                    continue;
                }
            };
            if level >= depth {
                // Asked one level further, the answer says whether the limit cut the walk
                // short: a caller not yet seen is a test path left unexplored.
                if found.iter().any(|(caller, _)| !seen.contains(&key(caller))) {
                    note(&mut incomplete, Gap::Depth { symbol: sym, depth });
                }
                continue;
            }
            for (caller, caller_is_test) in found {
                if !seen.insert(key(&caller)) {
                    continue;
                }
                if caller_is_test {
                    tests.insert(caller.clone());
                    reaches.push(Reach {
                        test: caller.clone(),
                        changed: origin.clone(),
                        hops: level + 1,
                    });
                } else if !changed_keys.contains(&key(&caller)) {
                    callers.insert(caller.clone());
                }
                queue.push_back((caller, caller_is_test, level + 1));
            }
        }
    }

    session.close().await;
    let tests: Vec<Symbol> = tests.into_iter().collect();
    let test_command = test_command(&language, &tools, &tests);
    Ok(ImpactReport {
        language,
        base: base.unwrap_or("HEAD").to_string(),
        changed_files,
        changed,
        callers: callers.into_iter().collect(),
        tests,
        test_command,
        unattributed_files: unattributed,
        index,
        reaches,
        incomplete,
    })
}

/// Adds `gap` unless it is already noted: two walks through one function meet the same gap.
fn note(gaps: &mut Vec<Gap>, gap: Gap) {
    if !gaps.contains(&gap) {
        gaps.push(gap);
    }
}

/// Says that `method` answered with something other than the shape the protocol gives it,
/// showing the start of the answer.
pub(crate) fn unreadable(method: &str, answer: &serde_json::Value) -> String {
    let text = answer.to_string();
    let shown: String = text.chars().take(80).collect();
    let cut = if shown.len() < text.len() { "…" } else { "" };
    format!("{method} answered with something it cannot read: {shown}{cut}")
}

/// The name a test runner selects the function `name` declared at `line` of `file` by, when it
/// is a test: flagged by the analyzer, named or placed like one, or marked as one in its source.
fn test_name(
    root: &Path,
    language: &str,
    name: &str,
    file: &str,
    line: u32,
    flagged: bool,
) -> Option<String> {
    if flagged || looks_like_test(language, name, file) {
        return Some(name.to_string());
    }
    // Beyond names: an attribute, a registration macro, a `TestCase` class (#201).
    let text = std::fs::read_to_string(root.join(file)).ok()?;
    test_marker(language, &text, line, name)
}

/// Times an empty answer from a managed language server is asked again, and the wait before
/// each: a server still reading the project answers empty (#202, #284).
pub(crate) const COLD_RETRIES: usize = 3;
pub(crate) const COLD_WAIT: std::time::Duration = std::time::Duration::from_millis(800);

/// What the analyzer said about the callers of one function.
enum Incoming {
    /// The functions that call it, each with whether it is a test; possibly none.
    Callers(Vec<(Symbol, bool)>),
    /// It has no call-hierarchy item at the function's name.
    NoItem,
    /// A request failed, or its answer is not the shape the protocol gives it.
    Failed(String),
}

/// The functions that call `sym`, each with whether it is a test (the analyzer's `isTest`, the
/// language's naming conventions, or a marker in the source).
async fn incoming_calls(
    session: &mut LspSession,
    root: &Path,
    language: &str,
    sym: &Symbol,
) -> Incoming {
    let abs = root.join(&sym.file);
    let Ok(uri) = Url::from_file_path(&abs).map(|u| u.to_string()) else {
        return Incoming::Failed(format!("{} is not a file path", sym.file));
    };
    let position = serde_json::json!({ "line": sym.line.saturating_sub(1), "character": sym.col.saturating_sub(1) });
    let prepare = "textDocument/prepareCallHierarchy";
    let items = match session
        .query(
            &abs,
            prepare,
            serde_json::json!({ "textDocument": { "uri": uri }, "position": position }),
        )
        .await
    {
        Ok(items) => items,
        Err(e) => return Incoming::Failed(format!("{e:#}")),
    };
    let items = match items {
        serde_json::Value::Null => return Incoming::NoItem,
        serde_json::Value::Array(all) if all.is_empty() => return Incoming::NoItem,
        serde_json::Value::Array(all) if all.iter().all(|item| item.is_object()) => all,
        other => return Incoming::Failed(unreadable(prepare, &other)),
    };
    // One name can stand for several items (a declaration and its definition, overloads):
    // the callers of each are callers of the function.
    let method = "callHierarchy/incomingCalls";
    let mut out: Vec<(Symbol, bool)> = Vec::new();
    for item in items {
        let incoming = match session
            .query(&abs, method, serde_json::json!({ "item": item }))
            .await
        {
            Ok(incoming) => incoming,
            Err(e) => return Incoming::Failed(format!("{e:#}")),
        };
        let edges = match incoming {
            // The protocol's "no calls", after an item was found.
            serde_json::Value::Null => continue,
            serde_json::Value::Array(edges) => edges,
            other => return Incoming::Failed(unreadable(method, &other)),
        };
        for edge in &edges {
            match caller(root, language, edge) {
                Ok(Some(found)) if !out.contains(&found) => out.push(found),
                Ok(_) => {}
                Err(error) => return Incoming::Failed(error),
            }
        }
    }
    Incoming::Callers(out)
}

/// The caller an incoming call names, with whether it is a test; `None` when it lies outside
/// the checkout. A call without its caller's name or place, or with a position that is not a
/// line and column, is an error: skipped, it would read as "no caller".
fn caller(
    root: &Path,
    language: &str,
    edge: &serde_json::Value,
) -> std::result::Result<Option<(Symbol, bool)>, String> {
    let malformed = || unreadable("callHierarchy/incomingCalls", edge);
    let from = edge
        .get("from")
        .filter(|f| f.is_object())
        .ok_or_else(malformed)?;
    let uri = from
        .get("uri")
        .and_then(|u| u.as_str())
        .ok_or_else(malformed)?;
    let name = from
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(malformed)?;
    let start = from
        .get("selectionRange")
        .or_else(|| from.get("range"))
        .and_then(|r| r.get("start"))
        .ok_or_else(malformed)?;
    let (line, col) = one_based(start, "line")
        .zip(one_based(start, "character"))
        .ok_or_else(malformed)?;
    let flagged = match edge.get("isTest") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(flag)) => *flag,
        Some(_) => return Err(malformed()),
    };
    let file = rel(root, uri);
    if file.starts_with('/') {
        return Ok(None); // outside the checkout
    }
    // Module-level code (a test file's top-level `it(...)` calls) is reported with the file as
    // its name: keep it checkout-relative.
    let mut name = if name.starts_with('/') {
        rel(root, name)
    } else {
        name.to_string()
    };
    let is_test = match test_name(root, language, &name, &file, line, flagged) {
        Some(test) => {
            name = test;
            true
        }
        None => false,
    };
    Ok(Some((
        Symbol {
            name,
            file,
            line,
            col,
        },
        is_test,
    )))
}

/// The changed functions that reach the failing test `name` (`tests::doubles`,
/// `MathTests.testAdds()`, `tests/test_x.py::test_y`), nearest first, each once.
pub fn suspects_for(reaches: &[Reach], name: &str) -> Vec<(Symbol, usize)> {
    let bare = |n: &str| -> String {
        n.rsplit("::")
            .next()
            .unwrap_or(n)
            .rsplit('.')
            .next()
            .unwrap_or(n)
            .trim_end_matches("()")
            .to_string()
    };
    let wanted = bare(name);
    let mut best: BTreeMap<Symbol, usize> = BTreeMap::new();
    for r in reaches.iter().filter(|r| bare(&r.test.name) == wanted) {
        let hops = best.entry(r.changed.clone()).or_insert(r.hops);
        *hops = (*hops).min(r.hops);
    }
    let mut out: Vec<(Symbol, usize)> = best.into_iter().collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conventions_per_language() {
        assert!(looks_like_test("go", "TestAdd", "pkg/add_test.go"));
        assert!(looks_like_test("go", "pkg.TestAdd", "pkg/add_test.go"));
        assert!(!looks_like_test("go", "helper", "pkg/add_test.go"));
        assert!(looks_like_test(
            "swift",
            "MathTests.testAdds()",
            "Tests/MathTests/MathTests.swift"
        ));
        assert!(looks_like_test(
            "rust",
            "adds_numbers",
            "crates/a/tests/it.rs"
        ));
        assert!(looks_like_test("python", "test_adds", "tests/test_math.py"));
        assert!(looks_like_test("typescript", "adds", "src/math.test.ts"));
        assert!(looks_like_test(
            "swift",
            "testAdds",
            "Tests/MathTests/MathTests.swift"
        ));
    }

    #[test]
    fn test_commands_select_tests() {
        let tools = crate::verify::ProjectTools::default();
        let t = |name: &str| Symbol {
            name: name.into(),
            file: "x".into(),
            line: 1,
            col: 1,
        };
        // Go runs the packages that hold the tests, each once (#371).
        let in_file = |name: &str, file: &str| Symbol {
            name: name.into(),
            file: file.into(),
            line: 1,
            col: 1,
        };
        assert_eq!(
            test_command(
                "go",
                &tools,
                &[
                    in_file("TestA", "internal/push/a_test.go"),
                    in_file("pkg.TestB", "internal/push/b_test.go"),
                    in_file("TestC", "cmd/tool/c_test.go"),
                    in_file("TestD", "main_test.go"),
                ]
            )
            .unwrap()
            .join(" "),
            "go test . ./cmd/tool ./internal/push -run ^(TestA|TestB|TestC|TestD)$"
        );
        assert_eq!(
            test_command("rust", &tools, &[t("a"), t("b")])
                .unwrap()
                .join(" "),
            "cargo test --workspace -- a b"
        );
        let py = test_command("python", &tools, &[t("test_a")]).unwrap();
        assert!(py.ends_with(&["-k".to_string(), "test_a".to_string()]));
        // ctest selects registered tests by name (#201).
        let cpp = test_command("cpp", &tools, &[t("Price.Doubles"), t("adds up")])
            .unwrap()
            .join(" ");
        assert!(
            cpp.contains("ctest") && cpp.contains("-R '^(Price.Doubles|adds up)$'"),
            "{cpp}"
        );
        let ts = test_command("typescript", &tools, &[t("tests/a.test.ts"), t("adds")]).unwrap();
        assert!(ts.ends_with(&[
            "tests/a.test.ts".to_string(),
            "-t".to_string(),
            "adds".to_string()
        ]));
        assert!(test_command("go", &tools, &[]).is_none());
    }

    #[test]
    fn a_test_is_found_by_attribute_registration_or_test_case_class() {
        let rust =
            "mod checks {\n    #[tokio::test]\n    async fn prices() {}\n    fn helper() {}\n}\n";
        assert_eq!(
            test_marker("rust", rust, 3, "prices").as_deref(),
            Some("prices")
        );
        assert_eq!(test_marker("rust", rust, 4, "helper"), None);
        let swift = "@Test func adds() {}\nfunc plain() {}\n";
        assert!(test_marker("swift", swift, 1, "adds()").is_some());
        assert!(test_marker("swift", swift, 2, "plain()").is_none());
        let cpp = "#include <gtest/gtest.h>\nTEST(Price, Doubles) {\n  EXPECT_EQ(price(2, 3), 6);\n}\nTEST_CASE(\"adds up\") {\n}\n";
        assert_eq!(
            test_marker("cpp", cpp, 2, "TestBody").as_deref(),
            Some("Price.Doubles")
        );
        assert_eq!(
            test_marker("cpp", cpp, 3, "TestBody").as_deref(),
            Some("Price.Doubles")
        );
        assert_eq!(test_marker("cpp", cpp, 5, "x").as_deref(), Some("adds up"));
        assert_eq!(
            registration("TEST_F(Suite, Name)"),
            Some("Suite.Name".into())
        );
        assert_eq!(registration("TEST(, x)"), None);
        let py = "import unittest\n\nclass PriceChecks(unittest.TestCase):\n    def test_doubles(self):\n        pass\n\n    def helper(self):\n        pass\n\nclass Other:\n    def test_not(self):\n        pass\n";
        assert!(test_marker("python", py, 4, "test_doubles").is_some());
        assert!(test_marker("python", py, 7, "helper").is_none());
        assert!(test_marker("python", py, 11, "test_not").is_none());
        assert!(test_marker("go", "func TestX(t *testing.T) {}\n", 1, "TestX").is_none());
        assert!(test_marker("rust", "", 9, "x").is_none());
    }

    #[test]
    fn the_whole_suite_runs_when_the_selection_cannot_be_trusted() {
        let mut report = ImpactReport {
            language: "rust".into(),
            base: "HEAD".into(),
            changed_files: vec!["src/lib.rs".into()],
            changed: vec![Symbol {
                name: "price".into(),
                file: "src/lib.rs".into(),
                line: 3,
                col: 8,
            }],
            callers: Vec::new(),
            tests: vec![Symbol {
                name: "prices".into(),
                file: "src/lib.rs".into(),
                line: 9,
                col: 8,
            }],
            test_command: None,
            unattributed_files: Vec::new(),
            index: None,
            reaches: Vec::new(),
            incomplete: Vec::new(),
        };
        assert_eq!(report.full_suite_reason(), None);
        // Tests reached, but no way to select them.
        let decision = report.ci_decision();
        assert_eq!(decision.run, CiRun::WholeSuite);
        assert!(
            decision.why.contains("cannot be selected"),
            "{}",
            decision.why
        );
        report.test_command = Some(vec!["cargo".into(), "test".into()]);
        assert_eq!(
            report.ci_decision(),
            CiDecision {
                run: CiRun::Selected(vec!["cargo".into(), "test".into()]),
                why: "1 test(s) that reach the change".into(),
            }
        );
        // A gap makes the selection untrustworthy, whatever it selected.
        report.incomplete = vec![Gap::Deleted {
            file: "src/gone.rs".into(),
        }];
        let decision = report.ci_decision();
        assert_eq!(decision.run, CiRun::WholeSuite);
        assert!(
            decision.why.contains("src/gone.rs was deleted"),
            "{}",
            decision.why
        );
        assert!(report.render().contains("incomplete analysis"));
        assert!(
            report
                .ci_summary(None, "x")
                .contains("- src/gone.rs was deleted")
        );
        report.incomplete.clear();
        report.test_command = None;
        let summary = report.ci_summary(Some(&["cargo".into(), "test".into()]), "the selection");
        assert!(
            summary.contains("| `price` | `src/lib.rs:3` |"),
            "{summary}"
        );
        assert!(summary.contains("- `prices` (`src/lib.rs:9`)"), "{summary}");
        assert!(
            summary.contains("Ran `cargo test`: the selection."),
            "{summary}"
        );
        assert!(
            report
                .ci_summary(None, "no test reaches them")
                .contains("Ran nothing")
        );
        report.unattributed_files = vec!["Cargo.toml".into()];
        assert!(report.full_suite_reason().unwrap().contains("Cargo.toml"));
        report.index = Some(IndexBuild {
            command: "swift build".into(),
            ok: false,
            duration_ms: 1,
        });
        assert!(report.full_suite_reason().unwrap().contains("index"));
    }

    #[test]
    fn a_hunk_is_inside_functions_only_when_all_it_changes_is() {
        // Two functions, lines 1-3 and 5-7, a blank line between them.
        let spans = [(1, 3), (5, 7)];
        let lines = ["fn a() {", "  1", "}", "", "fn b() {", "  2", "}", "use x;"];
        let hunk = |start, added, removed| Hunk {
            start,
            added,
            removed,
        };
        assert!(hunk(2, 1, 1).inside(&spans, &lines));
        assert!(hunk(2, 1, 1).touches(1, 3));
        assert!(!hunk(2, 1, 1).touches(5, 7));
        // A rewritten line between the functions, or a new import after them, is outside.
        assert!(!hunk(4, 1, 1).inside(&spans, &lines));
        assert!(!hunk(8, 1, 0).inside(&spans, &lines));
        // A function added with the blank line that separates it: the blank changes nothing.
        assert!(hunk(4, 4, 0).inside(&spans, &lines));
        // Removed inside `a`, or removed between the two functions (a whole item gone).
        assert!(hunk(2, 0, 3).inside(&spans, &lines));
        assert!(hunk(2, 0, 3).touches(1, 3));
        assert!(!hunk(3, 0, 4).inside(&spans, &lines));
        assert!(!hunk(3, 0, 4).touches(1, 3));
        assert!(!hunk(0, 0, 2).inside(&spans, &lines));
        // Past the end of the text, nothing is left to place.
        assert!(hunk(20, 2, 0).inside(&spans, &lines));
    }

    #[test]
    fn a_quoted_git_path_is_decoded_and_a_broken_one_is_refused() {
        assert_eq!(git_path(b"a/src/lib.rs"), Some(b"a/src/lib.rs".to_vec()));
        // A name with a space is followed by a tab that is not part of it.
        assert_eq!(git_path(b"b/sp ace.rs\t"), Some(b"b/sp ace.rs".to_vec()));
        assert_eq!(
            git_path(b"\"b/\\303\\274 x.rs\"\t"),
            Some("b/ü x.rs".as_bytes().to_vec())
        );
        assert_eq!(
            git_path(b"\"a/t\\tq\\\\\\\"z.rs\""),
            Some(b"a/t\tq\\\"z.rs".to_vec())
        );
        assert_eq!(git_path(b"\"a/open"), None);
        assert_eq!(git_path(b"\"a/\\9\""), None);
        assert_eq!(git_path(b"\"a/\\38x\""), None);
        assert_eq!(header_path(b"/dev/null", b"a/"), Some(None));
        assert_eq!(
            header_path(b"\"b/g\\303\\264ne.rs\"", b"b/"),
            Some(Some("gône.rs".to_string()))
        );
        assert_eq!(header_path(b"x/lib.rs", b"b/"), None);
    }

    #[test]
    fn a_hunk_header_is_read_whole_or_not_at_all() {
        let hunk = |start, added, removed| Hunk {
            start,
            added,
            removed,
        };
        assert_eq!(hunk_header(b"-2 +2 @@ fn a() -1 +9"), Some(hunk(2, 1, 1)));
        assert_eq!(hunk_header(b"-1,3 +0,0 @@"), Some(hunk(0, 0, 3)));
        assert_eq!(hunk_header(b"-4,0 +5,2 @@"), Some(hunk(5, 2, 0)));
        assert_eq!(hunk_header(b"-x +2 @@"), None);
        assert_eq!(hunk_header(b"-1 +2,-1 @@"), None);
        assert_eq!(hunk_header(b"-1 +99999999999 @@"), None);
        assert_eq!(hunk_header(b"-1 +2"), None);
    }

    #[test]
    fn a_malformed_document_symbol_is_an_error_not_a_skipped_entry() {
        let function = |line: serde_json::Value| {
            serde_json::json!({
                "name": "f", "kind": 12,
                "range": { "start": { "line": line, "character": 0 }, "end": { "line": 3, "character": 1 } },
                "selectionRange": { "start": { "line": 1, "character": 3 }, "end": { "line": 1, "character": 4 } }
            })
        };
        let mut out = Vec::new();
        assert_eq!(
            collect_functions(&[function(serde_json::json!(1))], &mut out),
            Ok(())
        );
        assert_eq!(out, vec![("f".to_string(), 2, 4, 2, 4)]);
        let module = |children| serde_json::json!({ "name": "m", "kind": 2, "children": children });
        for bad in [
            serde_json::json!(42),
            serde_json::json!({ "kind": 12 }),
            serde_json::json!({ "name": "", "kind": 12 }),
            serde_json::json!({ "name": "f", "kind": 0 }),
            serde_json::json!({ "name": "f", "kind": 99 }),
            serde_json::json!({ "name": "f" }),
            serde_json::json!({ "name": "f", "kind": 12 }),
            function(serde_json::json!(-1)),
            function(serde_json::json!(4_294_967_296u64)),
            function(serde_json::json!(1.5)),
            function(serde_json::json!(9)),
            module(serde_json::json!({ "x": 1 })),
            module(serde_json::json!([function(serde_json::json!("1"))])),
        ] {
            let error = collect_functions(std::slice::from_ref(&bad), &mut Vec::new()).unwrap_err();
            assert!(error.contains("cannot read"), "{bad}: {error}");
        }
        // An empty list, or a symbol with no children, is a complete answer.
        assert_eq!(collect_functions(&[], &mut Vec::new()), Ok(()));
        assert_eq!(
            collect_functions(&[module(serde_json::json!([]))], &mut Vec::new()),
            Ok(())
        );
    }

    #[test]
    fn an_unreadable_answer_is_quoted_short() {
        let long = serde_json::json!({ "x": "y".repeat(200) });
        let said = unreadable("m", &long);
        assert!(
            said.starts_with("m answered with something it cannot read: {"),
            "{said}"
        );
        assert!(said.ends_with('…'), "{said}");
        assert!(unreadable("m", &serde_json::json!(3)).ends_with(": 3"));
    }
}
