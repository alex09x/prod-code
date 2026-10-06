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

use super::case::{display, to_pascal_case, to_snake_case};

pub const COPY: &[&str] = &[
    "bool", "char", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128",
    "isize", "f32", "f64",
];

/// What the encapsulation did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EncapsulatedField {
    /// The struct the field belongs to.
    pub owner: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub field: String,
    pub ty: String,
    /// Whether the getter returns the value (a `Copy` type) or a reference to it.
    pub by_value: bool,
    pub reads: usize,
    pub writes: usize,
    /// Reads that go on to call a method on the field or index it: if one of those mutates it,
    /// a getter that returns `&T` does not compile, and only the compiler says so.
    pub chained_reads: usize,
    /// References inside the declaring file, which stay direct accesses.
    pub left_in_file: usize,
    /// Uses that cannot become a method call, with the reason.
    pub blocked: Vec<String>,
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl EncapsulatedField {
    /// The report: the accessors, the rewritten accesses, and what could not be rewritten.
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = if self.file.ends_with(".ts")
            || self.file.ends_with(".tsx")
            || self.file.ends_with(".js")
            || self.file.ends_with(".jsx")
        {
            let pascal = to_pascal_case(&self.field);
            let mut s = format!(
                "`{}.{}` ({})\n\n- the field becomes private\n- getter: `get{}(): {}`\n",
                self.owner, self.field, self.file, pascal, self.ty
            );
            if self.writes > 0 {
                s.push_str(&format!(
                    "- setter: `set{}({}: {}): void`\n",
                    pascal, self.field, self.ty
                ));
            }
            s
        } else if self.file.ends_with(".py") {
            let snake = to_snake_case(&self.field);
            let mut s = format!(
                "`{}.{}` ({})\n\n- the field becomes private\n- getter: `def get_{}(self) -> {}`\n",
                self.owner, self.field, self.file, snake, self.ty
            );
            if self.writes > 0 {
                s.push_str(&format!(
                    "- setter: `def set_{}(self, {}: {}) -> None`\n",
                    snake, self.field, self.ty
                ));
            }
            s
        } else if self.file.ends_with(".cpp")
            || self.file.ends_with(".cc")
            || self.file.ends_with(".cxx")
            || self.file.ends_with(".h")
            || self.file.ends_with(".hpp")
        {
            let snake = to_snake_case(&self.field);
            let ret_ty = if self.by_value {
                self.ty.clone()
            } else {
                format!("const {}&", self.ty)
            };
            let param_ty = if self.by_value {
                self.ty.clone()
            } else {
                format!("const {}&", self.ty)
            };
            let mut s = format!(
                "`{}::{}` ({})\n\n- the field becomes private\n- getter: `{} get_{}() const`\n",
                self.owner, self.field, self.file, ret_ty, snake
            );
            if self.writes > 0 {
                s.push_str(&format!(
                    "- setter: `void set_{}({} {})`\n",
                    snake, param_ty, self.field
                ));
            }
            s
        } else if self.file.ends_with(".swift") {
            let pascal = to_pascal_case(&self.field);
            let mut s = format!(
                "`{}.{}` ({})\n\n- the field becomes private\n- getter: `func get{}() -> {}`\n",
                self.owner, self.field, self.file, pascal, self.ty
            );
            if self.writes > 0 {
                s.push_str(&format!("- setter: `func set{}(_: {})`\n", pascal, self.ty));
            }
            s
        } else if self.file.ends_with(".go") {
            let pascal = to_pascal_case(&self.field);
            let mut s = format!(
                "`{}.{}` ({})\n\n- the field becomes private\n- getter: `func (s *{}) {}() {}`\n",
                self.owner, self.field, self.file, self.owner, pascal, self.ty
            );
            if self.writes > 0 {
                s.push_str(&format!(
                    "- setter: `func (s *{}) Set{}({} {})`\n",
                    self.owner, pascal, self.field, self.ty
                ));
            }
            s
        } else {
            let getter = if self.by_value {
                self.ty.clone()
            } else {
                format!("&{}", self.ty)
            };
            let mut s = format!(
                "`{}.{}` ({})\n\n- the field becomes private\n- getter: `fn {}(&self) -> {getter}`\n",
                self.owner, self.field, self.file, self.field
            );
            if self.writes > 0 {
                s.push_str(&format!(
                    "- setter: `fn set_{}(&mut self, {}: {})`\n",
                    self.field, self.field, self.ty
                ));
            }
            s
        };
        out.push_str(&format!(
            "- {} read(s) and {} write(s) outside {} rewritten; {} reference(s) inside it left \
             as they are, because a private field is still visible there\n\n",
            self.reads, self.writes, self.file, self.left_in_file
        ));
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
        if !self.blocked.is_empty() {
            out.push_str(&format!(
                "\n{} use(s) outside the declaring file cannot become a method call, and a \
                 private field would not compile there:\n",
                self.blocked.len()
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) this could not read as a field access):\n",
                self.unmatched.len()
            ));
            for r in &self.unmatched {
                out.push_str(&format!("  {r}\n"));
            }
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        if self.chained_reads > 0 && !self.by_value {
            out.push_str(&format!(
                "\n{} read(s) call a method on the field or index it. The getter returns a \
                 shared reference, so one that mutates the field no longer compiles — and the \
                 analyzer does not check borrows. Ask for `verify: \"compile\"` to be sure.\n",
                self.chained_reads
            ));
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

/// A named field's declaration, read from the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    pub name: String,
    /// Where the name starts.
    pub name_at: usize,
    /// The visibility as written, with its trailing space: `pub `, `pub(crate) `, or empty.
    pub vis: String,
    /// Where the visibility starts; it runs up to `name_at`.
    pub vis_at: usize,
    pub ty: String,
}

/// How a reference to the field is used, read from the text around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// `x.f`, possibly continuing with `.method()` or `[i]` (`chained`).
    Read { chained: bool },
    /// `x.f = rhs`, with the right-hand side's span.
    Write { rhs: (usize, usize) },
    /// Anything this cannot turn into a method call, and why.
    Blocked(&'static str),
    /// Not a field access at all: a method with the same name.
    NotAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    TypeScript,
    JavaScript,
    Python,
    Cpp,
    Swift,
    Go,
}

impl Language {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension().and_then(|s| s.to_str()) {
            Some("ts" | "tsx") => Some(Self::TypeScript),
            Some("js" | "jsx") => Some(Self::JavaScript),
            Some("py") => Some(Self::Python),
            Some("cpp" | "cc" | "cxx" | "h" | "hpp") => Some(Self::Cpp),
            Some("swift") => Some(Self::Swift),
            Some("go") => Some(Self::Go),
            _ => None,
        }
    }

    pub fn matches_extension(&self, path: &Path) -> bool {
        Self::from_path(path) == Some(*self)
    }
}
