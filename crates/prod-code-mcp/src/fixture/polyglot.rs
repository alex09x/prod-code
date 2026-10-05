//! Multi-language fixture parsing and value generation (Roadmap 8.5).
//!
//! Generates compile-ready test fixtures and dummy values for Go, TypeScript, Python,
//! Rust, C++, and Swift with deterministic realistic values in randomized mode.

use super::mock::{MethodSignature, generate_mock};
use crate::parameter_object::Language;

/// The shape of a type across languages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolyglotShape {
    /// A struct, interface, class or object with named fields: `(name, type)`.
    Record(Vec<(String, String)>),
    /// A tuple struct or positional sequence of types.
    Tuple(Vec<String>),
    /// A unit struct or void type.
    Unit,
    /// An enum and its variant names.
    Enum(Vec<String>),
    /// An interface, trait, or protocol with method signatures.
    Interface { methods: Vec<MethodSignature> },
    /// An interface with both named properties and method signatures.
    InterfaceWithFields {
        methods: Vec<MethodSignature>,
        fields: Vec<(String, String)>,
    },
}

/// Parses a declaration into a `PolyglotShape` based on the file language.
pub fn parse_polyglot_shape(decl: &str, language: Language) -> Option<PolyglotShape> {
    match language {
        Language::Go => parse_go_shape(decl),
        Language::TypeScript | Language::JavaScript => parse_ts_shape(decl),
        Language::Python => parse_python_shape(decl),
        Language::Rust => parse_rust_shape(decl),
        Language::Cpp | Language::C => parse_cpp_shape(decl),
        Language::Swift => parse_swift_shape(decl),
        Language::Java => parse_cpp_shape(decl),
    }
}

/// Parses Go `struct` and `interface` declarations.
pub fn parse_go_shape(decl: &str) -> Option<PolyglotShape> {
    let t = decl.trim();
    let open = t.find('{')?;
    let close = t.rfind('}')?;
    let header = &t[..open];
    let body = &t[open + 1..close];

    if header.contains("interface") {
        let mut methods = Vec::new();
        for line in body.lines() {
            let line = strip_comments(line);
            if line.is_empty() {
                continue;
            }
            if let Some(m) = parse_go_method_signature(line) {
                methods.push(m);
            }
        }
        return Some(PolyglotShape::Interface { methods });
    }

    if header.contains("struct") {
        let mut fields = Vec::new();
        for line in body.lines() {
            let line = strip_comments(line);
            let line = strip_go_tags(line);
            if line.is_empty() {
                continue;
            }
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.is_empty() {
                continue;
            }
            if tokens.len() == 1 {
                // Embedded type e.g. `*Config` or `sync.Mutex`
                let raw = tokens[0];
                let name = raw.trim_start_matches('*').rsplit('.').next().unwrap_or(raw);
                fields.push((name.to_string(), raw.to_string()));
            } else {
                let ty = tokens.last()?.to_string();
                let names_part = tokens[..tokens.len() - 1].join(" ");
                for name in names_part.split(',') {
                    let name = name.trim();
                    if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        fields.push((name.to_string(), ty.clone()));
                    }
                }
            }
        }
        return Some(PolyglotShape::Record(fields));
    }

    None
}

fn parse_go_method_signature(line: &str) -> Option<MethodSignature> {
    let open_paren = line.find('(')?;
    let name = line[..open_paren].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let rest = &line[open_paren + 1..];
    let close_paren = find_matching_paren(rest)?;
    let params_str = &rest[..close_paren];
    let returns_str = rest[close_paren + 1..].trim();

    let params = parse_go_parameters(params_str)?;

    let return_type = if returns_str.is_empty() {
        None
    } else {
        Some(parse_go_result_types(returns_str)?)
    };

    Some(MethodSignature {
        name: name.to_string(),
        params,
        return_type,
    })
}

fn parse_go_parameters(parameters: &str) -> Option<Vec<(String, String)>> {
    let parts = split_comma_top_level(parameters);
    let mut params = Vec::new();
    let mut pending_names = Vec::new();
    for (index, raw) in parts.iter().enumerate() {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((names_part, ty)) = part.split_once(char::is_whitespace) {
            let mut names = pending_names.drain(..).collect::<Vec<_>>();
            names.extend(names_part.split_whitespace().map(str::to_string));
            let ty = ty.trim();
            if names.is_empty() || ty.is_empty() {
                return None;
            }
            for name in names {
                params.push((name, ty.to_string()));
            }
        } else if parts
            .get(index + 1)
            .is_some_and(|next| next.split_once(char::is_whitespace).is_some())
            && !go_type_form(part)
        {
            pending_names.push(part.to_string());
        } else {
            if !pending_names.is_empty() {
                return None;
            }
            params.push((String::new(), part.to_string()));
        }
    }
    if !pending_names.is_empty() {
        return None;
    }
    Some(params)
}

fn parse_go_result_types(results: &str) -> Option<String> {
    let results = results.trim();
    let inside = if results.starts_with('(') && results.ends_with(')') {
        &results[1..results.len() - 1]
    } else {
        results
    };
    let mut types = Vec::new();
    let parts = split_comma_top_level(inside);
    let mut pending_names = Vec::new();
    for (index, raw) in parts.iter().enumerate() {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((names_part, ty)) = part.split_once(char::is_whitespace)
            && !go_type_form(part)
        {
            if !pending_names.is_empty() {
                types.extend(std::iter::repeat_n(ty.trim().to_string(), pending_names.len()));
                pending_names.clear();
            }
            types.extend(
                names_part
                    .split_whitespace()
                    .map(|_| ty.trim().to_string()),
            );
        } else if parts
            .get(index + 1)
            .is_some_and(|next| next.split_once(char::is_whitespace).is_some())
            && !go_type_form(part)
        {
            pending_names.push(part.to_string());
        } else {
            if !pending_names.is_empty() {
                return None;
            }
            types.push(part.to_string());
        }
    }
    if !pending_names.is_empty() {
        return None;
    }
    (!types.is_empty()).then(|| types.join(", "))
}

fn go_type_form(value: &str) -> bool {
    matches!(
        value,
        "bool" | "string" | "error" | "byte" | "rune" | "int" | "int8" | "int16"
            | "int32" | "int64" | "uint" | "uint8" | "uint16" | "uint32" | "uint64"
            | "uintptr" | "float32" | "float64" | "complex64" | "complex128" | "any"
    ) || value.starts_with(['*', '['])
        || value.starts_with("map[")
        || value.starts_with("chan")
        || value.starts_with("<-chan")
        || value.starts_with("func")
        || value.starts_with("struct {")
        || value.starts_with("interface {")
        || value.contains('.')
        || value.chars().next().is_some_and(char::is_uppercase)
}

/// Parses TypeScript / JavaScript `interface`, `type`, and `enum`.
pub fn parse_ts_shape(decl: &str) -> Option<PolyglotShape> {
    let t = decl.trim();
    if t.contains("enum ") {
        let open = t.find('{')?;
        let close = t.rfind('}')?;
        let variants = t[open + 1..close]
            .lines()
            .map(strip_comments)
            .filter(|l| !l.is_empty())
            .map(|l| {
                let name = l.split(['=', ',']).next().unwrap_or(l).trim();
                name.to_string()
            })
            .filter(|n| !n.is_empty())
            .collect();
        return Some(PolyglotShape::Enum(variants));
    }

    let open = t.find('{')?;
    let close = t.rfind('}')?;
    let body = &t[open + 1..close];

    let mut fields = Vec::new();
    let mut methods = Vec::new();

    for raw in body.split([';', '\n']) {
        let line = strip_comments(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(open_paren) = line.find('(')
            && let Some(colon) = line.find(':')
            && open_paren < colon
        {
            let name = line[..open_paren].trim().trim_start_matches("async ").trim();
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                let rest = &line[open_paren + 1..];
                if let Some(close_paren) = find_matching_paren(rest) {
                    let params_str = &rest[..close_paren];
                    let ret_str = rest[close_paren + 1..].trim().trim_start_matches(':').trim();
                    let mut params = Vec::new();
                    for part in split_comma_top_level(params_str) {
                        let part = part.trim();
                        if let Some((pn, pt)) = part.split_once(':') {
                            params.push((pn.trim().trim_end_matches('?').to_string(), pt.trim().to_string()));
                        }
                    }
                    methods.push(MethodSignature {
                        name: name.to_string(),
                        params,
                        return_type: Some(ret_str.to_string()),
                    });
                    continue;
                }
            }
        }

        // Check for property: name: type or name?: type
        if let Some((name_part, ty_part)) = line.split_once(':') {
            let name = name_part
                .trim()
                .trim_start_matches("readonly ")
                .trim_start_matches("public ")
                .trim_end_matches('?')
                .trim();
            let ty = ty_part.trim().trim_end_matches(',').trim_end_matches(';').trim();
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') {
                fields.push((name.to_string(), ty.to_string()));
            }
        }
    }

    if !methods.is_empty() && !fields.is_empty() {
        Some(PolyglotShape::InterfaceWithFields { methods, fields })
    } else if !methods.is_empty() {
        Some(PolyglotShape::Interface { methods })
    } else if !fields.is_empty() {
        Some(PolyglotShape::Record(fields))
    } else {
        Some(PolyglotShape::Record(Vec::new()))
    }
}

/// Parses Python `@dataclass`, `BaseModel`, `class with __init__`, or `Protocol`.
pub fn parse_python_shape(decl: &str) -> Option<PolyglotShape> {
    let lines: Vec<&str> = decl.lines().collect();
    let mut fields = Vec::new();
    let mut methods = Vec::new();
    let mut in_init = false;

    for line in lines {
        let trimmed = strip_comments(line);
        if trimmed.is_empty() {
            continue;
        }

        // Check for __init__
        if trimmed.starts_with("def __init__") {
            in_init = true;
            if let Some(open) = trimmed.find('(') {
                let rest = trimmed[open + 1..].trim_end_matches(':').trim_end_matches(')');
                for part in split_comma_top_level(rest) {
                    let part = part.trim();
                    if part == "self" || part.is_empty() {
                        continue;
                    }
                    if let Some((name_part, ty_part)) = part.split_once(':') {
                        let name = name_part.trim();
                        let ty = ty_part.split('=').next().unwrap_or(ty_part).trim();
                        fields.push((name.to_string(), ty.to_string()));
                    } else if let Some((name_part, _)) = part.split_once('=') {
                        fields.push((name_part.trim().to_string(), String::new()));
                    } else {
                        fields.push((part.to_string(), String::new()));
                    }
                }
            }
            continue;
        }

        // Check for methods
        if trimmed.starts_with("def ") && !trimmed.starts_with("def __") {
            in_init = false;
            let sig = trimmed.trim_start_matches("def ");
            if let Some(open) = sig.find('(') {
                let name = sig[..open].trim();
                let rest = &sig[open + 1..];
                if let Some(close) = find_matching_paren(rest) {
                    let params_str = &rest[..close];
                    let ret_part = rest[close + 1..].trim().trim_end_matches(':').trim();
                    let return_type = ret_part
                        .strip_prefix("->")
                        .map(|r| r.trim().to_string());

                    let mut params = Vec::new();
                    for part in split_comma_top_level(params_str) {
                        let part = part.trim();
                        if part == "self" || part.is_empty() {
                            continue;
                        }
                        if let Some((pn, pt)) = part.split_once(':') {
                            params.push((pn.trim().to_string(), pt.trim().to_string()));
                        } else {
                            params.push((part.to_string(), String::new()));
                        }
                    }

                    methods.push(MethodSignature {
                        name: name.to_string(),
                        params,
                        return_type,
                    });
                }
            }
            continue;
        }

        // Check for field: type
        if !in_init
            && !trimmed.starts_with("def ")
            && !trimmed.starts_with("class ")
            && !trimmed.starts_with('@')
            && let Some((name_part, ty_part)) = trimmed.split_once(':')
        {
            let name = name_part.trim();
            let ty = ty_part.split('=').next().unwrap_or(ty_part).trim();
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                fields.push((name.to_string(), ty.to_string()));
            }
        }
    }

    if !methods.is_empty() && fields.is_empty() {
        Some(PolyglotShape::Interface { methods })
    } else if !fields.is_empty() {
        Some(PolyglotShape::Record(fields))
    } else if !methods.is_empty() {
        Some(PolyglotShape::Interface { methods })
    } else {
        Some(PolyglotShape::Record(Vec::new()))
    }
}

/// Parses Rust `struct`, `enum`, or `trait`.
pub fn parse_rust_shape(decl: &str) -> Option<PolyglotShape> {
    let t = decl.trim();
    if t.starts_with("pub trait ") || t.starts_with("trait ") {
        let open = t.find('{')?;
        let close = t.rfind('}')?;
        let body = &t[open + 1..close];
        let mut methods = Vec::new();
        for line in body.lines() {
            let line = strip_comments(line);
            let line = line.trim();
            if line.starts_with("fn ") {
                let rest = line.trim_start_matches("fn ");
                if let Some(open_paren) = rest.find('(') {
                    let name = rest[..open_paren].trim();
                    let after = &rest[open_paren + 1..];
                    if let Some(close_paren) = find_matching_paren(after) {
                        let params_str = &after[..close_paren];
                        let ret_part = after[close_paren + 1..]
                            .trim()
                            .trim_end_matches(';')
                            .trim();
                        let return_type = ret_part
                            .strip_prefix("->")
                            .map(|r| r.trim().to_string());

                        let mut params = Vec::new();
                        for part in split_comma_top_level(params_str) {
                            let part = part.trim();
                            if part.is_empty() {
                                continue;
                            }
                            if part == "&self" || part == "&mut self" || part == "self" {
                                params.push((part.to_string(), String::new()));
                            } else if let Some((pn, pt)) = part.split_once(':') {
                                params.push((pn.trim().to_string(), pt.trim().to_string()));
                            }
                        }

                        methods.push(MethodSignature {
                            name: name.to_string(),
                            params,
                            return_type,
                        });
                    }
                }
            }
        }
        return Some(PolyglotShape::Interface { methods });
    }

    if let Some(shape) = super::parse_shape(decl) {
        return Some(match shape {
            super::Shape::Record(f) => PolyglotShape::Record(f),
            super::Shape::Tuple(t) => PolyglotShape::Tuple(t),
            super::Shape::Unit => PolyglotShape::Unit,
            super::Shape::Enum(v) => PolyglotShape::Enum(v),
        });
    }
    None
}

/// Parses C / C++ struct, class, or pure-virtual interface.
pub fn parse_cpp_shape(decl: &str) -> Option<PolyglotShape> {
    let t = decl.trim();
    let open = t.find('{')?;
    let close = t.rfind('}')?;
    let body = &t[open + 1..close];

    let mut fields = Vec::new();
    let mut methods = Vec::new();

    for raw in body.split(';') {
        let line = strip_comments(raw);
        let line = line
            .trim()
            .trim_start_matches("public:")
            .trim_start_matches("private:")
            .trim_start_matches("protected:")
            .trim();
        if line.is_empty() {
            continue;
        }

        // Virtual method e.g. `virtual void run(int code) = 0`
        if line.starts_with("virtual ") {
            let rest = line.trim_start_matches("virtual ").trim();
            if let Some(open_paren) = rest.find('(') {
                let ret_and_name = rest[..open_paren].trim();
                let after = &rest[open_paren + 1..];
                if let Some(close_paren) = find_matching_paren(after) {
                    let params_str = &after[..close_paren];
                    let tokens: Vec<&str> = ret_and_name.split_whitespace().collect();
                    if tokens.len() >= 2 {
                        let name = tokens.last()?.trim_start_matches('*');
                        let ret = tokens[..tokens.len() - 1].join(" ");
                        let mut params = Vec::new();
                        for part in split_comma_top_level(params_str) {
                            let part = part.trim();
                            if !part.is_empty() {
                                let ptoks: Vec<&str> = part.split_whitespace().collect();
                                if ptoks.len() >= 2 {
                                    params.push((
                                        ptoks.last().unwrap().to_string(),
                                        ptoks[..ptoks.len() - 1].join(" "),
                                    ));
                                } else {
                                    params.push((String::new(), part.to_string()));
                                }
                            }
                        }
                        methods.push(MethodSignature {
                            name: name.to_string(),
                            params,
                            return_type: Some(ret),
                        });
                        continue;
                    }
                }
            }
        }

        // Field e.g. `std::string host` or `int port = 0`
        let decl_part = line.split('=').next().unwrap_or(line).trim();
        let tokens: Vec<&str> = decl_part.split_whitespace().collect();
        if tokens.len() >= 2 {
            let name = tokens.last()?.trim_start_matches('*');
            let ty = tokens[..tokens.len() - 1].join(" ");
            if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                fields.push((name.to_string(), ty));
            }
        }
    }

    if !methods.is_empty() && fields.is_empty() {
        Some(PolyglotShape::Interface { methods })
    } else {
        Some(PolyglotShape::Record(fields))
    }
}

/// Parses Swift `struct`, `class`, or `protocol`.
pub fn parse_swift_shape(decl: &str) -> Option<PolyglotShape> {
    let t = decl.trim();
    let open = t.find('{')?;
    let close = t.rfind('}')?;
    let header = &t[..open];
    let body = &t[open + 1..close];

    if header.contains("protocol ") {
        let mut methods = Vec::new();
        for line in body.lines() {
            let line = strip_comments(line);
            let line = line.trim();
            if line.starts_with("func ") {
                let rest = line.trim_start_matches("func ");
                if let Some(open_paren) = rest.find('(') {
                    let name = rest[..open_paren].trim();
                    let after = &rest[open_paren + 1..];
                    if let Some(close_paren) = find_matching_paren(after) {
                        let params_str = &after[..close_paren];
                        let ret_part = after[close_paren + 1..].trim();
                        let return_type = ret_part
                            .strip_prefix("->")
                            .map(|r| r.trim().to_string());

                        let mut params = Vec::new();
                        for part in split_comma_top_level(params_str) {
                            let part = part.trim();
                            if !part.is_empty()
                                && let Some((pn, pt)) = part.split_once(':')
                            {
                                params.push((pn.trim().to_string(), pt.trim().to_string()));
                            }
                        }

                        methods.push(MethodSignature {
                            name: name.to_string(),
                            params,
                            return_type,
                        });
                    }
                }
            }
        }
        return Some(PolyglotShape::Interface { methods });
    }

    let mut fields = Vec::new();
    for line in body.lines() {
        let line = strip_comments(line);
        let line = line.trim();
        if line.starts_with("var ") || line.starts_with("let ") {
            let rest = line.trim_start_matches("var ").trim_start_matches("let ").trim();
            if let Some((name_part, ty_part)) = rest.split_once(':') {
                let name = name_part.trim();
                let ty = ty_part.split('=').next().unwrap_or(ty_part).trim();
                if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    fields.push((name.to_string(), ty.to_string()));
                }
            }
        }
    }

    Some(PolyglotShape::Record(fields))
}

/// Generates a sample value for a type in the specified language, either default or randomized.
pub fn sample_value_for_type(
    language: Language,
    ty: &str,
    field_name: Option<&str>,
    randomized: bool,
) -> String {
    let t = ty.trim();
    if language == Language::Python && t.is_empty() {
        return "None".to_string();
    }
    let fn_opt = field_name.map(str::to_ascii_lowercase);
    let fn_ref = fn_opt.as_deref().unwrap_or("");

    match language {
        Language::Go => {
            if !randomized {
                match t {
                    "bool" => "false".to_string(),
                    "string" => "\"\"".to_string(),
                    "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8" | "uint16"
                    | "uint32" | "uint64" | "byte" | "rune" | "uintptr" => "0".to_string(),
                    "float32" | "float64" => "0.0".to_string(),
                    "time.Time" => "time.Time{}".to_string(),
                    "time.Duration" => "0".to_string(),
                    "error" => "nil".to_string(),
                    p if p.starts_with('*') || p.starts_with("[]") || p.starts_with("map[") => {
                        "nil".to_string()
                    }
                    other => format!("{other}{{}}"),
                }
            } else {
                match t {
                    "bool" => "true".to_string(),
                    "string" => {
                        if fn_ref.contains("id") {
                            "\"id_9823\"".to_string()
                        } else if fn_ref.contains("email") {
                            "\"user@example.com\"".to_string()
                        } else if fn_ref.contains("name") {
                            "\"test_sample_name\"".to_string()
                        } else if fn_ref.contains("url") || fn_ref.contains("uri") {
                            "\"https://example.com/api\"".to_string()
                        } else if fn_ref.contains("token") {
                            "\"tok_sec_7a8b9c\"".to_string()
                        } else {
                            format!("\"sample_{}\"", field_name.unwrap_or("val"))
                        }
                    }
                    "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8" | "uint16"
                    | "uint32" | "uint64" | "uintptr" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else if fn_ref.contains("age") {
                            "30".to_string()
                        } else if fn_ref.contains("count") || fn_ref.contains("total") {
                            "100".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "float32" | "float64" => {
                        if fn_ref.contains("price") || fn_ref.contains("amount") {
                            "99.95".to_string()
                        } else {
                            "3.14".to_string()
                        }
                    }
                    "time.Time" => {
                        "time.Date(2026, 1, 1, 12, 0, 0, 0, time.UTC)".to_string()
                    }
                    "time.Duration" => "5 * time.Second".to_string(),
                    "[]string" => "[]string{\"sample_1\", \"sample_2\"}".to_string(),
                    "[]byte" => "[]byte(\"sample_bytes\")".to_string(),
                    "[]int" => "[]int{1, 2, 3}".to_string(),
                    p if p.starts_with("[]") => {
                        let inner = &p[2..];
                        format!("[]{inner}{{{}}}", sample_value_for_type(language, inner, None, true))
                    }
                    p if p.starts_with('*') => {
                        let inner = &p[1..];
                        format!("&{inner}{{}}")
                    }
                    other => format!("{other}{{}}"),
                }
            }
        }
        Language::TypeScript | Language::JavaScript => {
            if !randomized {
                match t {
                    "boolean" => "false".to_string(),
                    "number" => "0".to_string(),
                    "string" => "\"\"".to_string(),
                    "void" => "undefined".to_string(),
                    "any" | "unknown" => "null".to_string(),
                    "Date" => "new Date(0)".to_string(),
                    t if t.ends_with("[]") || t.starts_with("Array<") => "[]".to_string(),
                    t if t.starts_with("Record<") => "{}".to_string(),
                    other => format!("{{}} as {other}"),
                }
            } else {
                match t {
                    "boolean" => "true".to_string(),
                    "string" => {
                        if fn_ref.contains("id") {
                            "\"id_9823\"".to_string()
                        } else if fn_ref.contains("email") {
                            "\"user@example.com\"".to_string()
                        } else if fn_ref.contains("name") {
                            "\"test_sample_name\"".to_string()
                        } else if fn_ref.contains("url") {
                            "\"https://example.com/api\"".to_string()
                        } else {
                            format!("\"sample_{}\"", field_name.unwrap_or("val"))
                        }
                    }
                    "number" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else if fn_ref.contains("age") {
                            "30".to_string()
                        } else if fn_ref.contains("price") {
                            "99.95".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "Date" => "\"2026-01-01T00:00:00.000Z\"".to_string(),
                    "string[]" => "[\"sample_1\", \"sample_2\"]".to_string(),
                    "number[]" => "[10, 20, 30]".to_string(),
                    t if t.ends_with("[]") => "[]".to_string(),
                    t if t.starts_with("Record<") => "{\"key\": \"sample_val\"}".to_string(),
                    other => format!("{{}} as {other}"),
                }
            }
        }
        Language::Python => {
            if !randomized {
                match t {
                    "bool" => "False".to_string(),
                    "int" => "0".to_string(),
                    "float" => "0.0".to_string(),
                    "str" => "\"\"".to_string(),
                    "None" => "None".to_string(),
                    t if t.starts_with("list") || t.starts_with("List") => "[]".to_string(),
                    t if t.starts_with("dict") || t.starts_with("Dict") => "{}".to_string(),
                    t if t.starts_with("set") || t.starts_with("Set") => "set()".to_string(),
                    t if t.starts_with("Optional") => "None".to_string(),
                    other => format!("{other}()"),
                }
            } else {
                match t {
                    "bool" => "True".to_string(),
                    "str" => {
                        if fn_ref.contains("id") {
                            "\"id_9823\"".to_string()
                        } else if fn_ref.contains("email") {
                            "\"user@example.com\"".to_string()
                        } else if fn_ref.contains("name") {
                            "\"test_sample_name\"".to_string()
                        } else {
                            format!("\"sample_{}\"", field_name.unwrap_or("val"))
                        }
                    }
                    "int" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "float" => {
                        if fn_ref.contains("price") {
                            "99.95".to_string()
                        } else {
                            "3.14".to_string()
                        }
                    }
                    "list[str]" | "List[str]" => "[\"sample_1\", \"sample_2\"]".to_string(),
                    "list[int]" | "List[int]" => "[1, 2, 3]".to_string(),
                    t if t.starts_with("list") || t.starts_with("List") => "[]".to_string(),
                    t if t.starts_with("dict") || t.starts_with("Dict") => {
                        "{\"key\": \"sample_val\"}".to_string()
                    }
                    other => format!("{other}()"),
                }
            }
        }
        Language::Rust => {
            if !randomized {
                match super::known_value(t) {
                    Some(v) => v,
                    None => "Default::default()".to_string(),
                }
            } else {
                match t {
                    "bool" => "true".to_string(),
                    "String" => {
                        format!("\"sample_{}\".to_string()", field_name.unwrap_or("val"))
                    }
                    "&str" | "str" => {
                        format!("\"sample_{}\"", field_name.unwrap_or("val"))
                    }
                    "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32"
                    | "u64" | "u128" | "usize" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else if fn_ref.contains("timeout") {
                            "30".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "f32" | "f64" => "3.14".to_string(),
                    "Duration" => "std::time::Duration::from_secs(30)".to_string(),
                    "SocketAddr" => "\"127.0.0.1:8080\".parse().unwrap()".to_string(),
                    p if p.starts_with("Vec<") => {
                        let inner = inner_of_rust(p, "Vec").unwrap_or("String");
                        let sample_inner = sample_value_for_type(language, inner, None, true);
                        format!("vec![{sample_inner}]")
                    }
                    p if p.starts_with("Option<") => {
                        let inner = inner_of_rust(p, "Option").unwrap_or("String");
                        let sample_inner = sample_value_for_type(language, inner, None, true);
                        format!("Some({sample_inner})")
                    }
                    _ => match super::known_value(t) {
                        Some(v) => v,
                        None => "Default::default()".to_string(),
                    },
                }
            }
        }
        Language::Cpp | Language::C => {
            if !randomized {
                match t {
                    "bool" => "false".to_string(),
                    "int" | "long" | "size_t" | "uint32_t" | "int64_t" => "0".to_string(),
                    "float" | "double" => "0.0".to_string(),
                    "std::string" | "string" => "\"\"".to_string(),
                    _ => "{}".to_string(),
                }
            } else {
                match t {
                    "bool" => "true".to_string(),
                    "std::string" | "string" => {
                        format!("\"sample_{}\"", field_name.unwrap_or("val"))
                    }
                    "int" | "long" | "size_t" | "uint32_t" | "int64_t" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "float" | "double" => "3.14".to_string(),
                    _ => "{}".to_string(),
                }
            }
        }
        Language::Swift => {
            if !randomized {
                match t {
                    "Bool" => "false".to_string(),
                    "Int" | "UInt" | "Int64" => "0".to_string(),
                    "Double" | "Float" => "0.0".to_string(),
                    "String" => "\"\"".to_string(),
                    t if t.ends_with('?') => "nil".to_string(),
                    t if t.starts_with('[') && t.ends_with(']') => {
                        if t.contains(':') {
                            "[:]".to_string()
                        } else {
                            "[]".to_string()
                        }
                    }
                    other => format!("{other}()"),
                }
            } else {
                match t {
                    "Bool" => "true".to_string(),
                    "String" => {
                        format!("\"sample_{}\"", field_name.unwrap_or("val"))
                    }
                    "Int" | "UInt" | "Int64" => {
                        if fn_ref.contains("port") {
                            "8080".to_string()
                        } else {
                            "42".to_string()
                        }
                    }
                    "Double" | "Float" => "3.14".to_string(),
                    _ => format!("{t}()"),
                }
            }
        }
        Language::Java => {
            if !randomized {
                match t {
                    "boolean" => "false".to_string(),
                    "String" => "\"\"".to_string(),
                    "byte" | "short" | "int" | "long" => "0".to_string(),
                    "float" => "0.0f".to_string(),
                    "double" => "0.0".to_string(),
                    "char" => "'\\0'".to_string(),
                    p if p.starts_with("List<") || p.starts_with("java.util.List<") => "java.util.Collections.emptyList()".to_string(),
                    p if p.starts_with("Map<") || p.starts_with("java.util.Map<") => "java.util.Collections.emptyMap()".to_string(),
                    p if p.starts_with("Set<") || p.starts_with("java.util.Set<") => "java.util.Collections.emptySet()".to_string(),
                    _ => "null".to_string(),
                }
            } else {
                match t {
                    "boolean" => "true".to_string(),
                    "String" => format!("\"test_{}\"", fn_ref),
                    "int" | "long" => "42".to_string(),
                    "double" | "float" => "3.14".to_string(),
                    _ => "null".to_string(),
                }
            }
        }
    }
}

/// Formats a complete value fixture expression and snippet for `type_name`.
pub fn format_polyglot_fixture(
    language: Language,
    type_name: &str,
    shape: &PolyglotShape,
    randomized: bool,
    mock: bool,
) -> (String, String) {
    if mock {
        let (methods, fields) = match shape {
            PolyglotShape::Interface { methods } => (methods.as_slice(), [].as_slice()),
            PolyglotShape::InterfaceWithFields { methods, fields } => {
                (methods.as_slice(), fields.as_slice())
            }
            PolyglotShape::Record(fields) => ([].as_slice(), fields.as_slice()),
            _ => ([].as_slice(), [].as_slice()),
        };
        let code = generate_mock(language, type_name, methods, fields);
        return (code.clone(), code);
    }

    match shape {
        PolyglotShape::Record(fields) => {
            let mut pairs = Vec::new();
            for (f, ty) in fields {
                let val = sample_value_for_type(language, ty, Some(f), randomized);
                pairs.push((f.clone(), val));
            }

            match language {
                Language::Go => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    {f}: {v},"));
                    }
                    let val = format!("{type_name}{{\n{}\n}}", lines.join("\n"));
                    let var_name = lower_camel_case(type_name);
                    let snippet = format!("var {var_name} = {val}");
                    (val, snippet)
                }
                Language::TypeScript | Language::JavaScript => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    {f}: {v},"));
                    }
                    let val = format!("{{\n{}\n}}", lines.join("\n"));
                    let var_name = lower_camel_case(type_name);
                    let snippet = format!("const {var_name}: {type_name} = {val};");
                    (val, snippet)
                }
                Language::Python => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    {f}={v},"));
                    }
                    let val = format!("{type_name}(\n{}\n)", lines.join("\n"));
                    let var_name = snake_case(type_name);
                    let snippet = format!("{var_name} = {val}");
                    (val, snippet)
                }
                Language::Rust => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    {f}: {v},"));
                    }
                    let val = format!("{type_name} {{\n{}\n}}", lines.join("\n"));
                    let var_name = snake_case(type_name);
                    let snippet = format!("let {var_name} = {val};");
                    (val, snippet)
                }
                Language::Cpp | Language::C => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    .{f} = {v},"));
                    }
                    let val = format!("{type_name}{{\n{}\n}}", lines.join("\n"));
                    let var_name = lower_camel_case(type_name);
                    let snippet = format!("{type_name} {var_name} = {val};");
                    (val, snippet)
                }
                Language::Swift => {
                    let mut lines = Vec::new();
                    for (f, v) in &pairs {
                        lines.push(format!("    {f}: {v},"));
                    }
                    let val = format!("{type_name}(\n{}\n)", lines.join("\n"));
                    let var_name = lower_camel_case(type_name);
                    let snippet = format!("let {var_name} = {val}");
                    (val, snippet)
                }
                Language::Java => {
                    let mut args = Vec::new();
                    for (_, v) in &pairs {
                        args.push(v.clone());
                    }
                    let val = format!("new {type_name}({})", args.join(", "));
                    let var_name = lower_camel_case(type_name);
                    let snippet = format!("{type_name} {var_name} = {val};");
                    (val, snippet)
                }
            }
        }
        PolyglotShape::Tuple(types) => {
            let parts: Vec<String> = types
                .iter()
                .map(|t| sample_value_for_type(language, t, None, randomized))
                .collect();
            let val = format!("{type_name}({})", parts.join(", "));
            let var_name = snake_case(type_name);
            (val.clone(), format!("let {var_name} = {val};"))
        }
        PolyglotShape::Unit => {
            let val = type_name.to_string();
            let var_name = snake_case(type_name);
            (val.clone(), format!("let {var_name} = {val};"))
        }
        PolyglotShape::Enum(variants) => {
            let chosen = if randomized && variants.len() > 1 {
                &variants[1]
            } else {
                variants.first().map(String::as_str).unwrap_or("Default")
            };
            let val = match language {
                Language::Rust => format!("{type_name}::{chosen}"),
                Language::Swift => format!("{type_name}.{chosen}"),
                Language::TypeScript => format!("{type_name}.{chosen}"),
                Language::Go => (*chosen).to_string(),
                _ => format!("{type_name}.{chosen}"),
            };
            let var_name = snake_case(type_name);
            (val.clone(), format!("let {var_name} = {val};"))
        }
        PolyglotShape::Interface { methods } => {
            let code = generate_mock(language, type_name, methods, &[]);
            (code.clone(), code)
        }
        PolyglotShape::InterfaceWithFields { methods, fields } => {
            let code = generate_mock(language, type_name, methods, fields);
            (code.clone(), code)
        }
    }
}

pub fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

pub fn lower_camel_case(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn strip_comments(line: &str) -> &str {
    let mut line = line.trim();
    if let Some(pos) = line.find("//") {
        line = line[..pos].trim();
    }
    if let Some(pos) = line.find('#') {
        line = line[..pos].trim();
    }
    line
}

fn strip_go_tags(line: &str) -> &str {
    if let Some(pos) = line.find('`') {
        line[..pos].trim()
    } else {
        line
    }
}

fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 1;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn split_comma_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' | '<' | '[' | '{' => {
                depth += 1;
                current.push(c);
            }
            ')' | '>' | ']' | '}' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

fn inner_of_rust<'a>(ty: &'a str, wrapper: &str) -> Option<&'a str> {
    let t = ty.trim();
    let open = t.find('<')?;
    let close = t.rfind('>')?;
    let name = t[..open].rsplit("::").next()?.trim();
    if name == wrapper {
        Some(t[open + 1..close].trim())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_go_struct_shape() {
        let decl = "type ServerConfig struct {\n    Host string `json:\"host\"`\n    Port int `json:\"port\"`\n    TLS bool\n}";
        let shape = parse_polyglot_shape(decl, Language::Go).expect("go struct shape");
        assert_eq!(
            shape,
            PolyglotShape::Record(vec![
                ("Host".into(), "string".into()),
                ("Port".into(), "int".into()),
                ("TLS".into(), "bool".into()),
            ])
        );
    }

    #[test]
    fn parses_go_interface_shape() {
        let decl = "type Reader interface {\n    Read(p []byte) (n int, err error)\n    Close() error\n}";
        let shape = parse_polyglot_shape(decl, Language::Go).expect("go interface shape");
        match shape {
            PolyglotShape::Interface { methods } => {
                assert_eq!(methods.len(), 2);
                assert_eq!(methods[0].name, "Read");
                assert_eq!(methods[0].params, vec![("p".into(), "[]byte".into())]);
                assert_eq!(methods[0].return_type, Some("n int, err error".into()));
                assert_eq!(methods[1].name, "Close");
                assert_eq!(methods[1].return_type, Some("error".into()));
            }
            _ => panic!("expected interface"),
        }
    }

    #[test]
    fn parses_ts_interface_shape() {
        let decl = "export interface UserProfile {\n    id: string;\n    displayName: string;\n    age?: number;\n    active: boolean;\n}";
        let shape = parse_polyglot_shape(decl, Language::TypeScript).expect("ts interface shape");
        assert_eq!(
            shape,
            PolyglotShape::Record(vec![
                ("id".into(), "string".into()),
                ("displayName".into(), "string".into()),
                ("age".into(), "number".into()),
                ("active".into(), "boolean".into()),
            ])
        );
    }

    #[test]
    fn parses_python_dataclass_shape() {
        let decl = "@dataclass\nclass Config:\n    host: str\n    port: int = 8080\n    enabled: bool = True";
        let shape = parse_polyglot_shape(decl, Language::Python).expect("python shape");
        assert_eq!(
            shape,
            PolyglotShape::Record(vec![
                ("host".into(), "str".into()),
                ("port".into(), "int".into()),
                ("enabled".into(), "bool".into()),
            ])
        );
    }

    #[test]
    fn generates_randomized_dummy_data() {
        let val = sample_value_for_type(Language::Go, "string", Some("email"), true);
        assert_eq!(val, "\"user@example.com\"");
        let port = sample_value_for_type(Language::Go, "int", Some("port"), true);
        assert_eq!(port, "8080");
        let active = sample_value_for_type(Language::TypeScript, "boolean", None, true);
        assert_eq!(active, "true");
    }

    #[test]
    fn formats_go_fixture_literal() {
        let shape = PolyglotShape::Record(vec![
            ("Host".into(), "string".into()),
            ("Port".into(), "int".into()),
        ]);
        let (val, snippet) = format_polyglot_fixture(Language::Go, "Config", &shape, false, false);
        assert!(val.contains("Host: \"\","));
        assert!(val.contains("Port: 0,"));
        assert!(snippet.contains("var config = Config{"));
    }

    #[test]
    fn formats_ts_fixture_literal() {
        let shape = PolyglotShape::Record(vec![
            ("id".into(), "string".into()),
            ("active".into(), "boolean".into()),
        ]);
        let (val, snippet) = format_polyglot_fixture(Language::TypeScript, "User", &shape, true, false);
        assert!(val.contains("id: \"id_9823\","));
        assert!(val.contains("active: true,"));
        assert!(snippet.contains("const user: User = {"));
    }
}
