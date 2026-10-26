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
//! A header that may is still refused (no semantic declaration identity); only one proven to
//! declare a function of another namespace no longer blocks the change (#1382). Anything this
//! scan cannot read with certainty counts as "may declare" (#1546).

/// The namespaces open at byte `at`, outermost first and split at `::`: `namespace a::b {` and
/// `namespace a { namespace b {` both give `[a, b]`. `None` when the scan cannot be sure:
/// an anonymous or inline namespace, a raw string, a namespace macro, a namespace or brace
/// inside a conditional block, or an unbalanced `}`. Comments and literals are skipped, and
/// every step stays on bytes, so non-ASCII text anywhere is read safely (#1547).
pub(super) fn namespaces_at(text: &str, at: usize) -> Option<Vec<String>> {
    let bytes = text.as_bytes();
    let end = at.min(bytes.len());
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let mut open: Vec<Option<Vec<String>>> = Vec::new();
    let mut pending: Option<Vec<String>> = None;
    // `#if` nesting that is not the file's include guard.
    let mut conditional = 0usize;
    let mut guard_candidate: Option<String> = None;
    let mut i = 0;
    while i < end {
        let b = bytes[i];
        if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < end && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < end && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i += 2;
            continue;
        }
        if b == b'"' {
            if i > 0 && bytes[i - 1] == b'R' {
                return None;
            }
            i += 1;
            while i < end && bytes[i] != b'"' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            // A literal still open at the declaration was misread: no namespace is proven.
            if i >= end {
                return None;
            }
            i += 1;
            continue;
        }
        // A character literal, also prefixed (`L'}'`, `u8'}'`), but not a digit separator as
        // in `1'000` or `0xFF'FF`: that `'` follows a word that starts with a digit (#1552).
        // The walk back crosses earlier separators, so `0xAB'CD'EF` stays one number (#1556).
        let separator = || {
            let word_start = bytes[..i]
                .iter()
                .rposition(|c| !(c.is_ascii_alphanumeric() || *c == b'_' || *c == b'\''))
                .map_or(0, |p| p + 1);
            word_start < i && bytes[word_start].is_ascii_digit()
        };
        if b == b'\'' && !separator() {
            i += 1;
            while i < end && bytes[i] != b'\'' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            if i >= end {
                return None;
            }
            i += 1;
            continue;
        }
        if b == b'#'
            && bytes[..i]
                .iter()
                .rev()
                .take_while(|c| **c != b'\n')
                .all(|c| c.is_ascii_whitespace())
        {
            let line_end = (i..bytes.len())
                .find(|&j| bytes[j] == b'\n' && bytes[j - 1] != b'\\')
                .unwrap_or(bytes.len());
            let directive = String::from_utf8_lossy(&bytes[i + 1..line_end]);
            let mut words = directive.split_whitespace();
            let (kind, name) = (words.next(), words.next());
            // `#ifndef X` straight followed by `#define X` is an include guard, not a branch.
            if let Some(candidate) = guard_candidate.take()
                && kind == Some("define")
                && name == Some(candidate.as_str())
            {
                conditional -= 1;
            }
            match kind {
                Some("ifndef") => {
                    conditional += 1;
                    guard_candidate = name.map(str::to_string);
                }
                Some("if" | "ifdef") => conditional += 1,
                Some("endif") => conditional = conditional.saturating_sub(1),
                _ => {}
            }
            i = line_end;
            continue;
        }
        if ident(b) && (i == 0 || !ident(bytes[i - 1])) {
            let word_end = (i..bytes.len())
                .find(|&j| !ident(bytes[j]))
                .unwrap_or(bytes.len());
            let word = &text[i..word_end];
            if word.to_ascii_lowercase().contains("namespace") && word != "namespace" {
                return None;
            }
            match word {
                "namespace" => {
                    if conditional > 0 {
                        return None;
                    }
                    let mut j = word_end;
                    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    let name_end = (j..bytes.len())
                        .find(|&k| !(ident(bytes[k]) || bytes[k] == b':'))
                        .unwrap_or(bytes.len());
                    let segments: Vec<String> =
                        text[j..name_end].split("::").map(str::to_string).collect();
                    pending = Some(segments);
                    i = name_end;
                    continue;
                }
                "inline" => {
                    let rest = text[word_end..].trim_start();
                    if rest.starts_with("namespace") {
                        return None;
                    }
                }
                _ => {}
            }
            i = word_end;
            continue;
        }
        match b {
            b'{' => {
                if conditional > 0 {
                    return None;
                }
                let scope = pending.take();
                if scope
                    .as_ref()
                    .is_some_and(|s| s.iter().any(String::is_empty))
                {
                    return None;
                }
                open.push(scope);
            }
            b'}' => {
                if conditional > 0 || open.pop().is_none() {
                    return None;
                }
            }
            b';' => pending = None,
            _ => {}
        }
        i += 1;
    }
    Some(open.into_iter().flatten().flatten().collect())
}

/// Whether the header may declare the function named `name` at `def_at` in the source. It
/// may not only when every whole-word `name` in the header sits in namespaces this scan reads
/// with certainty, all of them differ from the definition's, and no name is qualified
/// (`int other::f()` defines `other::f`).
pub(super) fn header_may_declare(source: &str, def_at: usize, header: &str, name: &str) -> bool {
    let qualified = |text: &str, at: usize| text[..at].trim_end().ends_with("::");
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let Some(definition) = namespaces_at(source, def_at).filter(|_| !qualified(source, def_at))
    else {
        return true;
    };
    header
        .match_indices(name)
        .filter(|(at, _)| {
            !header[..*at].chars().next_back().is_some_and(ident)
                && !header[at + name.len()..].chars().next().is_some_and(ident)
        })
        .any(|(at, _)| {
            qualified(header, at) || namespaces_at(header, at).is_none_or(|ns| ns == definition)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, needle: &str) -> Option<Vec<String>> {
        namespaces_at(text, text.find(needle).unwrap())
    }

    #[test]
    fn namespaces_are_tracked_through_nesting_spellings_and_closing() {
        let text = "namespace a { namespace b { int f(); } int g(); }\nint h();\nnamespace a::b { int k(); }";
        assert_eq!(at(text, "int f").unwrap(), ["a", "b"]);
        assert_eq!(at(text, "int g").unwrap(), ["a"]);
        assert!(at(text, "int h").unwrap().is_empty());
        // `a::b` and nested `a { b {` are the same namespace (#1546).
        assert_eq!(at(text, "int k").unwrap(), ["a", "b"]);
        // `using namespace`, aliases, classes and `extern "C"` open no namespace.
        let text = "using namespace std;\nnamespace fs = std::filesystem;\nextern \"C\" { struct S { int x; }; }\nint m();";
        assert!(at(text, "int m").unwrap().is_empty());
    }

    #[test]
    fn comments_literals_and_unicode_do_not_move_the_scan() {
        // A multibyte character before the declaration is read, not sliced through (#1547).
        let text = "// café — namespace x {\nint selected(int x) { return x; }";
        assert!(at(text, "int selected").unwrap().is_empty());
        let text = "/* namespace x { */ const char* s = \"namespace y {\"; char c = '{'; int n = 1'000;\nnamespace é { int z(); }";
        assert_eq!(at(text, "int z").unwrap(), ["é"]);
        // Prefixed character literals are literals, digit separators are not (#1552).
        let text = "namespace a {\nconst wchar_t m = L'}';\nchar16_t n = u'}'; char8_t o = u8'}';\nint h = 0xFF'FF; int k = 1'0; int m = 0xAB'CD'EF; int q = 1'000'000;\nint selected(int x);\n}";
        assert_eq!(at(text, "int selected").unwrap(), ["a"]);
    }

    #[test]
    fn what_the_scan_cannot_be_sure_of_is_none() {
        assert!(at("namespace { int f(); }", "int f").is_none());
        assert!(at("namespace a { inline namespace v1 { int f(); } }", "int f").is_none());
        assert!(at("BEGIN_NAMESPACE(a)\nint f();", "int f").is_none());
        assert!(at("#ifdef X\nnamespace a {\n#endif\nint f();", "int f").is_none());
        assert!(at("const char* s = R\"(x)\";\nint f();", "int f").is_none());
        assert!(at("}\nint f();", "int f").is_none());
        // An include guard is not a conditional branch.
        let guarded = "#ifndef API_H\n#define API_H\nnamespace other { int f(); }\n#endif\n";
        assert_eq!(at(guarded, "int f").unwrap(), ["other"]);
    }

    #[test]
    fn only_a_proven_other_namespace_declares_another_function() {
        let source = "#include \"public.hpp\"\nint selected(int x) { return x; }\n";
        let def = source.find("selected(").unwrap();
        let other = "namespace other { int selected(int x); }\n";
        // `other::selected` is another function (#1382).
        assert!(!header_may_declare(source, def, other, "selected"));
        // A global prototype may be the declaration even when the header is reached only
        // transitively or has another name; that stays refused (#1546).
        let api = "#ifndef API_H\n#define API_H\nint selected(int x);\n#endif\n";
        assert!(header_may_declare(source, def, api, "selected"));
        // Equivalent namespace spellings match.
        let nested = "namespace a { namespace b { int selected(int x); } }";
        let flat = "namespace a::b {\nint selected(int x) { return x; }\n}";
        assert!(header_may_declare(
            flat,
            flat.find("selected").unwrap(),
            nested,
            "selected"
        ));
        // A qualified definition names its namespace itself.
        let qualified = "int other::selected(int x) { return x; }";
        assert!(header_may_declare(
            qualified,
            qualified.find("selected").unwrap(),
            other,
            "selected"
        ));
        // Uncertain scans may declare.
        let anonymous = "namespace { int selected(int x); }";
        assert!(header_may_declare(source, def, anonymous, "selected"));
        // The issue's fixture: the brace of `L'}'` must not close `namespace a` (#1552).
        let api = "namespace a { int selected(int x); }\n";
        let compute = "#include \"api.hpp\"\nnamespace a {\nconst wchar_t marker = L'}';\nint selected(int x) { return x; }\n}\n";
        assert!(header_may_declare(
            compute,
            compute.find("int selected").unwrap() + 4,
            api,
            "selected"
        ));
        // The issue's fixture: the second separator of `0xAB'CD'EF` opens no literal, so the
        // definition sits in `b` like the header's prototype (#1556).
        let api = "namespace b { int selected(int x); }\n";
        let compute = "#include \"api.hpp\"\nnamespace a {\nconstexpr unsigned marker = 0xAB'CD'EF;\n}\nnamespace b {\nint selected(int x) { return x; }\n}\n";
        assert!(header_may_declare(
            compute,
            compute.find("int selected").unwrap() + 4,
            api,
            "selected"
        ));
        assert_eq!(
            namespaces_at(compute, compute.find("int selected").unwrap()).unwrap(),
            ["b"]
        );
        // A literal still open at the declaration proves nothing.
        assert!(namespaces_at("char c = '{;\nint f();", 15).is_none());
        // One of several same-named declarations may be it: the first is not the only one.
        let both = "namespace other { int selected(int x); }\nint selected(int x);\n";
        assert!(header_may_declare(source, def, both, "selected"));
    }
}
