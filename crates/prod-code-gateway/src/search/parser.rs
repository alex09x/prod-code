/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::patterns::declaration_on;
use super::types::{Declaration, MAX_DECLS_PER_FILE, MAX_DOC_LINES};

/// The language a file name belongs to, or `None` when it is not source we index.
pub(crate) fn language_of(name: &str) -> Option<&'static str> {
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

/// Is this line a doc comment for whatever follows it?
pub(crate) fn doc_line(line: &str, language: &str) -> Option<String> {
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
    // A `#[cfg(test)]` item or a `mod tests` block is test material until its braces close,
    // the source fixtures tests keep in multi-line strings included; production code after
    // it is not (#1457). Python's `__main__` block runs to the end of the file.
    let mut scope = TestScope::default();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let in_test_module = scope.line(trimmed);
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

/// Which lines of a file are test material, read line by line (#1457).
#[derive(Default)]
struct TestScope {
    /// A `#[cfg(test)]` was read and its item has not started yet.
    pending: bool,
    /// The brace depth before the test item; it is test material until it is back there.
    until: Option<i32>,
    /// Everything from here on (Python's `if __name__ == "__main__":`).
    rest: bool,
    depth: i32,
}

impl TestScope {
    /// Whether `trimmed`, the next line of the file, is test material.
    fn line(&mut self, trimmed: &str) -> bool {
        if trimmed == "if __name__ == \"__main__\":" {
            self.rest = true;
        }
        let code = trimmed.split("//").next().unwrap_or(trimmed);
        let attribute = trimmed.starts_with("#[cfg(test)]");
        let starts_module =
            trimmed.starts_with("mod tests") || trimmed.starts_with("pub mod tests");
        if attribute {
            self.pending = true;
        }
        // `#[cfg(test)] mod tests {` puts the item on the attribute's own line.
        let item_here = trimmed
            .strip_prefix("#[cfg(test)]")
            .map_or(item_starts(trimmed), |rest| item_starts(rest.trim()));
        let before = self.depth;
        self.depth += code.matches('{').count() as i32 - code.matches('}').count() as i32;
        if self.until.is_none() && (starts_module || self.pending && item_here) {
            self.pending = false;
            // A one-line item (`use …;`, `fn f() {}`) is test material on its own line only.
            if self.depth > before {
                self.until = Some(before);
            }
            return true;
        }
        let inside = self.until.is_some();
        if self.until.is_some_and(|depth| self.depth <= depth) {
            self.until = None;
        }
        self.rest || inside || attribute || self.pending
    }
}

/// Whether a line after an attribute starts the item it applies to: not another attribute, a
/// comment or a blank line.
fn item_starts(trimmed: &str) -> bool {
    !trimmed.is_empty() && !trimmed.starts_with("#[") && !trimmed.starts_with("//")
}
