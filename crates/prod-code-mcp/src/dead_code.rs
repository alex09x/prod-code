//! Dead-code scan (roadmap 8.6): functions, methods and types nobody references, found by
//! asking the analyzer for the references of every symbol in the checkout.
//!
//! Only a successful answer with an empty list of references makes a symbol dead. A request
//! that failed, a `null` (the analyzer found no symbol there to search for) or an answer of
//! another shape leaves the symbol unverified, never dead, and so out of reach of pruning (#435).

use crate::session::LspSession;
use anyhow::{Result, anyhow};
use serde::Serialize;
use std::net::SocketAddr;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeadItem {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// Exported / public: nothing in this checkout uses it, but something outside might.
    pub exported: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeadCodeReport {
    pub language: String,
    pub files_scanned: usize,
    pub symbols_checked: usize,
    pub dead: Vec<DeadItem>,
    /// Methods without direct references: they may still be reached through a trait,
    /// interface or protocol, which reference search does not follow.
    pub methods_unreferenced: Vec<DeadItem>,
    /// Exported symbols without references that were not listed (`include_exported` off).
    pub exported_unreferenced: usize,
    pub truncated: bool,
    /// Files and symbols the analyzer could not answer for: nothing is known about them, so
    /// none is listed as dead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unverified: Vec<Unverified>,
}

/// A file whose symbols, or a symbol whose references, the analyzer did not establish.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Unverified {
    pub file: String,
    /// The symbol; `None` when the file's symbols could not be listed at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 1-based line and column of the symbol's name; 0 for a whole file.
    pub line: u32,
    pub col: u32,
    pub reason: String,
}

impl DeadCodeReport {
    /// Whether every source file and every symbol was judged: nothing cut by the file limit,
    /// nothing the analyzer failed to answer.
    pub fn complete(&self) -> bool {
        !self.truncated && self.unverified.is_empty()
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "dead code scan ({}): {} file(s), {} symbol(s) checked, {} unreferenced\n",
            self.language,
            self.files_scanned,
            self.symbols_checked,
            self.dead.len()
        );
        for item in &self.dead {
            out.push_str(&format!(
                "  • {} {}{}  {}:{}:{}\n",
                item.kind,
                item.name,
                if item.exported { " (exported)" } else { "" },
                item.file,
                item.line,
                item.col
            ));
        }
        if !self.methods_unreferenced.is_empty() {
            out.push_str(&format!(
                "methods without direct references ({}; may be reached through a trait / interface):\n",
                self.methods_unreferenced.len()
            ));
            for item in &self.methods_unreferenced {
                out.push_str(&format!(
                    "  • {}{}  {}:{}:{}\n",
                    item.name,
                    if item.exported { " (exported)" } else { "" },
                    item.file,
                    item.line,
                    item.col
                ));
            }
        }
        if self.exported_unreferenced > 0 {
            out.push_str(&format!(
                "{} exported symbol(s) are unreferenced inside the checkout (list them with --include-exported)\n",
                self.exported_unreferenced
            ));
        }
        if self.truncated {
            out.push_str("scan truncated by the file limit\n");
        }
        if !self.unverified.is_empty() {
            out.push_str(&format!(
                "{} could not be checked (the analyzer failed or gave no usable answer), so none is listed as dead:\n",
                self.unverified.len()
            ));
            for u in &self.unverified {
                out.push_str(&match &u.name {
                    Some(name) => format!(
                        "  • {name}  {}:{}:{}: {}\n",
                        u.file, u.line, u.col, u.reason
                    ),
                    None => format!("  • {}: {}\n", u.file, u.reason),
                });
            }
        }
        out
    }
}

/// How many references a `textDocument/references` answer lists, or why it says nothing about
/// them. Only a list, empty or not, is an answer; `null` is what an analyzer gives when it
/// found no symbol at the position to search from.
pub fn reference_count(answer: Result<serde_json::Value>) -> std::result::Result<usize, String> {
    match answer {
        Ok(serde_json::Value::Array(found)) => Ok(found.len()),
        Ok(serde_json::Value::Null) => {
            Err("textDocument/references answered null: no symbol was searched for".to_string())
        }
        Ok(other) => Err(crate::impact::unreadable("textDocument/references", &other)),
        Err(e) => Err(format!("{e:#}")),
    }
}

fn kind_name(kind: u64) -> Option<&'static str> {
    Some(match kind {
        5 => "class",
        6 => "method",
        10 => "enum",
        11 => "interface",
        12 => "function",
        23 => "struct",
        _ => return None,
    })
}

/// Whether a source line declares an exported / public item in `language`.
pub fn is_exported(language: &str, name: &str, line: &str) -> bool {
    let t = line.trim_start();
    match language {
        "rust" => t.starts_with("pub ") || t.starts_with("pub("),
        "go" => name.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
        "typescript" => t.starts_with("export "),
        "swift" => t.starts_with("public ") || t.starts_with("open "),
        "cpp" => false,
        "python" => !name.starts_with('_'),
        _ => false,
    }
}

fn is_test_path(language: &str, rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
        || match language {
            "go" => lower.ends_with("_test.go"),
            "python" => lower
                .rsplit('/')
                .next()
                .is_some_and(|b| b.starts_with("test_") || b.ends_with("_test.py")),
            "typescript" => lower.contains(".test.") || lower.contains(".spec."),
            "swift" => lower.ends_with("tests.swift"),
            _ => false,
        }
}

fn extensions(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &["rs"],
        "go" => &["go"],
        "python" => &["py"],
        "typescript" => &["ts", "tsx", "js", "jsx", "mts", "cts"],
        "cpp" => &["c", "cc", "cpp", "cxx", "h", "hh", "hpp"],
        "swift" => &["swift"],
        _ => &[],
    }
}

/// Whether a symbol's container is a trait implementation block (`impl Shape for Circle`):
/// its methods are reached through the trait, which reference search does not follow.
fn in_trait_impl(container: &str) -> bool {
    container
        .split(" > ")
        .any(|c| c.starts_with("impl ") && c.contains(" for "))
}

fn collect(symbols: &[serde_json::Value], out: &mut Vec<(String, String, u32, u32)>) {
    for sym in symbols {
        // Items inside a test module are tests, whatever they are called.
        let container = sym
            .get("containerName")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        // rust-analyzer labels modules by name ("tests"), other servers by kind and name.
        if container.split(" > ").any(|c| {
            let c = c.trim_start_matches("mod ");
            c == "tests" || c == "test" || c.ends_with("tests")
        }) {
            continue;
        }
        let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
        let name = sym
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(kind_name) = kind_name(kind)
            && !name.is_empty()
            && let Some(range) = sym
                .get("selectionRange")
                .or_else(|| sym.get("range"))
                .or_else(|| sym.get("location").and_then(|l| l.get("range")))
            && let Some(start) = range.get("start")
        {
            let line = start.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as u32 + 1;
            let col = start.get("character").and_then(|c| c.as_u64()).unwrap_or(0) as u32 + 1;
            let kind_name = if kind_name == "method" && in_trait_impl(&container) {
                "trait-method"
            } else {
                kind_name
            };
            out.push((name, kind_name.to_string(), line, col));
        }
        if let Some(children) = sym.get("children").and_then(|c| c.as_array()) {
            collect(children, out);
        }
    }
}

/// Scans the checkout at `root` (placed on `remote`) for unreferenced symbols.
pub async fn find_dead_code(
    remote: SocketAddr,
    root: &Path,
    include_exported: bool,
    max_files: usize,
) -> Result<DeadCodeReport> {
    let language = crate::sync::expected_engine(root)
        .ok_or_else(|| anyhow!("no project manifest at {}", root.display()))?
        .to_string();
    let exts = extensions(&language);
    let mut files: Vec<String> = crate::sync::scan_workspace_files(root, None)?
        .into_iter()
        .map(|d| d.relative_path)
        .filter(|p| {
            Path::new(p)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e))
        })
        .filter(|p| !is_test_path(&language, p))
        .collect();
    files.sort();
    let truncated = files.len() > max_files;
    files.truncate(max_files);

    let mut session = LspSession::open(remote, root, None).await?;
    let mut report = DeadCodeReport {
        language: language.clone(),
        files_scanned: 0,
        symbols_checked: 0,
        dead: Vec::new(),
        methods_unreferenced: Vec::new(),
        exported_unreferenced: 0,
        truncated,
        unverified: Vec::new(),
    };
    let whole_file = |file: &str, reason: String| Unverified {
        file: file.to_string(),
        name: None,
        line: 0,
        col: 0,
        reason,
    };
    for rel in &files {
        let abs = root.join(rel);
        let text = match std::fs::read_to_string(&abs) {
            Ok(text) => text,
            Err(e) => {
                report
                    .unverified
                    .push(whole_file(rel, format!("it cannot be read: {e}")));
                continue;
            }
        };
        let lines: Vec<&str> = text.lines().collect();
        let uri = session.uri_for(&abs)?;
        let symbols = match session
            .query(
                &abs,
                "textDocument/documentSymbol",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await
        {
            Ok(serde_json::Value::Array(symbols)) => symbols,
            // No symbols to judge, and so none listed dead.
            Ok(serde_json::Value::Null) => Vec::new(),
            Ok(other) => {
                let reason = crate::impact::unreadable("textDocument/documentSymbol", &other);
                report.unverified.push(whole_file(rel, reason));
                continue;
            }
            Err(e) => {
                report.unverified.push(whole_file(rel, format!("{e:#}")));
                continue;
            }
        };
        report.files_scanned += 1;
        let mut candidates = Vec::new();
        collect(&symbols, &mut candidates);
        for (name, kind, line, col) in candidates {
            let bare = name.split('(').next().unwrap_or(&name);
            if matches!(
                bare,
                "main" | "init" | "new" | "default" | "drop" | "fmt" | "eq" | "hash" | "clone"
            ) || bare.starts_with("test")
                || bare.starts_with("Test")
                || bare.starts_with("__")
            {
                continue;
            }
            let source_line = lines.get(line as usize - 1).copied().unwrap_or("");
            let exported = is_exported(&language, bare, source_line);
            report.symbols_checked += 1;
            let refs = session
                .query(
                    &abs,
                    "textDocument/references",
                    serde_json::json!({
                        "textDocument": { "uri": session.uri_for(&abs)? },
                        "position": { "line": line - 1, "character": col - 1 },
                        "context": { "includeDeclaration": false }
                    }),
                )
                .await;
            let count = match reference_count(refs) {
                Ok(count) => count,
                Err(reason) => {
                    report.unverified.push(Unverified {
                        file: rel.clone(),
                        name: Some(name),
                        line,
                        col,
                        reason,
                    });
                    continue;
                }
            };
            if count == 0 {
                let item = DeadItem {
                    name,
                    kind: kind.clone(),
                    file: rel.clone(),
                    line,
                    col,
                    exported,
                };
                // Rust inherent methods are checked like functions; trait-impl methods and
                // methods in languages with interfaces/protocols go to the "maybe" bucket.
                if kind == "trait-method" || (kind == "method" && language != "rust") {
                    report.methods_unreferenced.push(item);
                } else if exported && !include_exported {
                    report.exported_unreferenced += 1;
                } else {
                    report.dead.push(item);
                }
            }
        }
    }
    session.close().await;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_detection() {
        assert!(is_exported("rust", "f", "pub fn f() {}"));
        assert!(!is_exported("rust", "f", "fn f() {}"));
        assert!(is_exported("go", "Compute", "func Compute() {}"));
        assert!(!is_exported("go", "compute", "func compute() {}"));
        assert!(is_exported("typescript", "f", "export function f() {}"));
        assert!(is_exported("swift", "f", "public func f() {}"));
        assert!(is_test_path("go", "pkg/a_test.go"));
        assert!(in_trait_impl("impl Shape for Circle"));
        assert!(!in_trait_impl("impl Circle"));
        assert!(!is_test_path("rust", "src/lib.rs"));
    }

    #[test]
    fn only_a_list_counts_references() {
        assert_eq!(reference_count(Ok(serde_json::json!([]))), Ok(0));
        assert_eq!(reference_count(Ok(serde_json::json!([{}, {}]))), Ok(2));
        assert!(
            reference_count(Ok(serde_json::Value::Null))
                .unwrap_err()
                .contains("null")
        );
        assert!(
            reference_count(Ok(serde_json::json!({ "uri": "x" })))
                .unwrap_err()
                .contains("cannot read")
        );
        assert!(
            reference_count(Err(anyhow!("textDocument/references failed: boom")))
                .unwrap_err()
                .contains("boom")
        );
    }
}
