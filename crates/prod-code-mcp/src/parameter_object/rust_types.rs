/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// The type of a parameter as the declaration writes it: everything after the first top-level
/// colon, trimmed.
pub fn type_of(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' | b')' | b']' => depth -= 1,
            b':' if depth == 0 => {
                // `::` is a path separator, not the end of the name.
                if bytes.get(i + 1) == Some(&b':') {
                    i += 2;
                    continue;
                }
                return Some(raw[i + 1..].trim());
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether a type borrows without naming a lifetime, so the struct that holds it needs one.
pub fn needs_lifetime(ty: &str) -> bool {
    for (i, c) in ty.char_indices() {
        if c != '&' {
            continue;
        }
        if !ty[i + 1..].trim_start().starts_with('\'') {
            return true;
        }
    }
    false
}

/// The same type with every anonymous borrow tied to `'a`.
pub fn with_lifetime(ty: &str) -> String {
    let mut out = String::with_capacity(ty.len() + 4);
    let mut rest = ty;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        out.push('&');
        let after = &rest[at + 1..];
        let trimmed = after.trim_start();
        if trimmed.starts_with('\'') {
            out.push_str(after);
            return out;
        }
        out.push_str("'a ");
        rest = trimmed;
    }
    out.push_str(rest);
    out
}

/// The struct that holds the bundled parameters, as it will be written.
///
/// One field per parameter, in the order the declaration had them, with the type the
/// declaration gave. A single lifetime is introduced when any of those types borrows.
pub fn struct_text(name: &str, fields: &[(String, String)], doc: &str) -> String {
    let borrows = fields.iter().any(|(_, ty)| needs_lifetime(ty));
    let generics = if borrows { "<'a>" } else { "" };
    let mut out = String::new();
    if !doc.is_empty() {
        out.push_str(&format!("/// {doc}\n"));
    }
    out.push_str(&format!("pub struct {name}{generics} {{\n"));
    for (field, ty) in fields {
        let ty = if borrows {
            with_lifetime(ty)
        } else {
            ty.clone()
        };
        out.push_str(&format!("    pub {field}: {ty},\n"));
    }
    out.push_str("}\n");
    out
}

/// How the bundled parameter is spelled in the new declaration.
pub fn parameter_text(binding: &str, name: &str, fields: &[(String, String)]) -> String {
    let borrows = fields.iter().any(|(_, ty)| needs_lifetime(ty));
    if borrows {
        format!("{binding}: {name}<'_>")
    } else {
        format!("{binding}: {name}")
    }
}
