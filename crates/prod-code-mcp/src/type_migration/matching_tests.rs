/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn coverage_w2_matching_enclosing_functions_keeps_exact_return_spans() {
    use Language::*;
    for (lang, text, name, ty) in [
        (
            Rust,
            "fn load(x: u8) -> Vec<u8> {\n    marker\n}",
            "load",
            "Vec<u8>",
        ),
        (
            Swift,
            "func load() -> [Int] {\n    marker\n}",
            "load",
            "[Int]",
        ),
        (
            Python,
            "def load() -> list[int] :\n    marker\n",
            "load",
            "list[int]",
        ),
        (
            TypeScript,
            "function load(): number {\n    marker\n}",
            "load",
            "number",
        ),
        (
            JavaScript,
            "function load(): number {\n    marker\n}",
            "load",
            "number",
        ),
        (
            Go,
            "func load() (int, error) {\n    marker\n}",
            "load",
            "(int, error)",
        ),
        (Cpp, "long load() {\n    marker;\n}", "load", "long"),
    ] {
        let enclosing = find_enclosing_fn(text, text.find("marker").unwrap(), lang).unwrap();
        assert_eq!(enclosing.name, name);
        assert_eq!(enclosing.return_type, ty);
        assert_eq!(&text[enclosing.ret_start..enclosing.ret_end], ty);
    }
    assert!(find_enclosing_fn("load() { marker; }", 9, Cpp).is_none());
    assert!(find_enclosing_fn("marker", 0, Rust).is_none());
}

#[test]
fn coverage_w2_matching_vars_rejects_comments_wrong_types_and_partial_names() {
    for (lang, text) in [
        (
            Language::Python,
            "# copied: int = seed\ninvalid name: int = seed\n1bad: int = seed\ncopied: int = seed\nwrong: str = seed\nother: int = seedling\n",
        ),
        (
            Language::Go,
            "// var copied int = seed\nvar copied int = seed\nvar other string = seed\n",
        ),
        (
            Language::Java,
            "// int copied = seed;\nint copied = seed;\nString other = seed;\n",
        ),
    ] {
        let matches = find_matching_vars(text, "seed", "int", lang);
        assert_eq!(matches.len(), 1, "{text}");
        let (start, end, name) = &matches[0];
        assert_eq!(name, "copied");
        assert_eq!(&text[*start..*end], "int");
    }
    for (text, expected) in [
        ("seed", true),
        ("αseed", false),
        ("seed_", false),
        ("(seed)", true),
    ] {
        assert_eq!(contains_ident(text, "seed"), expected);
    }
}
