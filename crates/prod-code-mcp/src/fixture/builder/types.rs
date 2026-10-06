/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

/// The method the verification canary calls, which no builder has.
pub(crate) const CANARY_METHOD: &str = "__prod_code_missing_method";
/// Methods of the builder itself; a field of the same name would need a second one.
pub(crate) const RESERVED_METHODS: [&str; 2] = ["new", "build"];
/// Attributes that never change a struct's fields.
pub(crate) const INERT_ATTRIBUTES: [&str; 12] = [
    "derive",
    "doc",
    "repr",
    "allow",
    "warn",
    "deny",
    "forbid",
    "expect",
    "must_use",
    "non_exhaustive",
    "deprecated",
    "automatically_derived",
];
/// Strict and reserved keywords, which a builder name cannot be.
pub(crate) const KEYWORDS: [&str; 52] = [
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

/// What to generate a builder for.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuilderRequest<'a> {
    /// The struct's name, as the workspace symbol index knows it.
    pub symbol: &'a str,
    /// A file inside the project, which also picks one of several declarations of that name.
    pub hint: Option<&'a Path>,
    /// The builder's name; `<Type>Builder` when absent. The error type is `<builder>Error`.
    pub builder_name: Option<&'a str>,
    /// Check the builder with the analyzer in the scope it would be inserted into.
    pub verify: bool,
}

/// One field of the struct, and the setter the builder has for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuilderField {
    /// The field's name as declared, raw identifiers included (`r#type`).
    pub name: String,
    /// The field's type as spelled in the declaration, whitespace collapsed.
    pub ty: String,
    /// The setter's name: the field's own.
    pub setter: String,
}

/// The builder for one declaration, computed from the file's text alone.
#[derive(Debug, Clone)]
pub struct BuilderPlan {
    /// The struct's name as declared.
    pub type_name: String,
    pub builder_name: String,
    /// The error `build` returns for a field that was never set.
    pub error_name: String,
    /// The struct's visibility as declared, given to the builder, its error and their methods.
    pub visibility: String,
    pub fields: Vec<BuilderField>,
    /// The complete generated source, indented like the declaration.
    pub code: String,
    /// The whole file as it would be with the builder inserted.
    pub file_text: String,
    /// The declaration's first line (its first attribute) and last line, 1-based.
    pub declaration_lines: (u32, u32),
    /// The line the builder would be inserted after, 1-based: the declaration's last.
    pub insert_after_line: u32,
    /// Where `code` sits in `file_text`, 1-based and inclusive.
    pub code_lines: (u32, u32),
    /// Things worth knowing that did not stop the generation.
    pub notes: Vec<String>,
    pub(crate) indent: String,
    pub(crate) newline: &'static str,
}

/// What the analyzer made of the generated builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// The analyzer checked the builder where it would be inserted and reported no error; it
    /// also reported the deliberate error placed next to it, so it was really looking.
    Clean,
    /// The analyzer checked the builder and reported these errors.
    Rejected { diagnostics: Vec<String> },
    /// The builder was not checked, for this reason. Not a pass.
    Unverified { reason: String },
}

/// A generated builder, where it would go, and whether the analyzer accepted it there.
#[derive(Debug, Clone)]
pub struct BuilderPreview {
    /// The file that declares the struct, relative to the workspace root.
    pub file: String,
    pub plan: BuilderPlan,
    pub verification: Verification,
}

impl BuilderPreview {
    /// True only when the analyzer checked the builder in place and found nothing.
    pub fn verified(&self) -> bool {
        self.verification == Verification::Clean
    }

    /// The analyzer's errors, empty unless the builder was rejected.
    pub fn diagnostics(&self) -> &[String] {
        match &self.verification {
            Verification::Rejected { diagnostics } => diagnostics,
            _ => &[],
        }
    }

    pub fn render(&self) -> String {
        let plan = &self.plan;
        let mut out = format!(
            "builder `{}` for `{}` ({} field{}), declared in {}:{}-{}; it would be inserted after line {}\n\n```rust\n{}\n```\n",
            plan.builder_name,
            plan.type_name,
            plan.fields.len(),
            if plan.fields.len() == 1 { "" } else { "s" },
            self.file,
            plan.declaration_lines.0,
            plan.declaration_lines.1,
            plan.insert_after_line,
            plan.code,
        );
        match &self.verification {
            Verification::Clean => out.push_str(
                "\nverified: the analyzer checked it in the scope of the declaration: 0 errors\n",
            ),
            Verification::Rejected { diagnostics } => {
                out.push_str("\nrejected: the analyzer reports errors in it:\n");
                for d in diagnostics {
                    out.push_str(&format!("  {d}\n"));
                }
            }
            Verification::Unverified { reason } => {
                out.push_str(&format!("\nnot verified: {reason}\n"));
            }
        }
        for note in &plan.notes {
            out.push_str(&format!("note: {note}\n"));
        }
        out.push_str("nothing was written\n");
        out
    }
}

/// A declaration in the analyzer's document symbols: its lines, 1-based, and the fields it
/// lists, when it lists any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutlineNode {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) fields: Option<Vec<String>>,
}
