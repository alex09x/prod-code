/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Whether a C/C++ header's same-named prototype may declare the function being generified.
//! A header that may is still refused (no semantic declaration identity); one that provably
//! cannot no longer blocks the change (#1382).

use std::path::Path;

/// The namespaces open at byte `at`, outermost first: `namespace a { namespace b {` → `[a, b]`.
/// Anonymous namespaces count as one named `""`.
pub(super) fn namespaces_at(text: &str, at: usize) -> Vec<String> {
    let mut open: Vec<Option<String>> = Vec::new();
    let mut pending: Option<String> = None;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < at.min(text.len()) {
        if text[i..].starts_with("namespace")
            && (i == 0 || !(bytes[i - 1] as char).is_alphanumeric() && bytes[i - 1] != b'_')
            && text[i + 9..].starts_with(|c: char| c.is_whitespace() || c == '{')
        {
            let rest = &text[i + 9..];
            let name: String = rest
                .trim_start()
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                .collect();
            pending = Some(name);
            i += 9;
            continue;
        }
        match bytes[i] {
            b'{' => open.push(pending.take()),
            b'}' => {
                open.pop();
            }
            b';' => pending = None,
            _ => {}
        }
        i += 1;
    }
    open.into_iter().flatten().collect()
}

/// `source` includes a header with this file name: `#include "x.h"` or `<dir/x.h>`.
fn includes(source: &str, header: &Path) -> bool {
    let Some(name) = header.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    source.lines().any(|line| {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix('#') else {
            return false;
        };
        let Some(spec) = rest.trim_start().strip_prefix("include") else {
            return false;
        };
        let spec = spec.trim().trim_matches(|c| matches!(c, '"' | '<' | '>'));
        spec.rsplit('/').next() == Some(name)
    })
}

/// Whether the header prototype at `proto_at` may be the declaration of the function defined
/// at `def_at` in `source`. It cannot when it sits in other namespaces than the definition, or
/// when the source neither includes the header nor shares its stem.
pub(super) fn header_may_declare(
    source_path: &Path,
    source: &str,
    def_at: usize,
    header_path: &Path,
    header: &str,
    proto_at: usize,
) -> bool {
    if namespaces_at(header, proto_at) != namespaces_at(source, def_at) {
        return false;
    }
    let same_stem = source_path.file_stem() == header_path.file_stem();
    same_stem || includes(source, header_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_are_tracked_through_nesting_and_closing() {
        let text = "namespace a { namespace b { int f(); } int g(); }\nint h();";
        assert_eq!(namespaces_at(text, text.find("int f").unwrap()), ["a", "b"]);
        assert_eq!(namespaces_at(text, text.find("int g").unwrap()), ["a"]);
        assert!(namespaces_at(text, text.find("int h").unwrap()).is_empty());
        let anonymous = "namespace { int k(); }";
        assert_eq!(
            namespaces_at(anonymous, anonymous.find("int k").unwrap()),
            [""]
        );
    }

    #[test]
    fn an_unrelated_namespace_or_unincluded_header_cannot_declare_it() {
        let source = "int selected(int x) { return x; }\nint use() { return selected(1); }\n";
        let at = source.find("int selected").unwrap();
        let other = "namespace other { int selected(int x); }\n";
        let other_at = other.find("int selected").unwrap();
        // The bare spelling matches, but `other::selected` is another function (#1382).
        assert!(!header_may_declare(
            Path::new("src/value.cpp"),
            source,
            at,
            Path::new("include/value.h"),
            other,
            other_at
        ));
        let global = "int selected(int x);\n";
        // Same namespace, same stem: may be its declaration, so the caller still refuses.
        assert!(header_may_declare(
            Path::new("src/value.cpp"),
            source,
            at,
            Path::new("include/value.h"),
            global,
            0
        ));
        // Same namespace but neither included nor of the same stem.
        assert!(!header_may_declare(
            Path::new("src/value.cpp"),
            source,
            at,
            Path::new("include/unrelated.h"),
            global,
            0
        ));
        let including = format!("#include \"unrelated.h\"\n{source}");
        assert!(header_may_declare(
            Path::new("src/value.cpp"),
            &including,
            including.find("int selected").unwrap(),
            Path::new("include/unrelated.h"),
            global,
            0
        ));
    }
}
