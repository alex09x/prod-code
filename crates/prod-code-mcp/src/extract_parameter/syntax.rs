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

use super::enclosing::with_parameter;
use super::hover::{
    clangd_type, go_type, python_type, swift_type, type_from_hover, typescript_type,
};

/// The language of the file an extraction happens in.
///
/// The steps are the same everywhere: find the function, add a parameter, read it in the body,
/// pass the expression at every call. What differs is how a declaration is found, how a
/// parameter is spelled, and how each language server names a type in a hover, so those are
/// the parts chosen by the file's language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    C,
    Cpp,
    Swift,
}

impl Syntax {
    /// The syntax of `path`, from the language it is opened with; `None` for a language this
    /// cannot extract a parameter in.
    pub fn of(path: &Path) -> Option<Self> {
        match crate::lang::language_id_for_path(path) {
            "rust" => Some(Self::Rust),
            "typescript" | "typescriptreact" => Some(Self::TypeScript),
            "javascript" | "javascriptreact" => Some(Self::JavaScript),
            "python" => Some(Self::Python),
            "go" => Some(Self::Go),
            "c" => Some(Self::C),
            "cpp" => Some(Self::Cpp),
            "swift" => Some(Self::Swift),
            _ => None,
        }
    }

    /// Whether this is C or C++. clangd serves both alike, and a `.h` header is opened as C
    /// even in a C++ project, so nothing here may depend on telling them apart.
    pub fn is_c_family(self) -> bool {
        matches!(self, Self::C | Self::Cpp)
    }

    /// The new parameter as the declaration spells it, or `None` when this language needs a
    /// type and there is none. JavaScript has no annotations, and a Python parameter without
    /// one is still a parameter, so those two never need one. A C declarator binds `*` and `&`
    /// to the name, so `const char *` gives `const char *label`, as the language is written.
    pub fn parameter(self, name: &str, ty: Option<&str>) -> Option<String> {
        match (self, ty) {
            (Self::JavaScript, _) | (Self::Python, None) => Some(name.to_string()),
            (Self::Go, Some(ty)) => Some(format!("{name} {ty}")),
            (Self::C | Self::Cpp, Some(ty)) if ty.ends_with(['*', '&']) => {
                Some(format!("{ty}{name}"))
            }
            (Self::C | Self::Cpp, Some(ty)) => Some(format!("{ty} {name}")),
            (_, Some(ty)) => Some(format!("{name}: {ty}")),
            (_, None) => None,
        }
    }

    /// The type in a hover answer from this language's server, when it is one this can read.
    pub fn type_from_hover(self, hover: &str) -> Option<String> {
        match self {
            Self::Rust => type_from_hover(hover),
            Self::JavaScript => None,
            Self::TypeScript => typescript_type(hover),
            Self::Python => python_type(hover),
            Self::Go => go_type(hover),
            Self::C | Self::Cpp => clangd_type(hover),
            Self::Swift => swift_type(hover),
        }
    }

    /// The name a call site spells for the function the outline calls `callee`. gopls names a
    /// method `(*Store).Limit`, clangd an out-of-line one `Store::limit`, and sourcekit-lsp
    /// every function with its argument labels, `render(text:)`; a caller writes only `Limit`,
    /// `limit` and `render`.
    pub fn bare_name(self, callee: &str) -> &str {
        match self {
            Self::C | Self::Cpp => callee.rsplit("::").next().unwrap_or(callee),
            Self::Swift => callee.split('(').next().unwrap_or(callee),
            _ => callee.rsplit('.').next().unwrap_or(callee),
        }
    }

    /// The parameter list with `param` added at the end. In C, `f(void)` is how a function
    /// that takes nothing is declared, and the `void` gives way to the new parameter rather
    /// than preceding it.
    pub fn with_parameter(self, list: &str, param: &str) -> String {
        if self.is_c_family() && list.trim() == "void" {
            return param.to_string();
        }
        with_parameter(list, param)
    }

    /// The type of a literal expression. No language server answers a hover on `80` or `"x"`,
    /// yet a literal is the most common thing to extract, and its type is not in doubt.
    pub fn literal_type(self, expression: &str) -> Option<&'static str> {
        let e = expression.trim();
        let integer = !e.is_empty()
            && e.strip_prefix('-')
                .unwrap_or(e)
                .chars()
                .all(|c| c.is_ascii_digit() || c == '_')
            && e.chars().any(|c| c.is_ascii_digit());
        let float = !integer && e.contains('.') && e.replace('_', "").parse::<f64>().is_ok();
        let quoted = |q: char| e.len() >= 2 && e.starts_with(q) && e.ends_with(q);
        let string = match self {
            // In C a single quote is a character, and Swift has no other string quote.
            Self::C | Self::Cpp | Self::Swift => quoted('"'),
            _ => quoted('"') || (self != Self::Go && quoted('\'')) || quoted('`'),
        };
        let boolean = match self {
            Self::Python => e == "True" || e == "False",
            _ => e == "true" || e == "false",
        };
        // `0.5f` is a C float; without the suffix the same digits are a double.
        let suffixed_float = e
            .strip_suffix(['f', 'F'])
            .is_some_and(|m| m.contains('.') && m.parse::<f64>().is_ok());
        match self {
            Self::C | Self::Cpp if integer => Some("int"),
            Self::C | Self::Cpp if float => Some("double"),
            Self::C | Self::Cpp if suffixed_float => Some("float"),
            Self::C | Self::Cpp if string => Some("const char *"),
            Self::C | Self::Cpp if quoted('\'') => Some("char"),
            Self::C | Self::Cpp if boolean => Some("bool"),
            Self::Swift if integer => Some("Int"),
            Self::Swift if float => Some("Double"),
            Self::Swift if string => Some("String"),
            Self::Swift if boolean => Some("Bool"),
            Self::Rust | Self::JavaScript => None,
            Self::TypeScript if integer || float => Some("number"),
            Self::TypeScript if string => Some("string"),
            Self::TypeScript if boolean => Some("boolean"),
            Self::Python if integer => Some("int"),
            Self::Python if float => Some("float"),
            Self::Python if string => Some("str"),
            Self::Python if boolean => Some("bool"),
            Self::Go if integer => Some("int"),
            Self::Go if float => Some("float64"),
            Self::Go if string => Some("string"),
            Self::Go if quoted('\'') => Some("rune"),
            Self::Go if boolean => Some("bool"),
            _ => None,
        }
    }

    /// Whether the parameter list ends in one that takes whatever arguments are left: `...rest`
    /// in TypeScript and JavaScript, `...T` in Go, `*args`, a bare `*` or `**kwargs` in Python,
    /// `...` in C, a pack `Ts... xs` in C++, and an unlabeled `_ xs: Int...` in Swift. A
    /// parameter added after it would not receive the argument every call site passes, so
    /// the callers would change behaviour, which is exactly what this refactoring promises not
    /// to do. A labeled Swift variadic ends where the next label starts, so it is no obstacle.
    pub fn catch_all(self, list: &str) -> Option<String> {
        let params = crate::signature::split_params(list);
        match self {
            Self::Rust => None,
            Self::TypeScript | Self::JavaScript => {
                params.last().filter(|p| p.starts_with("...")).cloned()
            }
            Self::Go | Self::C | Self::Cpp => params.last().filter(|p| p.contains("...")).cloned(),
            Self::Python => params.into_iter().find(|p| p.starts_with('*')),
            Self::Swift => params
                .last()
                .filter(|p| p.ends_with("...") && !swift_labeled(p))
                .cloned(),
        }
    }

    /// Whether `line` imports a name rather than using it. The TypeScript server leaves imports
    /// out of `references`, basedpyright does not, and an import needs no argument.
    pub fn is_import(self, line: &str) -> bool {
        let line = line.trim_start();
        match self {
            Self::TypeScript | Self::JavaScript => {
                line.starts_with("import ")
                    || line.starts_with("import{")
                    || line.starts_with("export {")
                    || line.starts_with("export{")
            }
            Self::Python => line.starts_with("import ") || line.starts_with("from "),
            Self::Rust | Self::Go | Self::C | Self::Cpp | Self::Swift => false,
        }
    }
}

/// Whether a Swift parameter has an argument label, that is, whether its callers write
/// `name: value`. Only `_` as the first of its names makes the argument positional.
pub fn swift_labeled(param: &str) -> bool {
    param
        .split_once(':')
        .is_none_or(|(names, _)| names.split_whitespace().next() != Some("_"))
}
