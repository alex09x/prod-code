/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

use super::types::{Field, Language};

/// The type a hover gives a parameter: `number` from TypeScript's `(parameter) width: number`,
/// `int` from basedpyright's `(parameter) width: int`. `None` when the server does not know it,
/// which basedpyright says as `Unknown`.
pub fn hover_parameter_type(hover: &str, name: &str) -> Option<String> {
    let prefix = format!("(parameter) {name}");
    let rest = hover
        .lines()
        .find_map(|l| l.trim().strip_prefix(prefix.as_str()))?;
    let ty = rest
        .trim_start_matches('?')
        .trim_start()
        .strip_prefix(':')?
        .trim();
    (!ty.is_empty() && ty != "Unknown").then(|| ty.to_string())
}

pub(crate) async fn hover_type(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    at: usize,
    name: &str,
) -> Option<String> {
    let (line, col) = crate::signature::line_col_at(text, at)?;
    let uri = url::Url::from_file_path(file).ok()?.to_string();
    let res = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        }),
    )
    .await
    .ok()?;
    hover_parameter_type(res.pointer("/contents/value")?.as_str()?, name)
}

/// One level of indentation as the file writes it: its first indented line that is not the
/// inside of a block comment, or the language's usual one. Go is always a tab.
pub(crate) fn indent_unit(text: &str, language: Language) -> String {
    if language == Language::Go {
        return "\t".to_string();
    }
    if let Some(unit) = detected_indent(text) {
        return unit;
    }
    match language {
        Language::TypeScript | Language::JavaScript => "  ".to_string(),
        _ => "    ".to_string(),
    }
}

/// The indentation of the file's first indented line that is not the inside of a block comment,
/// or `None` when nothing in it is indented — a header of prototypes, typically.
pub(crate) fn detected_indent(text: &str) -> Option<String> {
    for line in text.lines() {
        let code = line.trim_start();
        if code.is_empty() || code.len() == line.len() || code.starts_with('*') {
            continue;
        }
        let ws = &line[..line.len() - code.len()];
        return Some(if ws.starts_with('\t') {
            "\t".to_string()
        } else {
            ws.to_string()
        });
    }
    None
}

/// The type that holds the bundled parameters in TypeScript, Python or Go, as it will be written.
///
/// TypeScript gets an `interface` (exported when the declaration it stands above is), with a
/// parameter that had no type and no hover typed `any`, which is what it was. Python gets a
/// `@dataclass` when every field has a type, since a dataclass field is an annotation; a
/// parameter the analyzer cannot type makes it a plain class with an `__init__`, rather than a
/// type being invented. Go gets a `struct` whose fields keep the parameters' names, so a field
/// is exported exactly when the parameter's name was capitalised, which it rarely is.
///
/// C and C++ get a `struct` whose members are declared the way the parameters were, with a C++
/// default as the member's initialiser. Swift gets a `struct` of `let` properties (public when
/// the function is), a defaulted one a `var`, so that its memberwise initialiser is the literal.
pub fn type_text(
    language: Language,
    name: &str,
    callee: &str,
    fields: &[Field],
    indent: &str,
    export: bool,
) -> String {
    let mut out = String::new();
    match language {
        Language::TypeScript => {
            out.push_str(&format!(
                "/** The parameters `{callee}` takes together. */\n"
            ));
            let export = if export { "export " } else { "" };
            out.push_str(&format!("{export}interface {name} {{\n"));
            for f in fields {
                let optional = if f.optional { "?" } else { "" };
                let ty = f.ty.as_deref().unwrap_or("any");
                out.push_str(&format!("{indent}{}{optional}: {ty};\n", f.name));
            }
            out.push_str("}\n");
        }
        Language::JavaScript => {
            // Only the report shows this: JavaScript has no type to declare, and a class written
            // just to name the shape would be one more thing every caller has to construct.
            out.push_str(&format!(
                "// {name}: the plain object `{callee}` takes; nothing is declared for it\n"
            ));
            let shape: Vec<String> = fields
                .iter()
                .map(|f| match &f.default {
                    Some(d) => format!("{} = {d}", f.name),
                    None => f.name.clone(),
                })
                .collect();
            out.push_str(&format!("{{ {} }}\n", shape.join(", ")));
        }
        Language::Python => {
            // A field without a default after one with a default is an error in a dataclass and
            // in an `__init__`, unless the fields are keyword-only — which every call site this
            // writes passes them as anyway.
            let mut seen_default = false;
            let keyword_only = fields.iter().any(|f| {
                seen_default |= f.default.is_some();
                seen_default && f.default.is_none()
            });
            let doc = format!("{indent}\"\"\"The parameters `{callee}` takes together.\"\"\"\n\n");
            if fields.iter().all(|f| f.ty.is_some()) {
                out.push_str(if keyword_only {
                    "@dataclass(kw_only=True)\n"
                } else {
                    "@dataclass\n"
                });
                out.push_str(&format!("class {name}:\n{doc}"));
                for f in fields {
                    let ty = f.ty.as_deref().unwrap_or_default();
                    match &f.default {
                        Some(d) => out.push_str(&format!("{indent}{}: {ty} = {d}\n", f.name)),
                        None => out.push_str(&format!("{indent}{}: {ty}\n", f.name)),
                    }
                }
            } else {
                out.push_str(&format!("class {name}:\n{doc}"));
                let mut params = vec!["self".to_string()];
                if keyword_only {
                    params.push("*".to_string());
                }
                for f in fields {
                    let mut p = f.name.clone();
                    if let Some(ty) = &f.ty {
                        p.push_str(&format!(": {ty}"));
                    }
                    if let Some(d) = &f.default {
                        p.push_str(if f.ty.is_some() { " = " } else { "=" });
                        p.push_str(d);
                    }
                    params.push(p);
                }
                out.push_str(&format!("{indent}def __init__({}):\n", params.join(", ")));
                for f in fields {
                    out.push_str(&format!("{indent}{indent}self.{0} = {0}\n", f.name));
                }
            }
        }
        Language::C | Language::Cpp => {
            // A C header may be read by a C89 compiler, where `//` is not a comment.
            if language == Language::C {
                out.push_str(&format!(
                    "/* The parameters `{callee}` takes together. */\n"
                ));
            } else {
                out.push_str(&format!("// The parameters `{callee}` takes together.\n"));
            }
            out.push_str(&format!("struct {name} {{\n"));
            for f in fields {
                let declaration = f.ty.as_deref().unwrap_or(&f.name);
                match &f.default {
                    Some(d) => out.push_str(&format!("{indent}{declaration} = {d};\n")),
                    None => out.push_str(&format!("{indent}{declaration};\n")),
                }
            }
            out.push_str("};\n");
        }
        Language::Swift => {
            out.push_str(&format!("/// The parameters `{callee}` takes together.\n"));
            // A public function cannot take an internal type.
            let public = if export { "public " } else { "" };
            out.push_str(&format!("{public}struct {name} {{\n"));
            for f in fields {
                let ty = f.ty.as_deref().unwrap_or("Any");
                // A `let` with a value is a constant the memberwise initialiser cannot set; a
                // `var` with one is a parameter of it with that default.
                match &f.default {
                    Some(d) => out.push_str(&format!("{indent}var {}: {ty} = {d}\n", f.name)),
                    None => out.push_str(&format!("{indent}let {}: {ty}\n", f.name)),
                }
            }
            out.push_str("}\n");
        }
        Language::Java => {
            out.push_str(&format!("// The parameters `{callee}` takes together.\n"));
            let public = if export { "public " } else { "" };
            let p_str = fields
                .iter()
                .map(|f| {
                    let ty = f.ty.as_deref().unwrap_or("Object");
                    format!("{ty} {}", f.name)
                })
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("{public}record {name}({p_str}) {{}}\n"));
        }
        _ => {
            out.push_str(&format!(
                "// {name} holds the parameters {callee} takes together.\n"
            ));
            out.push_str(&format!("type {name} struct {{\n"));
            // gofmt aligns the types of consecutive fields; writing them aligned keeps the file
            // as gofmt would leave it.
            let width = fields.iter().map(|f| f.name.len()).max().unwrap_or(0);
            for f in fields {
                let ty = f.ty.as_deref().unwrap_or("any");
                out.push_str(&format!("\t{:width$} {ty}\n", f.name));
            }
            out.push_str("}\n");
        }
    }
    out
}

/// How the bundled parameter is declared: `opts: Opts` in TypeScript, Python and Swift,
/// `opts Opts` in Go, `Opts opts` in C++, and `struct Opts opts` in C, where a struct's name is
/// only a type together with the keyword. JavaScript's is the bare name.
pub(crate) fn parameter_in(language: Language, binding: &str, name: &str) -> String {
    match language {
        Language::JavaScript => binding.to_string(),
        Language::Go => format!("{binding} {name}"),
        Language::Cpp | Language::Java => format!("{name} {binding}"),
        Language::C => format!("struct {name} {binding}"),
        _ => format!("{binding}: {name}"),
    }
}

/// A literal of the new type from (field, value) pairs: `{ a: x, b: y }` in TypeScript, whose
/// interfaces are structural and need no name, and in JavaScript, which has no type to name;
/// `Opts(a=x, b=y)` in Python; `Opts{a: x, b: y}`
/// in Go; `Opts(a: x, b: y)`, the memberwise initialiser, in Swift. C needs a compound literal,
/// `(struct Opts){.a = x, .b = y}`, since a braced list alone is not an expression there; C++
/// converts a braced list to the parameter's type, and its designators are C++20's — see
/// [`aggregate_text`] for the standards before it.
///
/// A TypeScript or JavaScript field called `__proto__` is written as a computed key: written
/// plainly, `__proto__: x` sets the object's prototype instead of giving it that field.
pub fn literal_text(language: Language, spelling: &str, pairs: &[(String, String)]) -> String {
    let join = |sep: &str| {
        pairs
            .iter()
            .map(|(f, v)| match language {
                Language::TypeScript | Language::JavaScript => format!("{}{sep}{v}", js_key(f)),
                _ => format!("{f}{sep}{v}"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let designated = || {
        pairs
            .iter()
            .map(|(f, v)| format!(".{f} = {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match language {
        Language::JavaScript if pairs.is_empty() => "{}".to_string(),
        Language::TypeScript | Language::JavaScript => format!("{{ {} }}", join(": ")),
        Language::Python => format!("{spelling}({})", join("=")),
        Language::Go => format!("{spelling}{{{}}}", join(": ")),
        Language::Rust => format!("{spelling} {{ {} }}", join(": ")),
        Language::C => format!("(struct {spelling}){{{}}}", designated()),
        Language::Cpp => format!("{{{}}}", designated()),
        Language::Swift => format!("{spelling}({})", join(": ")),
        Language::Java => {
            let values: Vec<&str> = pairs.iter().map(|(_, v)| v.as_str()).collect();
            format!("new {spelling}({})", values.join(", "))
        }
    }
}

/// The key an object literal gives the field `name`: the name itself, except `__proto__`, which
/// only a computed key makes an own property.
pub(crate) fn js_key(name: &str) -> String {
    if name == "__proto__" {
        "[\"__proto__\"]".to_string()
    } else {
        name.to_string()
    }
}

/// A C++ literal without designators, `{x, y}`, for a project that compiles to a standard before
/// C++20. The values are in field order, which is what an aggregate is initialised by; a field
/// the call left to its default can only be a trailing one, and it is left out.
pub fn aggregate_text(pairs: &[(String, String)]) -> String {
    let values: Vec<&str> = pairs.iter().map(|(_, v)| v.as_str()).collect();
    format!("{{{}}}", values.join(", "))
}
