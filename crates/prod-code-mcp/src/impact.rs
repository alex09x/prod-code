//! Blast radius of a change (roadmap 8.1): the functions a diff touches, the callers that
//! reach them through the analyzer's call hierarchy, and the tests among those callers,
//! with the command that runs only the affected tests.

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
    /// Why the selection cannot be trusted and the whole suite should run instead: lines changed
    /// outside any function, or an index that could not be built. `None` when it can.
    pub fn full_suite_reason(&self) -> Option<String> {
        if self.index.as_ref().is_some_and(|b| !b.ok) {
            return Some("the analyzer's index could not be built, so callers are unknown".into());
        }
        if !self.unattributed_files.is_empty() {
            return Some(format!(
                "lines changed outside any function in {}",
                self.unattributed_files.join(", ")
            ));
        }
        None
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
        } else if !self.changed.is_empty() {
            out.push_str("affected tests: none reach the changed functions\n");
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
/// untracked files as fully changed.
pub fn changed_lines(root: &Path, base: Option<&str>) -> Result<BTreeMap<String, Vec<(u32, u32)>>> {
    let base = base.unwrap_or("HEAD");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "-U0", "--no-color", base, "--"])
        .output()
        .context("git diff failed")?;
    if !out.status.success() {
        anyhow::bail!(
            "git diff {base} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mut ranges: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            current = Some(path.to_string());
            ranges.entry(path.to_string()).or_default();
        } else if line.starts_with("+++ /dev/null") {
            current = None;
        } else if let (Some(file), Some(rest)) = (&current, line.strip_prefix("@@ ")) {
            // @@ -a,b +c,d @@
            if let Some(plus) = rest.split_whitespace().find(|w| w.starts_with('+')) {
                let spec = &plus[1..];
                let (start, count) = match spec.split_once(',') {
                    Some((s, c)) => (s.parse::<u32>().unwrap_or(0), c.parse::<u32>().unwrap_or(0)),
                    None => (spec.parse::<u32>().unwrap_or(0), 1),
                };
                // A pure deletion (count 0) still touches the surrounding function.
                let end = start + count.max(1) - 1;
                ranges
                    .get_mut(file)
                    .unwrap()
                    .push((start.max(1), end.max(1)));
            }
        }
    }
    // Untracked files count as entirely new.
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "-uall"])
        .output()
        .context("git status failed")?;
    for line in String::from_utf8_lossy(&status.stdout).lines() {
        if let Some(path) = line.strip_prefix("?? ") {
            ranges
                .entry(path.to_string())
                .or_insert_with(|| vec![(1, u32::MAX)]);
        }
    }
    Ok(ranges)
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

fn symbol_range(sym: &serde_json::Value) -> Option<(u32, u32, u32, u32)> {
    let range = sym
        .get("range")
        .or_else(|| sym.get("location").and_then(|l| l.get("range")))?;
    let sel = sym.get("selectionRange").unwrap_or(range);
    let start = range.get("start")?.get("line")?.as_u64()? as u32 + 1;
    let end = range.get("end")?.get("line")?.as_u64()? as u32 + 1;
    let sl = sel.get("start")?.get("line")?.as_u64()? as u32 + 1;
    let sc = sel.get("start")?.get("character")?.as_u64()? as u32 + 1;
    Some((start, end, sl, sc))
}

/// Functions and methods (LSP kinds 6, 9, 12) in a document, flattened with their ranges.
fn collect_functions(symbols: &[serde_json::Value], out: &mut Vec<(String, u32, u32, u32, u32)>) {
    for sym in symbols {
        let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
        let name = sym
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        if matches!(kind, 6 | 9 | 12)
            && !name.is_empty()
            && let Some((start, end, sl, sc)) = symbol_range(sym)
        {
            out.push((name, start, end, sl, sc));
        }
        if let Some(children) = sym.get("children").and_then(|c| c.as_array()) {
            collect_functions(children, out);
        }
    }
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
        "go" => vec![
            "go".to_string(),
            "test".to_string(),
            "./...".to_string(),
            "-run".to_string(),
            format!(
                "^({})$",
                names
                    .iter()
                    .map(|n| go_name(n))
                    .collect::<Vec<_>>()
                    .join("|")
            ),
        ],
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
    let ranges = changed_lines(root, base)?;
    // sourcekit-lsp 5 finds a caller in another file only through the index store a build
    // leaves; without one every answer is empty and reads like "nothing calls this" (#166).
    let index = if language == "swift" && !ranges.is_empty() {
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
    let changed_files: Vec<String> = ranges.keys().cloned().collect();
    let mut changed: Vec<Symbol> = Vec::new();
    let mut unattributed: Vec<String> = Vec::new();

    for (file, lines) in &ranges {
        if !is_source_file(file) {
            unattributed.push(file.clone());
            continue;
        }
        let abs: PathBuf = root.join(file);
        if !abs.is_file() {
            continue;
        }
        let uri = Url::from_file_path(&abs)
            .map_err(|_| anyhow!("bad path {file}"))?
            .to_string();
        let symbols = session
            .query(
                &abs,
                "textDocument/documentSymbol",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await
            .unwrap_or(serde_json::Value::Null);
        let mut functions = Vec::new();
        collect_functions(
            symbols.as_array().map(|a| a.as_slice()).unwrap_or(&[]),
            &mut functions,
        );
        let mut attributed = false;
        for (name, start, end, sl, sc) in &functions {
            let hit = lines.iter().any(|(a, b)| a <= end && b >= start);
            if hit {
                attributed = true;
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
        if !attributed && !functions.is_empty() || functions.is_empty() {
            unattributed.push(file.clone());
        }
    }

    // Walk incoming calls breadth-first from each changed function on its own, so every test
    // reached knows which changed functions reach it and in how many hops. The answers are
    // cached: a function two walks pass through is asked once.
    let key = |s: &Symbol| (s.file.clone(), s.line, s.col);
    let changed_keys: HashSet<(String, u32, u32)> = changed.iter().map(key).collect();
    let mut cache: HashMap<(String, u32, u32), Vec<(Symbol, bool)>> = HashMap::new();
    // A language server that has just started answers the call hierarchy with nothing until it
    // has read the project, and "no callers" would then read as "no test is affected" (#202).
    // For the managed servers, an empty answer for the first changed function is asked again a
    // few times before it is believed; rust-analyzer answers from a database already loaded.
    if language != "rust"
        && let Some(first) = changed.first()
    {
        let mut answer = incoming_calls(&mut session, root, &language, first).await;
        for _ in 0..COLD_RETRIES {
            if !answer.is_empty() {
                break;
            }
            tokio::time::sleep(COLD_WAIT).await;
            answer = incoming_calls(&mut session, root, &language, first).await;
        }
        cache.insert(key(first), answer);
    }
    let mut callers: BTreeSet<Symbol> = BTreeSet::new();
    let mut tests: BTreeSet<Symbol> = BTreeSet::new();
    let mut reaches: Vec<Reach> = Vec::new();
    for origin in &changed {
        let mut seen: HashSet<(String, u32, u32)> = HashSet::from([key(origin)]);
        let mut queue: VecDeque<(Symbol, usize)> = VecDeque::from([(origin.clone(), 0)]);
        while let Some((sym, level)) = queue.pop_front() {
            if level >= depth {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(slot) = cache.entry(key(&sym)) {
                slot.insert(incoming_calls(&mut session, root, &language, &sym).await);
            }
            for (caller, is_test) in cache[&key(&sym)].clone() {
                if !seen.insert(key(&caller)) {
                    continue;
                }
                if is_test {
                    tests.insert(caller.clone());
                    reaches.push(Reach {
                        test: caller.clone(),
                        changed: origin.clone(),
                        hops: level + 1,
                    });
                } else if !changed_keys.contains(&key(&caller)) {
                    callers.insert(caller.clone());
                }
                queue.push_back((caller, level + 1));
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
    })
}

/// Times an empty call hierarchy from a managed language server is asked again, and the wait
/// before each: a server still reading the project answers empty (#202).
const COLD_RETRIES: usize = 3;
const COLD_WAIT: std::time::Duration = std::time::Duration::from_millis(800);

/// The functions that call `sym`, each with whether it is a test (the analyzer's `isTest`, or
/// the language's naming conventions).
async fn incoming_calls(
    session: &mut LspSession,
    root: &Path,
    language: &str,
    sym: &Symbol,
) -> Vec<(Symbol, bool)> {
    let abs = root.join(&sym.file);
    let Ok(uri) = Url::from_file_path(&abs).map(|u| u.to_string()) else {
        return Vec::new();
    };
    let position = serde_json::json!({ "line": sym.line.saturating_sub(1), "character": sym.col.saturating_sub(1) });
    let items = session
        .query(
            &abs,
            "textDocument/prepareCallHierarchy",
            serde_json::json!({ "textDocument": { "uri": uri }, "position": position }),
        )
        .await
        .unwrap_or(serde_json::Value::Null);
    let Some(item) = items.as_array().and_then(|a| a.first()).cloned() else {
        return Vec::new();
    };
    let incoming = session
        .query(
            &abs,
            "callHierarchy/incomingCalls",
            serde_json::json!({ "item": item }),
        )
        .await
        .unwrap_or(serde_json::Value::Null);
    let mut out = Vec::new();
    for edge in incoming.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let Some(from) = edge.get("from") else {
            continue;
        };
        let mut name = from
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        let file = rel(root, from.get("uri").and_then(|u| u.as_str()).unwrap_or(""));
        // Module-level code (a test file's top-level `it(...)` calls) is reported with the
        // file as its name: keep it checkout-relative.
        if name.starts_with('/') {
            name = rel(root, &name);
        }
        let sel = from.get("selectionRange").and_then(|r| r.get("start"));
        let line = sel
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0) as u32
            + 1;
        let col = sel
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0) as u32
            + 1;
        if file.starts_with('/') || name.is_empty() {
            continue; // outside the checkout
        }
        let flagged = edge
            .get("isTest")
            .and_then(|t| t.as_bool())
            .unwrap_or(false);
        let mut is_test = flagged || looks_like_test(language, &name, &file);
        // Beyond names: an attribute, a registration macro, a `TestCase` class (#201).
        if !is_test
            && let Ok(text) = std::fs::read_to_string(root.join(&file))
            && let Some(test_name) = test_marker(language, &text, line, &name)
        {
            is_test = true;
            name = test_name;
        }
        out.push((
            Symbol {
                name,
                file,
                line,
                col,
            },
            is_test,
        ));
    }
    out
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
        assert_eq!(
            test_command("go", &tools, &[t("TestA"), t("pkg.TestB")])
                .unwrap()
                .join(" "),
            "go test ./... -run ^(TestA|TestB)$"
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
        };
        assert_eq!(report.full_suite_reason(), None);
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
}
