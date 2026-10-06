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

use super::resolve::shape_of;
use super::types::{MANY_FIELDS, Shape, inner_of, split_top_level};

/// A value expression for a type, without consulting the analyzer.
///
/// Returns `None` when the type is not one we know how to build, so the caller can try the
/// workspace for its declaration and fall back to `Default::default()`.
pub fn known_value(ty: &str) -> Option<String> {
    // `&'a mut str`, `'static str` and `str` are the same type for this purpose, and a caller
    // may already have stripped the reference.
    let mut t = ty.trim();
    loop {
        let before = t;
        t = t.trim_start_matches('&').trim_start();
        if let Some(rest) = t.strip_prefix('\'') {
            t = rest
                .split_once(char::is_whitespace)
                .map(|(_, rest)| rest)
                .unwrap_or("")
                .trim_start();
        }
        t = t.strip_prefix("mut ").unwrap_or(t).trim_start();
        if t == before {
            break;
        }
    }
    let bare = t.rsplit("::").next().unwrap_or(t).trim();
    let simple = bare.split('<').next().unwrap_or(bare).trim();
    let v = match simple {
        "bool" => "false".to_string(),
        "char" => "'a'".to_string(),
        "f32" | "f64" => "0.0".to_string(),
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" => "0".to_string(),
        "String" => "String::new()".to_string(),
        "str" => "\"\"".to_string(),
        "PathBuf" => "std::path::PathBuf::new()".to_string(),
        "Duration" => "std::time::Duration::from_secs(0)".to_string(),
        "Instant" => "std::time::Instant::now()".to_string(),
        "SocketAddr" => "\"127.0.0.1:0\".parse().unwrap()".to_string(),
        "Vec" | "VecDeque" | "HashMap" | "BTreeMap" | "HashSet" | "BTreeSet" | "BinaryHeap" => {
            format!("{simple}::new()")
        }
        "Option" => "None".to_string(),
        "AtomicBool" => "std::sync::atomic::AtomicBool::new(false)".to_string(),
        "AtomicU8" | "AtomicU16" | "AtomicU32" | "AtomicU64" | "AtomicUsize" | "AtomicI32"
        | "AtomicI64" | "AtomicIsize" => format!("std::sync::atomic::{simple}::new(0)"),
        _ => return None,
    };
    Some(v)
}

/// Wrappers whose value is the wrapped value in a constructor.
pub(crate) fn wrapper_value(ty: &str, inner_value: impl FnOnce(&str) -> String) -> Option<String> {
    for (wrapper, ctor) in [
        ("Box", "Box::new"),
        ("Arc", "std::sync::Arc::new"),
        ("Rc", "std::rc::Rc::new"),
        ("Mutex", "std::sync::Mutex::new"),
        ("RwLock", "std::sync::RwLock::new"),
        ("RefCell", "std::cell::RefCell::new"),
        ("Cell", "std::cell::Cell::new"),
    ] {
        if let Some(inner) = inner_of(ty, wrapper) {
            return Some(format!("{ctor}({})", inner_value(inner)));
        }
    }
    None
}

/// Builds the value for one type, asking the analyzer about types it does not know.
pub(crate) async fn value_for(
    remote: SocketAddr,
    root: &Path,
    ty: &str,
    depth: u32,
    indent: usize,
    seen: &mut Vec<String>,
    fallbacks: &mut Vec<String>,
) -> String {
    let t = ty.trim().trim_start_matches('&').trim();
    if t.is_empty() || t == "()" {
        return "()".to_string();
    }
    if let Some(v) = known_value(t) {
        return v;
    }
    if let Some(v) = wrapper_inner(remote, root, t, depth, indent, seen, fallbacks).await {
        return v;
    }
    // A tuple type: build each element.
    if t.starts_with('(') && t.ends_with(')') {
        let mut parts = Vec::new();
        for element in split_top_level(&t[1..t.len() - 1]) {
            if element.trim().is_empty() {
                continue;
            }
            parts.push(
                Box::pin(value_for(
                    remote, root, &element, depth, indent, seen, fallbacks,
                ))
                .await,
            );
        }
        return format!("({})", parts.join(", "));
    }
    let name = t
        .split('<')
        .next()
        .unwrap_or(t)
        .rsplit("::")
        .next()
        .unwrap_or(t)
        .trim();
    if depth == 0 || seen.iter().any(|s| s == name) {
        fallbacks.push(name.to_string());
        return "Default::default()".to_string();
    }
    seen.push(name.to_string());
    let built = build_literal(remote, root, name, depth - 1, indent, seen, fallbacks, None).await;
    seen.pop();
    match built {
        Some(literal) => literal,
        None => {
            fallbacks.push(name.to_string());
            "Default::default()".to_string()
        }
    }
}

/// `Box<T>`, `Arc<T>` and friends, whose inner value has to be built first.
async fn wrapper_inner(
    remote: SocketAddr,
    root: &Path,
    ty: &str,
    depth: u32,
    indent: usize,
    seen: &mut Vec<String>,
    fallbacks: &mut Vec<String>,
) -> Option<String> {
    for wrapper in ["Box", "Arc", "Rc", "Mutex", "RwLock", "RefCell", "Cell"] {
        if let Some(inner) = inner_of(ty, wrapper) {
            let inner_value = Box::pin(value_for(
                remote, root, inner, depth, indent, seen, fallbacks,
            ))
            .await;
            return wrapper_value(ty, |_| inner_value);
        }
    }
    None
}

/// The literal for a type declared in the workspace, or `None` when it cannot be found.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_literal(
    remote: SocketAddr,
    root: &Path,
    name: &str,
    depth: u32,
    indent: usize,
    seen: &mut Vec<String>,
    fallbacks: &mut Vec<String>,
    hint: Option<&Path>,
) -> Option<String> {
    let (shape, _path) = shape_of(remote, root, name, hint).await.ok()?;
    let pad = "    ".repeat(indent + 1);
    let closing = "    ".repeat(indent);
    match shape {
        Shape::Unit => Some(name.to_string()),
        Shape::Enum(variants) => variants.first().map(|v| format!("{name}::{v}")),
        Shape::Tuple(types) => {
            let mut parts = Vec::new();
            for ty in types {
                parts.push(
                    Box::pin(value_for(remote, root, &ty, depth, indent, seen, fallbacks)).await,
                );
            }
            Some(format!("{name}({})", parts.join(", ")))
        }
        Shape::Record(fields) => {
            if fields.is_empty() {
                return Some(format!("{name} {{}}"));
            }
            let mut lines = Vec::new();
            for (field, ty) in fields.iter().take(MANY_FIELDS) {
                let value = Box::pin(value_for(
                    remote,
                    root,
                    ty,
                    depth,
                    indent + 1,
                    seen,
                    fallbacks,
                ))
                .await;
                lines.push(format!("{pad}{field}: {value},"));
            }
            Some(format!("{name} {{\n{}\n{closing}}}", lines.join("\n")))
        }
    }
}
