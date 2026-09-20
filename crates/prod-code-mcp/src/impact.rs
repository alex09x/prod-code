//! Blast radius of a change (roadmap 8.1): the functions a diff touches, the callers that
//! reach them through the analyzer's call hierarchy, and the tests among those callers,
//! with the command that runs only the affected tests.

use crate::tools::execute_lsp_query;
use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
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
}

impl ImpactReport {
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "impact of {} ({} changed file(s), {} changed function(s), {} caller(s), {} test(s))\n",
            self.base,
            self.changed_files.len(),
            self.changed.len(),
            self.callers.len(),
            self.tests.len()
        ));
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

/// Whether a caller is a test by name or file conventions of `language`.
pub fn looks_like_test(language: &str, name: &str, file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
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
        let symbols = execute_lsp_query(
            remote,
            root,
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

    // Walk incoming calls breadth-first from every changed function.
    let mut seen: HashSet<(String, u32, u32)> = changed
        .iter()
        .map(|s| (s.file.clone(), s.line, s.col))
        .collect();
    let mut queue: VecDeque<(Symbol, usize)> = changed.iter().cloned().map(|s| (s, 0)).collect();
    let mut callers: BTreeSet<Symbol> = BTreeSet::new();
    let mut tests: BTreeSet<Symbol> = BTreeSet::new();
    while let Some((sym, level)) = queue.pop_front() {
        if level >= depth {
            continue;
        }
        let abs = root.join(&sym.file);
        let uri = match Url::from_file_path(&abs) {
            Ok(u) => u.to_string(),
            Err(_) => continue,
        };
        let position = serde_json::json!({ "line": sym.line.saturating_sub(1), "character": sym.col.saturating_sub(1) });
        let items = execute_lsp_query(
            remote,
            root,
            &abs,
            "textDocument/prepareCallHierarchy",
            serde_json::json!({ "textDocument": { "uri": uri }, "position": position }),
        )
        .await
        .unwrap_or(serde_json::Value::Null);
        let Some(item) = items.as_array().and_then(|a| a.first()).cloned() else {
            continue;
        };
        let incoming = execute_lsp_query(
            remote,
            root,
            &abs,
            "callHierarchy/incomingCalls",
            serde_json::json!({ "item": item }),
        )
        .await
        .unwrap_or(serde_json::Value::Null);
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
            if !seen.insert((file.clone(), line, col)) {
                continue;
            }
            let caller = Symbol {
                name,
                file,
                line,
                col,
            };
            let flagged = edge
                .get("isTest")
                .and_then(|t| t.as_bool())
                .unwrap_or(false);
            if flagged || looks_like_test(&language, &caller.name, &caller.file) {
                tests.insert(caller.clone());
            } else {
                callers.insert(caller.clone());
            }
            queue.push_back((caller, level + 1));
        }
    }

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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conventions_per_language() {
        assert!(looks_like_test("go", "TestAdd", "pkg/add_test.go"));
        assert!(!looks_like_test("go", "helper", "pkg/add_test.go"));
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
        assert!(test_command("cpp", &tools, &[t("x")]).is_none());
        let ts = test_command("typescript", &tools, &[t("tests/a.test.ts"), t("adds")]).unwrap();
        assert!(ts.ends_with(&[
            "tests/a.test.ts".to_string(),
            "-t".to_string(),
            "adds".to_string()
        ]));
        assert!(test_command("go", &tools, &[]).is_none());
    }
}
