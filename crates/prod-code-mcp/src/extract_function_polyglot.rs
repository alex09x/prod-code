/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Polyglot extract function refactoring across TypeScript, Python, Go, C++, Swift, and Rust (Roadmap Section 7.1.2).
//!
//! Extracts a code selection into a new function, automatically analyzing free variable captures
//! for input parameters and downstream mutations for return synthesis, replacing the selection
//! with a call, and discovering and replacing identical or structurally parameterized duplicates
//! across the file and workspace.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::extract_function::{Duplicate, Extracted};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
    C,
    Cpp,
    Swift,
    Java,
    Kotlin,
    Csharp,
    Zig,
}

impl Language {
    pub fn of(file: &Path) -> Option<Self> {
        let ext = file.extension().and_then(|e| e.to_str())?;
        match ext {
            "rs" => Some(Self::Rust),
            "py" => Some(Self::Python),
            "ts" | "tsx" => Some(Self::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "go" => Some(Self::Go),
            "c" | "h" => Some(Self::C),
            "cpp" | "cc" | "cxx" | "hpp" => Some(Self::Cpp),
            "swift" => Some(Self::Swift),
            "java" => Some(Self::Java),
            "kt" | "kts" => Some(Self::Kotlin),
            "cs" => Some(Self::Csharp),
            "zig" | "zon" => Some(Self::Zig),
            _ => None,
        }
    }
}
fn is_candidate_source_file(path: &Path, lang: Language) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    match lang {
        Language::TypeScript => matches!(ext, "ts" | "tsx" | "js" | "jsx"),
        Language::JavaScript => matches!(ext, "js" | "jsx" | "ts" | "tsx"),
        Language::Python => ext == "py",
        Language::Cpp | Language::C => matches!(ext, "cpp" | "cc" | "cxx" | "c" | "h" | "hpp" | "hxx"),
        Language::Swift => ext == "swift",
        Language::Go => ext == "go",
        Language::Rust => ext == "rs",
        Language::Java => ext == "java",
        Language::Kotlin => matches!(ext, "kt" | "kts"),
        Language::Csharp => ext == "cs",
        Language::Zig => matches!(ext, "zig" | "zon"),
    }
}

fn collect_workspace_sources(root: &Path, lang: Language) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.')
                || name_str == "target"
                || name_str == "node_modules"
                || name_str == "build"
                || name_str == ".build"
                || name_str == "dist"
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() && is_candidate_source_file(&path, lang) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Whether `text` holds `word` as an isolated identifier.
fn mentions(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        !text[..i].chars().next_back().is_some_and(is_ident)
            && !text[i + word.len()..].chars().next().is_some_and(is_ident)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolyTokenKind {
    Word,
    Number,
    Str,
    Char,
    Punct,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolyToken {
    pub kind: PolyTokenKind,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

pub fn tokenize_polyglot(text: &str, lang: Language) -> Vec<PolyToken> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = text[i..].chars().next().unwrap_or(' ');
        let len = c.len_utf8();
        if c.is_whitespace() {
            i += len;
        } else if lang == Language::Python && text[i..].starts_with('#') {
            i = text[i..].find('\n').map_or(text.len(), |n| i + n);
        } else if lang != Language::Python && text[i..].starts_with("//") {
            i = text[i..].find('\n').map_or(text.len(), |n| i + n);
        } else if lang != Language::Python && text[i..].starts_with("/*") {
            i = text[i..].find("*/").map_or(text.len(), |n| i + n + 2);
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'.')
            {
                if bytes[j] == b'.' && bytes.get(j + 1) == Some(&b'.') {
                    break;
                }
                j += 1;
            }
            out.push(PolyToken {
                kind: PolyTokenKind::Number,
                start: i,
                end: j,
                text: text[i..j].to_string(),
            });
            i = j;
        } else if c == '"'
            || c == '`'
            || (c == '\''
                && lang != Language::Rust
                && lang != Language::Cpp
                && lang != Language::C
                && lang != Language::Zig)
        {
            let quote = c;
            let mut j = i + 1;
            while j < bytes.len() && text[j..].chars().next().unwrap_or(' ') != quote {
                let clen = text[j..].chars().next().map_or(1, |ch| ch.len_utf8());
                if bytes[j] == b'\\' {
                    j += 1;
                    if j < bytes.len() {
                        let elen = text[j..].chars().next().map_or(1, |ch| ch.len_utf8());
                        j += elen;
                    }
                } else {
                    j += clen;
                }
            }
            let j = if j < bytes.len() { j + 1 } else { bytes.len() };
            out.push(PolyToken {
                kind: PolyTokenKind::Str,
                start: i,
                end: j,
                text: text[i..j].to_string(),
            });
            i = j;
        } else if c == '\'' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'\'' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let j = (j + 1).min(bytes.len());
            out.push(PolyToken {
                kind: PolyTokenKind::Char,
                start: i,
                end: j,
                text: text[i..j].to_string(),
            });
            i = j;
        } else if is_ident(c) && !c.is_ascii_digit() {
            let mut j = i;
            while let Some(ch) = text[j..].chars().next() {
                if !is_ident(ch) {
                    break;
                }
                j += ch.len_utf8();
            }
            out.push(PolyToken {
                kind: PolyTokenKind::Word,
                start: i,
                end: j,
                text: text[i..j].to_string(),
            });
            i = j;
        } else {
            let two = if i + 2 <= bytes.len() && c.is_ascii() && bytes[i + 1].is_ascii() {
                &text[i..i + 2]
            } else {
                ""
            };
            let punct_len = match two {
                "->" | "=>" | "::" | ":=" | "==" | "!=" | "<=" | ">=" | "&&" | "||" | "++"
                | "--" | "+=" | "-=" | "*=" | "/=" => 2,
                _ => len,
            };
            out.push(PolyToken {
                kind: PolyTokenKind::Punct,
                start: i,
                end: i + punct_len,
                text: text[i..i + punct_len].to_string(),
            });
            i += punct_len;
        }
    }
    out
}

pub fn is_keyword(word: &str, lang: Language) -> bool {
    match lang {
        Language::Python => matches!(
            word,
            "and" | "as" | "assert" | "async" | "await" | "break" | "case" | "class"
                | "continue" | "def" | "del" | "elif" | "else" | "except" | "finally"
                | "for" | "from" | "global" | "if" | "import" | "in" | "is" | "lambda"
                | "match" | "nonlocal" | "not" | "or" | "pass" | "raise" | "return"
                | "try" | "while" | "with" | "yield" | "True" | "False" | "None" | "self"
        ),
        Language::TypeScript | Language::JavaScript => matches!(
            word,
            "break" | "case" | "catch" | "class" | "const" | "continue" | "debugger"
                | "default" | "delete" | "do" | "else" | "export" | "extends" | "finally"
                | "for" | "function" | "if" | "import" | "in" | "instanceof" | "new"
                | "return" | "super" | "switch" | "this" | "throw" | "try" | "typeof"
                | "var" | "void" | "while" | "with" | "yield" | "let" | "static" | "enum"
                | "await" | "async" | "from" | "as" | "true" | "false" | "null"
                | "undefined" | "number" | "string" | "boolean" | "any" | "never"
                | "unknown" | "type" | "interface" | "implements" | "declare" | "readonly"
        ),
        Language::Go => matches!(
            word,
            "break" | "default" | "func" | "interface" | "select" | "case" | "defer"
                | "go" | "map" | "struct" | "chan" | "else" | "goto" | "package" | "switch"
                | "const" | "fallthrough" | "if" | "range" | "type" | "continue" | "for"
                | "import" | "return" | "var" | "true" | "false" | "nil" | "int" | "int8"
                | "int16" | "int32" | "int64" | "uint" | "uint8" | "uint16" | "uint32"
                | "uint64" | "string" | "float32" | "float64" | "bool" | "byte" | "rune"
                | "error" | "any"
        ),
        Language::Cpp | Language::C => matches!(
            word,
            "auto" | "break" | "case" | "char" | "const" | "continue" | "default" | "do"
                | "double" | "else" | "enum" | "extern" | "float" | "for" | "goto" | "if"
                | "int" | "long" | "register" | "return" | "short" | "signed" | "sizeof"
                | "static" | "struct" | "switch" | "typedef" | "union" | "unsigned"
                | "void" | "volatile" | "while" | "class" | "namespace" | "new" | "delete"
                | "this" | "public" | "private" | "protected" | "virtual" | "override"
                | "template" | "typename" | "using" | "true" | "false" | "nullptr" | "std"
                | "bool" | "size_t" | "constexpr" | "inline"
        ),
        Language::Swift => matches!(
            word,
            "associatedtype" | "class" | "deinit" | "enum" | "extension" | "fileprivate"
                | "func" | "import" | "init" | "inout" | "internal" | "let" | "open"
                | "operator" | "private" | "protocol" | "public" | "rethrows" | "static"
                | "struct" | "subscript" | "typealias" | "var" | "break" | "case"
                | "continue" | "default" | "defer" | "do" | "else" | "fallthrough" | "for"
                | "guard" | "if" | "in" | "repeat" | "return" | "switch" | "where"
                | "while" | "as" | "Any" | "catch" | "false" | "is" | "nil" | "super"
                | "self" | "Self" | "throw" | "throws" | "true" | "try" | "Int" | "Double"
                | "Float" | "String" | "Bool"
        ),
        Language::Rust => matches!(
            word,
            "as" | "break" | "const" | "continue" | "crate" | "else" | "enum" | "extern"
                | "false" | "fn" | "for" | "if" | "impl" | "in" | "let" | "loop" | "match"
                | "mod" | "move" | "mut" | "pub" | "ref" | "return" | "self" | "Self"
                | "static" | "struct" | "super" | "trait" | "true" | "type" | "unsafe"
                | "use" | "where" | "while" | "async" | "await" | "dyn"
        ),
        Language::Java => matches!(
            word,
            "abstract" | "assert" | "boolean" | "break" | "byte" | "case" | "catch"
                | "char" | "class" | "const" | "continue" | "default" | "do" | "double"
                | "else" | "enum" | "extends" | "final" | "finally" | "float" | "for"
                | "goto" | "if" | "implements" | "import" | "instanceof" | "int" | "interface"
                | "long" | "native" | "new" | "package" | "private" | "protected" | "public"
                | "return" | "short" | "static" | "strictfp" | "super" | "switch" | "synchronized"
                | "this" | "throw" | "throws" | "transient" | "try" | "void" | "volatile"
                | "while" | "record" | "var" | "yield" | "sealed" | "permits" | "non-sealed"
                | "true" | "false" | "null"
        ),
        Language::Kotlin => matches!(
            word,
            "as" | "break" | "class" | "continue" | "do" | "else" | "false" | "for"
                | "fun" | "if" | "in" | "interface" | "is" | "null" | "object" | "package"
                | "return" | "super" | "this" | "throw" | "true" | "try" | "typealias"
                | "typeof" | "val" | "var" | "when" | "while" | "by" | "catch" | "constructor"
                | "delegate" | "dynamic" | "field" | "file" | "finally" | "get" | "import"
                | "init" | "param" | "property" | "receiver" | "set" | "setparam" | "where"
                | "actual" | "abstract" | "annotation" | "companion" | "const" | "crossinline"
                | "data" | "enum" | "expect" | "external" | "final" | "infix" | "inline"
                | "inner" | "internal" | "lateinit" | "noinline" | "open" | "operator"
                | "out" | "override" | "private" | "protected" | "public" | "reified"
                | "sealed" | "suspend" | "tailrec" | "vararg" | "value"
        ),
        Language::Csharp => matches!(
            word,
            "abstract" | "as" | "base" | "bool" | "break" | "byte" | "case" | "catch"
                | "char" | "checked" | "class" | "const" | "continue" | "decimal" | "default"
                | "delegate" | "do" | "double" | "else" | "enum" | "event" | "explicit"
                | "extern" | "false" | "finally" | "fixed" | "float" | "for" | "foreach"
                | "goto" | "if" | "implicit" | "in" | "int" | "interface" | "internal"
                | "is" | "lock" | "long" | "namespace" | "new" | "null" | "object"
                | "operator" | "out" | "override" | "params" | "private" | "protected"
                | "public" | "readonly" | "record" | "ref" | "return" | "sbyte" | "sealed"
                | "short" | "sizeof" | "stackalloc" | "static" | "string" | "struct"
                | "switch" | "this" | "throw" | "true" | "try" | "typeof" | "uint"
                | "ulong" | "unchecked" | "unsafe" | "ushort" | "using" | "virtual"
                | "void" | "volatile" | "while" | "var" | "async" | "await" | "yield"
        ),
        Language::Zig => matches!(
            word,
            "addrspace" | "align" | "allowzero" | "and" | "anyframe" | "anytype" | "asm" | "async"
                | "await" | "break" | "callconv" | "catch" | "comptime" | "const" | "continue"
                | "defer" | "else" | "enum" | "errdefer" | "error" | "export" | "extern"
                | "fn" | "for" | "if" | "inline" | "noalias" | "noinline" | "nosuspend"
                | "opaque" | "or" | "orelse" | "packed" | "pub" | "resume" | "return"
                | "linksection" | "struct" | "suspend" | "switch" | "test" | "threadlocal"
                | "try" | "union" | "unreachable" | "usingnamespace" | "var" | "volatile"
                | "while" | "true" | "false" | "null" | "undefined"
                | "u8" | "u16" | "u32" | "u64" | "u128" | "usize"
                | "i8" | "i16" | "i32" | "i64" | "i128" | "isize"
                | "f16" | "f32" | "f64" | "f80" | "f128" | "c_int" | "c_uint" | "c_long"
                | "c_ulong" | "c_char" | "bool" | "void" | "noreturn" | "type" | "anyerror"
        ),
    }
}

pub fn is_builtin_or_global(word: &str, lang: Language) -> bool {
    match lang {
        Language::Python => matches!(
            word,
            "print" | "len" | "range" | "sum" | "min" | "max" | "abs" | "round" | "int"
                | "float" | "str" | "bool" | "list" | "dict" | "set" | "tuple" | "open"
                | "isinstance" | "issubclass" | "type" | "zip" | "map" | "filter"
                | "enumerate" | "any" | "all" | "sorted" | "reversed" | "id" | "repr"
                | "iter" | "next"
        ),
        Language::TypeScript | Language::JavaScript => matches!(
            word,
            "console" | "Math" | "JSON" | "Object" | "Array" | "String" | "Number"
                | "Boolean" | "Date" | "RegExp" | "Error" | "Promise" | "Map" | "Set"
                | "WeakMap" | "WeakSet" | "Symbol" | "parseInt" | "parseFloat"
                | "isNaN" | "isFinite" | "encodeURI" | "decodeURI"
        ),
        Language::Go => matches!(
            word,
            "fmt" | "make" | "append" | "copy" | "delete" | "len" | "cap" | "panic"
                | "recover" | "close" | "new" | "real" | "imag" | "complex" | "print"
                | "println"
        ),
        Language::Cpp | Language::C => matches!(
            word,
            "std" | "printf" | "scanf" | "cout" | "cin" | "cerr" | "endl" | "malloc"
                | "free" | "sizeof" | "memcpy" | "memset" | "vector" | "string"
                | "map" | "unordered_map" | "set" | "shared_ptr" | "unique_ptr"
        ),
        Language::Swift => matches!(
            word,
            "print" | "min" | "max" | "abs" | "Array" | "Dictionary" | "Set" | "String"
                | "Int" | "Double" | "Float" | "Bool" | "fatalError" | "precondition"
        ),
        Language::Rust => matches!(
            word,
            "println" | "print" | "eprintln" | "eprint" | "format" | "vec" | "panic"
                | "Some" | "None" | "Ok" | "Err" | "Box" | "Vec" | "String" | "Option"
                | "Result"
        ),
        Language::Java => matches!(
            word,
            "System" | "String" | "Object" | "Integer" | "Long" | "Double" | "Float"
                | "Boolean" | "Character" | "Byte" | "Short" | "Math" | "Arrays" | "Collections"
                | "List" | "Map" | "Set" | "Optional" | "Objects" | "StringBuilder" | "StringBuffer"
                | "Exception" | "RuntimeException" | "Throwable" | "Thread" | "Runnable"
        ),
        Language::Kotlin => matches!(
            word,
            "println" | "print" | "require" | "check" | "error" | "assert" | "TODO"
                | "run" | "let" | "also" | "apply" | "with" | "takeIf" | "takeUnless"
                | "repeat" | "lazy" | "emptyList" | "listOf" | "mutableListOf" | "emptySet"
                | "setOf" | "mutableSetOf" | "emptyMap" | "mapOf" | "mutableMapOf"
                | "String" | "Int" | "Long" | "Double" | "Float" | "Boolean" | "Byte"
                | "Short" | "Char" | "Any" | "Unit" | "Nothing" | "Array" | "ByteArray"
                | "IntArray" | "LongArray" | "CharArray" | "BooleanArray"
        ),
        Language::Csharp => matches!(
            word,
            "Console" | "String" | "Object" | "Int32" | "Int64" | "Double" | "Single"
                | "Boolean" | "Char" | "Byte" | "Int16" | "Math" | "Array" | "List" | "Dictionary"
                | "HashSet" | "StringBuilder" | "Exception" | "Task" | "ValueTask" | "Action"
                | "Func" | "Predicate" | "Nullable" | "Span" | "ReadOnlySpan" | "Memory"
        ),
        Language::Zig => matches!(
            word,
            "std" | "builtin" | "root" | "assert"
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedParam {
    pub name: String,
    pub ty: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputKind {
    Expression(String),
    EndsWithReturn,
    SingleVar { name: String, is_new: bool },
    MultipleVars(Vec<String>),
    Void,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolyOccurrence {
    pub start: usize,
    pub end: usize,
    pub differs: Vec<(usize, String)>,
}

fn is_balanced(text: &str) -> bool {
    let mut stack = Vec::new();
    let mut in_str = None;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(quote) = in_str {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == quote {
                in_str = None;
            }
        } else {
            match c {
                '"' | '\'' | '`' => in_str = Some(c),
                '(' | '[' | '{' => stack.push(c),
                ')' if stack.pop() != Some('(') => return false,
                ']' if stack.pop() != Some('[') => return false,
                '}' if stack.pop() != Some('{') => return false,
                _ => {}
            }
        }
        i += 1;
    }
    stack.is_empty() && in_str.is_none()
}

fn has_complete_expression_boundaries(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].trim_end();
    let left = before.chars().next_back();
    let left_ok = left.is_none_or(|c| matches!(c, '(' | '[' | '{' | ',' | ':' | '=' | ';'))
        || before.ends_with("return")
        || before.ends_with("=>");
    let after = text[end..].trim_start();
    let right = after.chars().next();
    let right_ok = right.is_none_or(|c| matches!(c, ')' | ']' | '}' | ',' | ';' | ':'));
    left_ok && right_ok
}

fn find_enclosing_scope(
    text: &str,
    lang: Language,
    start: usize,
    _end: usize,
) -> (usize, usize, Option<String>, Option<String>, bool, String) {
    let mut enclosing_ret: Option<String> = None;
    let before = &text[..start];
    let mut scope_start = 0;
    let mut scope_end = text.len();
    let mut enclosing_fn = None;
    let mut is_method = false;
    let mut method_indent = String::new();

    match lang {
        Language::Python => {
            let sel_line = text[..start].rfind('\n').map_or(0, |i| i + 1);
            let sel_indent = text[sel_line..start]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .count();

            for (pos, _) in before.rmatch_indices("def ") {
                let line_s = before[..pos].rfind('\n').map_or(0, |i| i + 1);
                let line_indent = before[line_s..pos]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .count();
                if line_indent < sel_indent || sel_indent == 0 {
                    let head = &before[pos + 4..];
                    if let Some(open_p) = head.find('(') {
                        let fn_name = head[..open_p].trim().to_string();
                        enclosing_fn = Some(fn_name);
                        scope_start = line_s;
                        method_indent = " ".repeat(line_indent);
                        if line_indent > 0 {
                            is_method = true;
                        }
                    }
                    break;
                }
            }
            if scope_start > 0 {
                let def_indent = method_indent.len();
                let rest = &text[start..];
                let mut cur = start;
                for l in rest.lines() {
                    if l.trim().is_empty() || l.trim_start().starts_with('#') {
                        cur += l.len() + 1;
                        continue;
                    }
                    let cur_indent = l.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                    if cur_indent <= def_indent && cur > start {
                        scope_end = cur;
                        break;
                    }
                    cur += l.len() + 1;
                }
            }
        }
        _ => {
            let mut cur = start;
            while let Some(b_open) = text[..cur].rfind('{') {
                if let Some(b_close) = crate::parameter_object::matching_bracket(text, b_open)
                    .filter(|&bc| b_open < start && start < bc)
                {
                    scope_start = text[..b_open].rfind('\n').map_or(0, |i| i + 1);
                    scope_end = b_close + 1;
                    let head = text[scope_start..b_open].trim();
                    if let Some((first_p, last_p)) = head
                        .rfind(')')
                        .and_then(|lp| head[..lp].rfind('(').map(|fp| (fp, lp)))
                    {
                        let before_p = head[..first_p].trim();
                        let fn_name = before_p
                            .split_whitespace()
                            .last()
                            .unwrap_or("")
                            .to_string();
                        let is_control = matches!(
                            fn_name.as_str(),
                            "if" | "for" | "while" | "switch" | "catch" | "synchronized"
                                | "with" | "lock" | "using" | "try" | "else" | "do"
                        );
                        if !fn_name.is_empty() && !is_control {
                            enclosing_fn = Some(fn_name);

                            let after_p = head[last_p + 1..].trim();
                            if after_p.starts_with("->") {
                                let clean = after_p.trim_start_matches("->").trim();
                                let ret_type = clean.split_whitespace().next().unwrap_or("").trim_end_matches('{').trim();
                                if !ret_type.is_empty() {
                                    enclosing_ret = Some(ret_type.to_string());
                                }
                            } else if after_p.starts_with(':') {
                                let clean = after_p.trim_start_matches(':').trim();
                                let ret_type = clean.split_whitespace().next().unwrap_or("").trim_end_matches('{').trim();
                                if !ret_type.is_empty() {
                                    enclosing_ret = Some(ret_type.to_string());
                                }
                            } else if !after_p.is_empty() && !after_p.starts_with('{') {
                                let ret_type = after_p.split_whitespace().next().unwrap_or("").trim_end_matches('{').trim();
                                if !ret_type.is_empty() {
                                    enclosing_ret = Some(ret_type.to_string());
                                }
                            } else if lang == Language::Java || lang == Language::Cpp || lang == Language::C || lang == Language::Csharp {
                                let parts: Vec<&str> = before_p.split_whitespace().collect();
                                if parts.len() >= 2 {
                                    let ty = parts[parts.len() - 2];
                                    if ty != "export" && ty != "static" && ty != "inline" && ty != "virtual" && ty != "fun" && ty != "def" {
                                        enclosing_ret = Some(ty.to_string());
                                    }
                                }
                            }
                            let indent_chars = text[scope_start..]
                                .chars()
                                .take_while(|c| *c == ' ' || *c == '\t')
                                .collect::<String>();
                            method_indent = indent_chars;
                            if !method_indent.is_empty() {
                                is_method = true;
                            }
                            break;
                        }
                    }
                }
                cur = b_open;
            }
        }
    }

    (scope_start, scope_end, enclosing_fn, enclosing_ret, is_method, method_indent)
}

fn extract_input_variables(
    selection: &str,
    scope_before: &str,
    lang: Language,
) -> Vec<ExtractedParam> {
    let tokens = tokenize_polyglot(selection, lang);
    let mut declared_in_selection = HashSet::new();

    for (i, t) in tokens.iter().enumerate() {
        if matches!(t.text.as_str(), "let" | "const" | "var" | "val") && i + 1 < tokens.len() {
            let next = &tokens[i + 1];
            if next.kind == PolyTokenKind::Word && !is_keyword(&next.text, lang) {
                declared_in_selection.insert(next.text.clone());
            }
        } else if t.text == ":=" && i > 0 {
            let prev = &tokens[i - 1];
            if prev.kind == PolyTokenKind::Word && !is_keyword(&prev.text, lang) {
                declared_in_selection.insert(prev.text.clone());
            }
        } else if t.text == "=" && i > 0 && i + 1 < tokens.len() {
            let prev = &tokens[i - 1];
            if prev.kind == PolyTokenKind::Word
                && !is_keyword(&prev.text, lang)
                && (i == 1 || tokens[i - 2].text == "\n" || tokens[i - 2].text == ";")
            {
                declared_in_selection.insert(prev.text.clone());
            }
        }
    }

    let mut inputs = Vec::new();
    let mut seen = HashSet::new();

    for (i, t) in tokens.iter().enumerate() {
        if t.kind != PolyTokenKind::Word {
            continue;
        }
        let word = &t.text;
        if is_keyword(word, lang) || is_builtin_or_global(word, lang) {
            continue;
        }
        if i > 0 && (tokens[i - 1].text == "." || tokens[i - 1].text == "->") {
            continue;
        }
        if declared_in_selection.contains(word) {
            continue;
        }
        if mentions(scope_before, word) && seen.insert(word.clone()) {
            let ty = infer_param_type(word, scope_before, lang);
            inputs.push(ExtractedParam {
                name: word.clone(),
                ty,
            });
        }
    }

    inputs
}

fn infer_param_type(name: &str, scope: &str, lang: Language) -> Option<String> {
    match lang {
        Language::Python | Language::JavaScript => None,
        Language::TypeScript => {
            let pat = format!("{name}:");
            if let Some(pos) = scope.find(&pat) {
                let rest = scope[pos + pat.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| {
                        is_ident(*c) || *c == '<' || *c == '>' || *c == '[' || *c == ']'
                    })
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            let pat_eq = format!("{name} =");
            if let Some(pos) = scope.find(&pat_eq) {
                let rest = scope[pos + pat_eq.len()..].trim_start();
                if rest.starts_with('"') || rest.starts_with('\'') || rest.starts_with('`') {
                    return Some("string".into());
                }
                if rest.starts_with(|c: char| c.is_ascii_digit()) {
                    return Some("number".into());
                }
                if rest.starts_with("true") || rest.starts_with("false") {
                    return Some("boolean".into());
                }
            }
            Some("any".into())
        }
        Language::Go => {
            let pat = format!("{name} ");
            if let Some(pos) = scope.find(&pat) {
                let rest = scope[pos + pat.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| is_ident(*c) || *c == '[' || *c == ']' || *c == '*')
                    .collect();
                if !ty.is_empty() && !is_keyword(&ty, lang) {
                    return Some(ty);
                }
            }
            let pat_walrus = format!("{name} :=");
            if let Some(pos) = scope.find(&pat_walrus) {
                let rest = scope[pos + pat_walrus.len()..].trim_start();
                if rest.starts_with('"') {
                    return Some("string".into());
                }
                if rest.starts_with(|c: char| c.is_ascii_digit()) {
                    if rest
                        .split(|c: char| !c.is_ascii_digit() && c != '.')
                        .next()
                        .unwrap_or("")
                        .contains('.')
                    {
                        return Some("float64".into());
                    }
                    return Some("int".into());
                }
                if rest.starts_with("true") || rest.starts_with("false") {
                    return Some("bool".into());
                }
            }
            Some("int".into())
        }
        Language::Cpp | Language::C => {
            let pat = format!(" {name}");
            if let Some(pos) = scope.find(&pat) {
                let before = scope[..pos].trim_end();
                let ty: String = before
                    .chars()
                    .rev()
                    .take_while(|c| is_ident(*c) || *c == '*' || *c == '&')
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                let ty_trimmed = ty.trim_matches(|c| c == '*' || c == '&');
                if is_ident(ty_trimmed.chars().next().unwrap_or(' '))
                    && (!is_keyword(ty_trimmed, lang)
                        || matches!(
                            ty_trimmed,
                            "int" | "double" | "float" | "bool" | "char" | "size_t" | "long"
                                | "short" | "unsigned" | "signed"
                        ))
                {
                    return Some(ty);
                }
            }
            Some("int".into())
        }
        Language::Swift => {
            let pat = format!("{name}:");
            if let Some(pos) = scope.find(&pat) {
                let rest = scope[pos + pat.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| is_ident(*c) || *c == '<' || *c == '>')
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            Some("Double".into())
        }
        Language::Rust => Some("usize".into()),
        Language::Java => {
            let pat = format!(" {name}");
            if let Some(pos) = scope.find(&pat) {
                let before = scope[..pos].trim_end();
                let ty: String = before
                    .chars()
                    .rev()
                    .take_while(|c| is_ident(*c) || *c == '<' || *c == '>' || *c == '[' || *c == ']')
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                if is_ident(ty.chars().next().unwrap_or(' '))
                    && (!is_keyword(&ty, lang)
                        || matches!(
                            ty.as_str(),
                            "int" | "double" | "float" | "boolean" | "char" | "long"
                                | "short" | "byte" | "String" | "Object"
                        ))
                {
                    return Some(ty);
                }
            }
            Some("Object".into())
        }
        Language::Csharp => {
            let pat = format!(" {name}");
            if let Some(pos) = scope.find(&pat) {
                let before = scope[..pos].trim_end();
                let ty: String = before
                    .chars()
                    .rev()
                    .take_while(|c| is_ident(*c) || *c == '<' || *c == '>' || *c == '[' || *c == ']' || *c == '?')
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                if is_ident(ty.chars().next().unwrap_or(' '))
                    && (!is_keyword(&ty, lang)
                        || matches!(
                            ty.as_str(),
                            "int" | "double" | "float" | "bool" | "char" | "long"
                                | "short" | "byte" | "string" | "object"
                        ))
                {
                    return Some(ty);
                }
            }
            Some("object".into())
        }
        Language::Kotlin => {
            let pat = format!("{name}:");
            if let Some(pos) = scope.find(&pat) {
                let rest = scope[pos + pat.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| is_ident(*c) || *c == '<' || *c == '>' || *c == '[' || *c == ']' || *c == '?')
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            let pat_space = format!("{name} : ");
            if let Some(pos) = scope.find(&pat_space) {
                let rest = scope[pos + pat_space.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| is_ident(*c) || *c == '<' || *c == '>' || *c == '[' || *c == ']' || *c == '?')
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            let pat_eq = format!("{name} =");
            if let Some(pos) = scope.find(&pat_eq) {
                let rest = scope[pos + pat_eq.len()..].trim_start();
                if rest.starts_with('"') {
                    return Some("String".into());
                }
                if rest.starts_with(|c: char| c.is_ascii_digit()) {
                    if rest.contains('.') {
                        return Some("Double".into());
                    }
                    return Some("Int".into());
                }
                if rest.starts_with("true") || rest.starts_with("false") {
                    return Some("Boolean".into());
                }
            }
            Some("Any".into())
        }
        Language::Zig => {
            let pat = format!("{name}:");
            if let Some(pos) = scope.find(&pat) {
                let rest = scope[pos + pat.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| is_ident(*c) || *c == '[' || *c == ']' || *c == '*' || *c == '?' || *c == '!')
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            let pat_space = format!("{name} : ");
            if let Some(pos) = scope.find(&pat_space) {
                let rest = scope[pos + pat_space.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| is_ident(*c) || *c == '[' || *c == ']' || *c == '*' || *c == '?' || *c == '!')
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            Some("anytype".into())
        }
    }
}

fn analyze_outputs(
    selection: &str,
    scope_after: &str,
    scope_before: &str,
    lang: Language,
) -> OutputKind {
    let trimmed = selection.trim();

    if trimmed.starts_with("return ") || trimmed.starts_with("return\n") {
        return OutputKind::EndsWithReturn;
    }

    let tokens = tokenize_polyglot(trimmed, lang);
    let has_semi = trimmed.contains(';');
    let has_stmt_kw = tokens.iter().any(|t| {
        matches!(
            t.text.as_str(),
            "let"
                | "const"
                | "var"
                | "val"
                | "def"
                | "func"
                | "fun"
                | "if"
                | "while"
                | "for"
                | "return"
                | "import"
        )
    });
    let has_assign = tokens
        .iter()
        .any(|t| matches!(t.text.as_str(), "=" | ":=" | "+=" | "-=" | "*=" | "/="));

    if !has_semi && !has_stmt_kw && !has_assign {
        return OutputKind::Expression(trimmed.to_string());
    }

    let mut assigned = Vec::new();
    let mut seen = HashSet::new();
    for (i, t) in tokens.iter().enumerate() {
        if matches!(t.text.as_str(), "let" | "const" | "var" | "val") && i + 1 < tokens.len() {
            let next = &tokens[i + 1];
            if next.kind == PolyTokenKind::Word
                && !is_keyword(&next.text, lang)
                && seen.insert(next.text.clone())
            {
                assigned.push(next.text.clone());
            }
        } else if matches!(
            t.text.as_str(),
            ":=" | "=" | "+=" | "-=" | "*=" | "/="
        ) && i > 0
        {
            let prev = &tokens[i - 1];
            if prev.kind == PolyTokenKind::Word
                && !is_keyword(&prev.text, lang)
                && seen.insert(prev.text.clone())
            {
                assigned.push(prev.text.clone());
            }
        }
    }

    let used_after: Vec<String> = assigned
        .into_iter()
        .filter(|v| mentions(scope_after, v))
        .collect();

    match used_after.len() {
        0 => OutputKind::Void,
        1 => {
            let name = used_after[0].clone();
            let is_new = !mentions(scope_before, &name);
            OutputKind::SingleVar { name, is_new }
        }
        _ => OutputKind::MultipleVars(used_after),
    }
}

fn reindent_body(body: &str, target_indent_spaces: usize, lang: Language) -> String {
    let lines: Vec<&str> = body.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').count())
        .min()
        .unwrap_or(0);

    let target_prefix = if lang == Language::Go || body.contains('\t') {
        let tabs = target_indent_spaces.div_ceil(4).max(1);
        "\t".repeat(tabs)
    } else {
        " ".repeat(target_indent_spaces)
    };

    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }
        let stripped = if line.len() >= min_indent {
            &line[min_indent..]
        } else {
            line.trim_start()
        };
        out.push_str(&target_prefix);
        out.push_str(stripped);
        if idx + 1 < lines.len() {
            out.push('\n');
        }
    }
    out
}

fn generate_call_replacement(
    name: &str,
    args: &[String],
    param_names: &[String],
    output: &OutputKind,
    lang: Language,
    is_method: bool,
    indent: &str,
) -> String {
    let args_str = match lang {
        Language::Swift => param_names
            .iter()
            .zip(args)
            .map(|(p, a)| format!("{p}: {a}"))
            .collect::<Vec<_>>()
            .join(", "),
        _ => args.join(", "),
    };

    let call = if is_method {
        match lang {
            Language::Python | Language::Swift => format!("self.{name}({args_str})"),
            Language::TypeScript | Language::JavaScript | Language::Java | Language::Csharp => format!("this.{name}({args_str})"),
            Language::Kotlin => format!("{name}({args_str})"),
            Language::Cpp | Language::C => format!("this->{name}({args_str})"),
            Language::Go => format!("r.{name}({args_str})"),
            Language::Rust | Language::Zig => format!("self.{name}({args_str})"),
        }
    } else {
        format!("{name}({args_str})")
    };

    let semi = match lang {
        Language::Python | Language::Swift | Language::Go | Language::Kotlin => "",
        _ => ";",
    };

    match output {
        OutputKind::Expression(_) => format!("{indent}{call}"),
        OutputKind::EndsWithReturn => format!("{indent}return {call}{semi}"),
        OutputKind::SingleVar {
            name: v,
            is_new,
        } => {
            if *is_new {
                match lang {
                    Language::Python => format!("{indent}{v} = {call}"),
                    Language::TypeScript | Language::JavaScript => {
                        format!("{indent}const {v} = {call};")
                    }
                    Language::Go => format!("{indent}{v} := {call}"),
                    Language::Cpp | Language::C => format!("{indent}auto {v} = {call};"),
                    Language::Swift => format!("{indent}let {v} = {call}"),
                    Language::Rust => format!("{indent}let {v} = {call};"),
                    Language::Java | Language::Csharp => format!("{indent}var {v} = {call};"),
                    Language::Kotlin => format!("{indent}val {v} = {call}"),
                    Language::Zig => format!("{indent}const {v} = {call};"),
                }
            } else {
                format!("{indent}{v} = {call}{semi}")
            }
        }
        OutputKind::MultipleVars(vars) => {
            let joined = vars.join(", ");
            match lang {
                Language::Python => format!("{indent}{joined} = {call}"),
                Language::TypeScript | Language::JavaScript => {
                    format!("{indent}const [{joined}] = {call};")
                }
                Language::Go => format!("{indent}{joined} := {call}"),
                Language::Cpp | Language::C => format!("{indent}auto [{joined}] = {call};"),
                Language::Swift => format!("{indent}let ({joined}) = {call}"),
                Language::Rust => format!("{indent}let ({joined}) = {call};"),
                Language::Java => format!("{indent}var res = {call};"),
                Language::Csharp => format!("{indent}var ({joined}) = {call};"),
                Language::Kotlin => format!("{indent}val ({joined}) = {call}"),
                Language::Zig => format!("{indent}const {joined} = {call};"),
            }
        }
        OutputKind::Void => format!("{indent}{call}{semi}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_function_code(
    name: &str,
    params: &[ExtractedParam],
    output: &OutputKind,
    selection_body: &str,
    lang: Language,
    is_method: bool,
    is_exported: bool,
    method_indent: &str,
    enclosing_ret: Option<&str>,
) -> String {
    let body = match output {
        OutputKind::Expression(expr) => {
            let semi = match lang {
                Language::Python | Language::Swift | Language::Go | Language::Kotlin => "",
                _ => ";",
            };
            format!("return {expr}{semi}")
        }
        OutputKind::EndsWithReturn => selection_body.trim().to_string(),
        OutputKind::SingleVar { name: v, .. } => {
            let semi = match lang {
                Language::Python | Language::Swift | Language::Go | Language::Kotlin => "",
                _ => ";",
            };
            let mut s = selection_body.trim_end().to_string();
            if !s.ends_with(&format!("return {v}")) && !s.ends_with(&format!("return {v};")) {
                s.push('\n');
                s.push_str(&format!("return {v}{semi}"));
            }
            s
        }
        OutputKind::MultipleVars(vars) => {
            let semi = match lang {
                Language::Python | Language::Swift | Language::Go | Language::Kotlin => "",
                _ => ";",
            };
            let mut s = selection_body.trim_end().to_string();
            let ret_val = match lang {
                Language::TypeScript | Language::JavaScript => format!("[{}]", vars.join(", ")),
                _ => vars.join(", "),
            };
            s.push('\n');
            s.push_str(&format!("return {ret_val}{semi}"));
            s
        }
        OutputKind::Void => selection_body.trim_end().to_string(),
    };

    let base_body_indent = if is_method {
        method_indent.len() + 4
    } else {
        4
    };
    let reindented = reindent_body(&body, base_body_indent, lang);

    match lang {
        Language::Python => {
            let mut p_list = Vec::new();
            if is_method {
                p_list.push("self".to_string());
            }
            p_list.extend(params.iter().map(|p| p.name.clone()));
            let p_str = p_list.join(", ");
            if is_method {
                format!("{method_indent}def {name}({p_str}):\n{reindented}\n")
            } else {
                format!("def {name}({p_str}):\n{reindented}\n")
            }
        }
        Language::TypeScript => {
            let p_str = params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty.as_deref().unwrap_or("any")))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("number");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => format!(": {ret_ty}"),
                OutputKind::Void => ": void".to_string(),
                _ => String::new(),
            };
            let export_prefix = if is_exported && !is_method {
                "export "
            } else {
                ""
            };
            if is_method {
                format!("{method_indent}private {name}({p_str}){ret_ann} {{\n{reindented}\n{method_indent}}}\n")
            } else {
                format!("{export_prefix}function {name}({p_str}){ret_ann} {{\n{reindented}\n}}\n")
            }
        }
        Language::JavaScript => {
            let p_str = params
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let export_prefix = if is_exported && !is_method {
                "export "
            } else {
                ""
            };
            if is_method {
                format!("{method_indent}private {name}({p_str}) {{\n{reindented}\n{method_indent}}}\n")
            } else {
                format!("{export_prefix}function {name}({p_str}) {{\n{reindented}\n}}\n")
            }
        }
        Language::Go => {
            let p_str = params
                .iter()
                .map(|p| format!("{} {}", p.name, p.ty.as_deref().unwrap_or("int")))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("int");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => format!(" {ret_ty}"),
                _ => String::new(),
            };
            format!("func {name}({p_str}){ret_ann} {{\n{reindented}\n}}\n")
        }
        Language::Cpp | Language::C => {
            let p_str = params
                .iter()
                .map(|p| format!("{} {}", p.ty.as_deref().unwrap_or("int"), p.name))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("int");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => ret_ty,
                _ => "void",
            };
            if is_method {
                format!("{method_indent}{ret_ann} {name}({p_str}) {{\n{reindented}\n{method_indent}}}\n")
            } else {
                format!("static {ret_ann} {name}({p_str}) {{\n{reindented}\n}}\n")
            }
        }
        Language::Swift => {
            let p_str = params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty.as_deref().unwrap_or("Int")))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("Int");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => format!(" -> {ret_ty}"),
                _ => String::new(),
            };
            if is_method {
                format!("{method_indent}private func {name}({p_str}){ret_ann} {{\n{reindented}\n{method_indent}}}\n")
            } else {
                format!("func {name}({p_str}){ret_ann} {{\n{reindented}\n}}\n")
            }
        }
        Language::Rust => {
            let p_str = params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty.as_deref().unwrap_or("usize")))
                .collect::<Vec<_>>()
                .join(", ");
            format!("fn {name}({p_str}) {{\n{reindented}\n}}\n")
        }
        Language::Java => {
            let p_str = params
                .iter()
                .map(|p| format!("{} {}", p.ty.as_deref().unwrap_or("Object"), p.name))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("void");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => ret_ty,
                _ => "void",
            };
            let vis = if is_exported { "public " } else { "private " };
            let has_this = selection_body.contains("this.") || selection_body.contains("this ");
            if is_method || has_this {
                format!("{method_indent}{vis}{ret_ann} {name}({p_str}) {{\n{reindented}\n{method_indent}}}\n")
            } else {
                format!("{method_indent}{vis}static {ret_ann} {name}({p_str}) {{\n{reindented}\n{method_indent}}}\n")
            }
        }
        Language::Csharp => {
            let p_str = params
                .iter()
                .map(|p| format!("{} {}", p.ty.as_deref().unwrap_or("object"), p.name))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("void");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => ret_ty,
                _ => "void",
            };
            let vis = if is_exported { "public " } else { "private " };
            let has_this = selection_body.contains("this.") || selection_body.contains("this ");
            if is_method || has_this {
                format!("{method_indent}{vis}{ret_ann} {name}({p_str})\n{method_indent}{{\n{reindented}\n{method_indent}}}\n")
            } else {
                format!("{method_indent}{vis}static {ret_ann} {name}({p_str})\n{method_indent}{{\n{reindented}\n{method_indent}}}\n")
            }
        }
        Language::Kotlin => {
            let p_str = params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty.as_deref().unwrap_or("Any")))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => {
                    if let Some(ret) = enclosing_ret {
                        format!(": {ret}")
                    } else {
                        String::new()
                    }
                }
                OutputKind::Void => String::new(),
                _ => String::new(),
            };
            let vis = if is_exported { "" } else { "private " };
            if is_method {
                format!("{method_indent}{vis}fun {name}({p_str}){ret_ann} {{\n{reindented}\n{method_indent}}}\n")
            } else {
                format!("{vis}fun {name}({p_str}){ret_ann} {{\n{reindented}\n}}\n")
            }
        }
        Language::Zig => {
            let p_str = params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty.as_deref().unwrap_or("anytype")))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => {
                    if let Some(ret) = enclosing_ret {
                        format!(" {ret}")
                    } else {
                        " anytype".to_string()
                    }
                }
                OutputKind::Void => " void".to_string(),
                _ => String::new(),
            };
            let vis = if is_exported { "pub " } else { "" };
            if is_method {
                format!("{method_indent}{vis}fn {name}({p_str}){ret_ann} {{\n{reindented}\n{method_indent}}}\n")
            } else {
                format!("{vis}fn {name}({p_str}){ret_ann} {{\n{reindented}\n}}\n")
            }
        }
    }
}

fn find_duplicates_in_text(
    selection: &str,
    text: &str,
    exclude: Option<(usize, usize)>,
    parameterize: bool,
    lang: Language,
) -> Vec<PolyOccurrence> {
    let wanted = tokenize_polyglot(selection, lang);
    let have = tokenize_polyglot(text, lang);
    if wanted.is_empty() || wanted.len() > have.len() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut k = 0;
    while k + wanted.len() <= have.len() {
        let window = &have[k..k + wanted.len()];
        let mut differs = Vec::new();
        let matches = wanted.iter().zip(window).enumerate().all(|(idx, (w, h))| {
            if w.text == h.text {
                return true;
            }
            if parameterize
                && w.kind == h.kind
                && matches!(
                    w.kind,
                    PolyTokenKind::Number | PolyTokenKind::Str | PolyTokenKind::Char
                )
            {
                differs.push((idx, h.text.clone()));
                return true;
            }
            false
        });

        let (from, to) = (window[0].start, window.last().unwrap().end);
        let overlaps = exclude.is_some_and(|(s, e)| from < e && s < to);

        if matches && !overlaps && has_complete_expression_boundaries(text, from, to) {
            out.push(PolyOccurrence {
                start: from,
                end: to,
                differs,
            });
            k += wanted.len();
        } else {
            k += 1;
        }
    }

    out
}

#[allow(clippy::too_many_arguments)]
pub async fn extract_function_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    (line, col): (u32, u32),
    (end_line, end_col): (u32, u32),
    name: &str,
    duplicates: bool,
    parameterize: bool,
    other_files: bool,
) -> Result<Extracted> {
    anyhow::ensure!(
        !name.is_empty()
            && name.chars().all(is_ident)
            && !name.starts_with(|c: char| c.is_ascii_digit()),
        "`{name}` is not an identifier"
    );
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read {}", file.display()))?;
    anyhow::ensure!(
        !mentions(&text, name),
        "the file already has something called `{name}`; choose another name"
    );

    let start = crate::signature::offset_of(&text, line, col)
        .context("the start is not in the file")?;
    let end = crate::signature::offset_of(&text, end_line, end_col)
        .context("the end is not in the file")?;
    anyhow::ensure!(start < end, "the selection is empty");
    let selection = text[start..end].trim().to_string();
    anyhow::ensure!(!selection.is_empty(), "the selection is empty");
    anyhow::ensure!(
        is_balanced(&selection),
        "the selection contains an unbalanced delimiter or string"
    );

    let lang = Language::of(file).unwrap_or(Language::TypeScript);

    let (scope_start, scope_end, _enclosing_fn, enclosing_ret, is_method, method_indent) =
        find_enclosing_scope(&text, lang, start, end);
    let scope_before = &text[scope_start..start];
    let scope_after = &text[end..scope_end];

    let mut inputs = extract_input_variables(&selection, scope_before, lang);
    let output = analyze_outputs(&selection, scope_after, scope_before, lang);

    let mut in_file_copies = Vec::new();
    if duplicates {
        in_file_copies = find_duplicates_in_text(
            &selection,
            &text,
            Some((start, end)),
            parameterize,
            lang,
        );
    }

    let sel_tokens = tokenize_polyglot(&selection, lang);
    let mut param_descriptions = Vec::new();
    let mut param_indices: Vec<(usize, String, String)> = Vec::new();

    if parameterize {
        let mut differing_token_indices: Vec<usize> = in_file_copies
            .iter()
            .flat_map(|c| c.differs.iter().map(|(idx, _)| *idx))
            .collect();
        differing_token_indices.sort_unstable();
        differing_token_indices.dedup();

        for (n, &tok_idx) in differing_token_indices.iter().enumerate() {
            if tok_idx < sel_tokens.len() {
                let orig_lit = sel_tokens[tok_idx].text.clone();
                let param_name = if differing_token_indices.len() == 1 {
                    "value".to_string()
                } else {
                    format!("value{}", n + 1)
                };
                let ty = if orig_lit.starts_with('"') {
                    match lang {
                        Language::Zig => Some("[]const u8".into()),
                        _ => Some("string".into()),
                    }
                } else if orig_lit.starts_with('\'') {
                    match lang {
                        Language::Zig => Some("u8".into()),
                        _ => Some("char".into()),
                    }
                } else {
                    match lang {
                        Language::Go | Language::Cpp | Language::C | Language::Csharp => Some("int".into()),
                        Language::Swift | Language::Kotlin => Some("Int".into()),
                        Language::Zig => Some("usize".into()),
                        _ => Some("number".into()),
                    }
                };
                inputs.push(ExtractedParam {
                    name: param_name.clone(),
                    ty: ty.clone(),
                });
                let ty_str = ty.unwrap_or_else(|| "any".into());
                param_descriptions.push(format!("{param_name}: {ty_str}"));
                param_indices.push((tok_idx, param_name, orig_lit));
            }
        }
    }

    let fn_selection_body = if param_indices.is_empty() {
        selection.clone()
    } else {
        let mut body_edits: Vec<(usize, usize, String)> = Vec::new();
        for (tok_idx, param_name, _) in &param_indices {
            if let Some(t) = sel_tokens.get(*tok_idx) {
                body_edits.push((t.start, t.end, param_name.clone()));
            }
        }
        apply_edits(&selection, &body_edits)
    };

    let param_names: Vec<String> = inputs.iter().map(|p| p.name.clone()).collect();
    let orig_args: Vec<String> = inputs
        .iter()
        .map(|p| {
            if let Some((_, _, orig_lit)) = param_indices.iter().find(|(_, name, _)| name == &p.name) {
                orig_lit.clone()
            } else {
                p.name.clone()
            }
        })
        .collect();

    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let mut sel_indent = text[line_start..start]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect::<String>();
    if sel_indent.is_empty() {
        sel_indent = text[start..end]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect::<String>();
    }
    let is_method = is_method || selection.contains("this.") || selection.contains("this ");
    let is_bol = text[..start].ends_with('\n') || start == 0 || text[line_start..start].trim().is_empty();

    let mut call_replacement = if is_bol {
        generate_call_replacement(name, &orig_args, &param_names, &output, lang, is_method, &sel_indent)
    } else {
        generate_call_replacement(name, &orig_args, &param_names, &output, lang, is_method, "")
    };
    if text[start..end].ends_with('\n') && !call_replacement.ends_with('\n') {
        call_replacement.push('\n');
    }

    let is_exported = other_files || text.contains("export ");
    let fn_code = generate_function_code(
        name,
        &inputs,
        &output,
        &fn_selection_body,
        lang,
        is_method,
        is_exported,
        &method_indent,
        enclosing_ret.as_deref(),
    );

    let mut file_edits: Vec<(usize, usize, String)> = Vec::new();
    file_edits.push((start, end, call_replacement.clone()));

    let mut duplicate_records = Vec::new();
    for copy in &in_file_copies {
        let (dup_line, _) = crate::signature::position_at(&text, copy.start)?;
        let dup_line_start = text[..copy.start].rfind('\n').map_or(0, |i| i + 1);
        let dup_indent = text[dup_line_start..copy.start]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect::<String>();
        let dup_is_bol = text[..copy.start].ends_with('\n') || copy.start == 0 || text[dup_line_start..copy.start].trim().is_empty();
        let copy_args: Vec<String> = inputs
            .iter()
            .map(|p| {
                if let Some((tok_idx, _, _)) = param_indices.iter().find(|(_, name, _)| name == &p.name) {
                    if let Some((_, copy_lit)) = copy.differs.iter().find(|(idx, _)| idx == tok_idx) {
                        copy_lit.clone()
                    } else {
                        p.name.clone()
                    }
                } else {
                    p.name.clone()
                }
            })
            .collect();
        let mut dup_call = if dup_is_bol {
            generate_call_replacement(name, &copy_args, &param_names, &output, lang, is_method, &dup_indent)
        } else {
            generate_call_replacement(name, &copy_args, &param_names, &output, lang, is_method, "")
        };
        if text[copy.start..copy.end].ends_with('\n') && !dup_call.ends_with('\n') {
            dup_call.push('\n');
        }
        file_edits.push((copy.start, copy.end, dup_call));
        duplicate_records.push(Duplicate {
            file: display(root, file),
            line: dup_line,
            replaced: true,
            reason: None,
            passes: copy.differs.iter().map(|(_, v)| v.clone()).collect(),
        });
    }

    let insert_pos = if is_method {
        scope_end
    } else {
        scope_start
    };
    let sep = if lang == Language::Python && !is_method {
        "\n\n"
    } else {
        "\n"
    };
    let insert_text = if is_method {
        format!("\n{fn_code}")
    } else {
        format!("{fn_code}{sep}")
    };
    file_edits.push((insert_pos, insert_pos, insert_text));

    let new_text = apply_edits(&text, &file_edits);
    let mut rewritten = vec![(file.to_string_lossy().to_string(), new_text)];

    if other_files {
        let sources = collect_workspace_sources(root, lang);
        for other in sources {
            if other == *file {
                continue;
            }
            let Ok(other_text) = std::fs::read_to_string(&other) else {
                continue;
            };
            let copies =
                find_duplicates_in_text(&selection, &other_text, None, parameterize, lang);
            if !copies.is_empty() {
                let uncovered_literal = copies.iter().find_map(|copy| {
                    copy.differs.iter().find_map(|(token_idx, _)| {
                        (!param_indices.iter().any(|(known_idx, _, _)| known_idx == token_idx))
                            .then_some(*token_idx)
                    })
                });
                anyhow::ensure!(
                    uncovered_literal.is_none(),
                    "{} has a differing literal at token {}, but no parameter was created for it; nothing was rewritten",
                    display(root, &other),
                    uncovered_literal.unwrap_or_default()
                );
                let mut other_edits: Vec<(usize, usize, String)> = Vec::new();
                for copy in &copies {
                    let (dup_line, _) = crate::signature::position_at(&other_text, copy.start)?;
                    let copy_args: Vec<String> = inputs
                        .iter()
                        .map(|p| {
                            if let Some((tok_idx, _, _)) = param_indices.iter().find(|(_, name, _)| name == &p.name) {
                                if let Some((_, copy_lit)) = copy.differs.iter().find(|(idx, _)| idx == tok_idx) {
                                    copy_lit.clone()
                                } else {
                                    p.name.clone()
                                }
                            } else {
                                p.name.clone()
                            }
                        })
                        .collect();
                    let dup_call =
                        generate_call_replacement(name, &copy_args, &param_names, &output, lang, false, "");
                    other_edits.push((copy.start, copy.end, dup_call));
                    duplicate_records.push(Duplicate {
                        file: display(root, &other),
                        line: dup_line,
                        replaced: true,
                        reason: None,
                        passes: copy.differs.iter().map(|(_, v)| v.clone()).collect(),
                    });
                }
                let mut new_other = apply_edits(&other_text, &other_edits);
                let import_stmt = match lang {
                    Language::TypeScript | Language::JavaScript => {
                        let rel = crate::move_polyglot::relative_import_specifier(&other, file);
                        format!("import {{ {name} }} from \"{rel}\";\n")
                    }
                    Language::Python => {
                        let mod_name = file.file_stem().unwrap_or_default().to_string_lossy();
                        format!("from .{mod_name} import {name}\n")
                    }
                    _ => String::new(),
                };
                if !import_stmt.is_empty() {
                    new_other.insert_str(0, &import_stmt);
                }
                rewritten.push((other.to_string_lossy().to_string(), new_other));
            }
        }
    }

    let validate_files: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &validate_files, &[]).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| {
            r.items
                .iter()
                .filter(|d| d.severity == "error")
                .map(move |d| format!("{}:{}:{}: {}", r.file, d.line, d.col, d.message))
        })
        .collect();

    let extracted = Extracted {
        name: name.to_string(),
        root: root.to_path_buf(),
        file: display(root, file),
        call: call_replacement,
        parameters: param_descriptions,
        duplicates: duplicate_records,
        rewritten,
        diagnostics,
        applied: false,
    };

    Ok(extracted)
}

fn apply_edits(text: &str, edits: &[(usize, usize, String)]) -> String {
    let mut sorted = edits.to_vec();
    sorted.sort_by_key(|(from, _, _)| std::cmp::Reverse(*from));
    let mut out = text.to_string();
    for (from, to, replacement) in sorted {
        if from <= to && to <= out.len() {
            out.replace_range(from..to, &replacement);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_polyglot_multibyte_unicode() {
        let code = "let price = 50€; let ratio = 10 / 2; let name = \"café\";";
        let tokens = tokenize_polyglot(code, Language::TypeScript);
        assert!(tokens.iter().any(|t| t.text == "€" && t.kind == PolyTokenKind::Punct));
        assert!(tokens.iter().any(|t| t.text == "\"café\"" && t.kind == PolyTokenKind::Str));
    }

    #[test]
    fn test_kotlin_extract_function_generation() {
        let params = vec![ExtractedParam {
            name: "ch".to_string(),
            ty: Some("Char".to_string()),
        }];
        let output = OutputKind::Expression("ch.digitToInt(16)".to_string());
        let fn_code = generate_function_code(
            "parseHexDigit",
            &params,
            &output,
            "ch.digitToInt(16)",
            Language::Kotlin,
            false,
            false,
            "",
            Some("Int"),
        );
        assert!(fn_code.contains("private fun parseHexDigit(ch: Char): Int {"));
        assert!(fn_code.contains("return ch.digitToInt(16)"));
        assert!(!fn_code.contains("return ch.digitToInt(16);"));

        let call = generate_call_replacement(
            "parseHexDigit",
            &["c".to_string()],
            &["ch".to_string()],
            &OutputKind::SingleVar {
                name: "digit".to_string(),
                is_new: true,
            },
            Language::Kotlin,
            false,
            "    ",
        );
        assert_eq!(call, "    val digit = parseHexDigit(c)");
    }

    #[test]
    fn test_csharp_extract_function_generation() {
        let params = vec![ExtractedParam {
            name: "input".to_string(),
            ty: Some("string".to_string()),
        }];
        let output = OutputKind::Expression("input.Trim()".to_string());
        let fn_code = generate_function_code(
            "CleanInput",
            &params,
            &output,
            "input.Trim()",
            Language::Csharp,
            false,
            false,
            "    ",
            Some("string"),
        );
        assert!(fn_code.contains("private static string CleanInput(string input)"));
        assert!(fn_code.contains("return input.Trim();"));

        let call = generate_call_replacement(
            "CleanInput",
            &["raw".to_string()],
            &["input".to_string()],
            &OutputKind::SingleVar {
                name: "cleaned".to_string(),
                is_new: true,
            },
            Language::Csharp,
            false,
            "        ",
        );
        assert_eq!(call, "        var cleaned = CleanInput(raw);");
    }
}
