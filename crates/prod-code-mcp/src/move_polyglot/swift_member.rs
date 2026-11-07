/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A Swift declaration that sits inside braces is a member of a type, an extension or a
//! function. Moved alone to another file it becomes a free declaration outside its type: its
//! `self` and its properties no longer resolve and a `private` one is no longer reachable, yet the
//! preview reported zero errors (#1978). The move refuses it instead.

/// Whether the declaration that starts on the 1-based `line` of `source` is inside braces.
pub(super) fn is_member_at_line(source: &str, line: u32) -> bool {
    let offset: usize = source
        .split_inclusive('\n')
        .take(line.saturating_sub(1) as usize)
        .map(str::len)
        .sum();
    is_inside_braces(source, offset)
}

/// Whether the declaration that starts at byte `decl_start` of `source` is inside braces.
/// Only braces in code count: those in comments (nested ones too) and in string literals (raw
/// `#"…"#`, multi-line `"""…"""` and interpolated ones) are left out, by the same lexical mask the
/// caller migration uses.
pub(super) fn is_inside_braces(source: &str, decl_start: usize) -> bool {
    let end = decl_start.min(source.len());
    let mask = crate::caller_migration::mask::lexical_code_mask(
        source,
        crate::parameter_object::Language::Swift,
    );
    let depth: i64 = source.as_bytes()[..end]
        .iter()
        .zip(&mask)
        .map(|(byte, code)| match (*code, *byte) {
            (true, b'{') => 1,
            (true, b'}') => -1,
            _ => 0,
        })
        .sum();
    depth > 0
}

#[cfg(test)]
mod tests {
    use super::{is_inside_braces, is_member_at_line};

    fn start_of(source: &str, needle: &str) -> usize {
        source
            .find(needle)
            .expect("the declaration is in the source")
    }

    #[test]
    fn a_method_of_a_class_or_extension_is_inside_braces() {
        let source = "import Foundation\n\nclass Example {\n    init() { setup() }\n    private func setup() { self.value = 1 }\n    var value = 0\n}\n";
        assert!(is_inside_braces(
            source,
            start_of(source, "private func setup")
        ));
        let extension = "extension Example {\n    func more() {}\n}\n";
        assert!(is_inside_braces(
            extension,
            start_of(extension, "func more")
        ));
        let nested = "struct A {\n    struct B {\n        func f() {}\n    }\n}\n";
        assert!(is_inside_braces(nested, start_of(nested, "func f")));
    }

    #[test]
    fn a_declaration_is_found_by_its_line() {
        let source = "class Example {\n    private func setup() {}\n}\n\nfunc free() {}\n";
        assert!(is_member_at_line(source, 2), "the method is on line 2");
        assert!(
            !is_member_at_line(source, 5),
            "the free function is on line 5"
        );
        assert!(
            !is_member_at_line(source, 1),
            "the class itself is not inside braces"
        );
    }

    #[test]
    fn a_top_level_declaration_after_other_code_is_not() {
        let source = "class Example {\n    func a() {}\n}\n\nfunc free() {}\n";
        assert!(!is_inside_braces(source, start_of(source, "func free")));
        assert!(!is_inside_braces("func only() {}\n", 0));
    }

    /// Raw, multi-line and interpolated strings and nested comments (#1978 review): a brace in one
    /// is not code, whichever way it falls.
    #[test]
    fn braces_in_raw_multiline_and_interpolated_strings_and_nested_comments_are_not_counted() {
        let member = "class A { let q = #\"a\"}\"#; private func m() {} }\n";
        assert!(is_inside_braces(member, start_of(member, "private func")));
        for (source, decl) in [
            ("let r = #\"a\"{\"#\nfunc free() {}\n", "func free"),
            ("let m = \"\"\"\n\"{\n\"\"\"\nfunc free() {}\n", "func free"),
            ("let s = \"x: \\(d[\"{\"])\"\nfunc free() {}\n", "func free"),
            ("/* a /* b */ { */\nfunc free() {}\n", "func free"),
        ] {
            assert!(
                !is_inside_braces(source, start_of(source, decl)),
                "{source:?}"
            );
        }
    }

    #[test]
    fn a_brace_in_a_comment_or_a_string_is_not_counted() {
        let source = "// {\n/* { { */\nlet s = \"{ \\\" {\"\nfunc free() {}\n";
        assert!(!is_inside_braces(source, start_of(source, "func free")));
        let member = "class A {\n    let s = \"}\"\n    func m() {}\n}\n";
        assert!(is_inside_braces(member, start_of(member, "func m")));
    }
}
