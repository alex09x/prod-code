/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

/// What bundling did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ParameterObject {
    pub symbol: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    /// The type that was generated, as it will be written.
    pub struct_text: String,
    pub was: String,
    pub now: String,
    /// How many call sites were rewritten.
    pub call_sites: usize,
    /// One line per import a call site in another module needed.
    pub imports: Vec<String>,
    /// How many uses of the bundled parameters the body had.
    pub body_uses: usize,
    pub rewritten: Vec<(String, String)>,
    /// References the rule did not match, named rather than guessed at.
    pub unmatched: Vec<String>,
    /// Files that call the function by name where the analyzer reported no reference: checked
    /// with the rewritten files, not rewritten (#294).
    pub unreported: Vec<String>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
    /// The language the declaration is written in, as the report's code block names it.
    #[serde(skip)]
    pub language: &'static str,
}

impl ParameterObject {
    /// The report: the new type, what the declaration became, and whether it compiles.
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: ({})\n- now: ({})\n- {} call site(s) rewritten, {} use(s) in \
             the body\n\n```{}\n{}\n```\n\n",
            self.symbol,
            self.file,
            self.was,
            self.now,
            self.call_sites,
            self.body_uses,
            self.language,
            self.struct_text
        );
        let mut body = String::new();
        let mut changed_lines = 0usize;
        for (path, new_text) in &self.rewritten {
            let old_text = crate::refactor::text_before_apply(Path::new(path));
            let rel = display(&self.root, Path::new(path));
            let diff = similar::TextDiff::from_lines(&old_text, new_text);
            changed_lines += diff
                .iter_all_changes()
                .filter(|c| c.tag() != similar::ChangeTag::Equal)
                .count();
            body.push_str(
                &diff
                    .unified_diff()
                    .context_radius(2)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        out.push_str(&format!(
            "{} changed line(s) in {} file(s)\n\n",
            changed_lines,
            self.rewritten.len()
        ));
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }
        if !self.imports.is_empty() {
            out.push_str("\nimports:\n");
            for note in &self.imports {
                out.push_str(&format!("  {note}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            let usual = if self.language == "rust" {
                "a function pointer, a macro, or a call already changed"
            } else {
                "the function passed as a value, or a call already changed"
            };
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) that are not a call with the arity this \
                 declaration has — {usual}):\n",
                self.unmatched.len()
            ));
            for r in &self.unmatched {
                out.push_str(&format!("  {r}\n"));
            }
        }
        out.push_str(&crate::extract_parameter::unreported_note(
            &self.unreported,
            &self.file,
        ));
        if self.diagnostics.is_empty() && self.language == "javascript" {
            out.push_str(
                "\nthe analyzer accepts the result: 0 errors (in JavaScript that is the syntax; \
                 nothing checks a call's arguments against the object)\n",
            );
        } else if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        if self.applied {
            out.push_str(&format!(
                "\n[applied to {} file(s)]\n",
                self.rewritten.len()
            ));
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make these edits\n");
        }
        out
    }
}

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    C,
    Cpp,
    Swift,
    Java,
}

impl Language {
    pub fn of(path: &Path) -> Option<Language> {
        match crate::lang::language_id_for_path(path) {
            "rust" => Some(Language::Rust),
            "typescript" | "typescriptreact" => Some(Language::TypeScript),
            "javascript" | "javascriptreact" => Some(Language::JavaScript),
            "python" => Some(Language::Python),
            "go" => Some(Language::Go),
            "c" => Some(Language::C),
            "cpp" => Some(Language::Cpp),
            "swift" => Some(Language::Swift),
            "java" => Some(Language::Java),
            _ => None,
        }
    }

    pub fn fence(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::TypeScript => "typescript",
            Language::JavaScript => "javascript",
            Language::Python => "python",
            Language::Go => "go",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Swift => "swift",
            Language::Java => "java",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::TypeScript => "TypeScript",
            Language::JavaScript => "JavaScript",
            Language::Python => "Python",
            Language::Go => "Go",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Swift => "Swift",
            Language::Java => "Java",
        }
    }
}

pub fn default_binding(file: &Path, name: &str) -> String {
    match Language::of(file) {
        Some(
            Language::TypeScript
            | Language::JavaScript
            | Language::Go
            | Language::Swift
            | Language::Java,
        ) => lower_camel(name),
        _ => crate::fixture::snake_case(name),
    }
}

fn lower_camel(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut upper = chars.iter().take_while(|c| c.is_uppercase()).count();
    if upper > 1 && upper < chars.len() {
        upper -= 1;
    }
    chars[..upper]
        .iter()
        .flat_map(|c| c.to_lowercase())
        .chain(chars[upper..].iter().copied())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Variadic,
    Keywords,
    Marker,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub raw: String,
    pub name: String,
    pub name_at: usize,
    pub ty: Option<String>,
    pub default: Option<String>,
    pub optional: bool,
    pub shares_type: bool,
    pub kind: Kind,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Option<String>,
    pub default: Option<String>,
    pub optional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spelled {
    Inert,
    Primitive,
    MayDrop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Nothing,
    Reads,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct CDeclaration {
    pub path: PathBuf,
    pub text: String,
    pub name_at: usize,
    pub open: usize,
    pub close: usize,
    pub params: Vec<Param>,
    pub body: bool,
}
