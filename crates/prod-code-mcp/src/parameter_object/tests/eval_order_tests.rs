/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::binding::bind_arguments;
use super::super::effects::reordered_arguments;
use super::super::params::parse_params;
use super::super::types::Language;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_bundled_argument_is_not_moved_past_one_that_may_change_it() {
    let (_, params) = parse_params("a, b, c", Language::JavaScript);
    let order = |args: &[&str], bundled: &[usize]| {
        let args = strings(args);
        let bound = bind_arguments(&args, &params, Language::JavaScript).expect("bound");
        reordered_arguments(&args, &bound, bundled, &params, Language::JavaScript)
            .map(|(m, p)| (m.to_string(), p.to_string()))
    };
    let mark = ["mark(\"a\")", "mark(\"b\")", "mark(\"c\")"];
    assert_eq!(
        order(&mark, &[0, 2]),
        Some(("mark(\"c\")".to_string(), "mark(\"b\")".to_string())),
        "the object would evaluate `c` before `b`"
    );
    assert_eq!(order(&mark, &[0, 1]), None, "adjacent: nothing moves");
    assert_eq!(order(&mark, &[1, 2]), None, "adjacent: nothing moves");
    // Each may be a getter on `globalThis`: the spelling of a name proves nothing (#436).
    assert_eq!(
        order(&["x", "y", "z"], &[0, 2]),
        Some(("z".to_string(), "y".to_string())),
        "a JavaScript name may run a getter"
    );
    assert_eq!(order(&["x", "y", "z"], &[1, 2]), None, "adjacent names");
    assert_eq!(order(&["1", "mark(\"b\")", "\"c\""], &[0, 2]), None);
    assert_eq!(order(&["mark(\"a\")", "b", "3"], &[0, 2]), None);
    assert!(
        order(&["x", "mark(\"b\")", "z"], &[0, 2]).is_some(),
        "b may set z"
    );
    assert!(
        order(&["x", "b", "next()"], &[0, 2]).is_some(),
        "next() may set b"
    );
    assert!(order(&["x", "b", "1..missing"], &[0, 2]).is_some());

    // Rust and the others are positional too; Python's keywords are compared by value.
    let (_, py) = parse_params("a, b, c", Language::Python);
    let args = strings(&["mark(1)", "c=mark(3)", "b=2"]);
    let bound = bind_arguments(&args, &py, Language::Python).expect("bound");
    assert_eq!(
        reordered_arguments(&args, &bound, &[0, 1], &py, Language::Python),
        None,
        "a keyword argument whose value is a literal"
    );
    let args = strings(&["mark(1)", "b=mark(2)", "c=mark(3)"]);
    let bound = bind_arguments(&args, &py, Language::Python).expect("bound");
    assert!(reordered_arguments(&args, &bound, &[0, 2], &py, Language::Python).is_some());
    let rust = strings(&["a()", "b()", "c()"]);
    let bound: Vec<Option<usize>> = (0..3).map(Some).collect();
    assert!(reordered_arguments(&rust, &bound, &[0, 2], &[], Language::Rust).is_some());
    assert_eq!(
        reordered_arguments(&rust, &bound, &[1, 2], &[], Language::Rust),
        None
    );

    // Reading a name runs nothing in Rust and Go; anywhere else it may.
    let names = strings(&["x", "y", "z"]);
    for language in [Language::Rust, Language::Go] {
        assert_eq!(
            reordered_arguments(&names, &bound, &[0, 2], &[], language),
            None,
            "{language:?}"
        );
    }
    for (language, list) in [
        (Language::Python, "a, b, c"),
        (Language::TypeScript, "a: number, b: number, c: number"),
        (Language::Swift, "_ a: Int, _ b: Int, _ c: Int"),
        (Language::C, "int a, int b, int c"),
        (Language::Cpp, "int a, int b, int c"),
    ] {
        let (_, params) = parse_params(list, language);
        let bound = bind_arguments(&names, &params, language).expect("bound");
        assert_eq!(
            reordered_arguments(&names, &bound, &[0, 2], &params, language),
            Some(("z", "y")),
            "{language:?}: a plain name is no proof"
        );
        assert_eq!(
            reordered_arguments(&names, &bound, &[1, 2], &params, language),
            None,
            "{language:?}: adjacent arguments keep their order"
        );
    }
}

#[test]
fn a_literal_counts_as_inert_only_where_the_language_converts_it() {
    let moved = |list: &str, language: Language, args: &[&str]| {
        let (_, params) = parse_params(list, language);
        let args = strings(args);
        let bound = bind_arguments(&args, &params, language).expect("bound");
        reordered_arguments(&args, &bound, &[0, 2], &params, language).map(|(m, _)| m.to_string())
    };
    // C++: a built-in parameter type takes a literal as it is; a class runs a converting
    // constructor, and a suffix of the program's calls its `operator""`.
    let cpp = Language::Cpp;
    assert_eq!(
        moved("int a, int b, int c", cpp, &["a()", "b()", "3"]),
        None
    );
    assert_eq!(
        moved("int a, int b, const char *c", cpp, &["a()", "b()", "\"c\""]),
        None
    );
    assert_eq!(
        moved("int a, int b, unsigned long &c", cpp, &["a()", "b()", "3"]),
        None
    );
    assert_eq!(
        moved("int a, int b, int c", cpp, &["a()", "b()", "12_km"]),
        Some("12_km".to_string())
    );
    assert_eq!(
        moved("int a, int b, Meters c", cpp, &["a()", "b()", "3"]),
        Some("3".to_string())
    );
    assert_eq!(
        moved("int a, int b, std::string c", cpp, &["a()", "b()", "\"c\""]),
        Some("\"c\"".to_string())
    );
    assert_eq!(
        moved("int a, int b, int c", cpp, &["a()", "b()", "\"c\"_s"]),
        Some("\"c\"_s".to_string())
    );
    // C has no literal operators or constructors, and a `_` separates nothing there either.
    assert_eq!(
        moved("int a, int b, int c", Language::C, &["a()", "b()", "3u"]),
        None
    );

    // Swift: the standard library's own literal types, optional or not; a type of the
    // program's runs its `init(…Literal:)`, and a substitution is code.
    let swift = Language::Swift;
    assert_eq!(
        moved("_ a: Int, _ b: Int, _ c: Int", swift, &["a()", "b()", "3"]),
        None
    );
    assert_eq!(
        moved(
            "_ a: Int, _ b: Int, c: String?",
            swift,
            &["a()", "b()", "c: \"x\""]
        ),
        None
    );
    assert_eq!(
        moved(
            "_ a: Int, _ b: Int, _ c: Meters",
            swift,
            &["a()", "b()", "3"]
        ),
        Some("3".to_string())
    );
    assert_eq!(
        moved(
            "_ a: Int, _ b: Int, _ c: String",
            swift,
            &["a()", "b()", "\"\\(b)\""]
        ),
        Some("\"\\(b)\"".to_string())
    );

    // JavaScript and Python: a substitution or a prefix makes a string code.
    assert_eq!(
        moved("a, b, c", Language::JavaScript, &["a()", "b()", "`${x}`"]),
        Some("`${x}`".to_string())
    );
    assert_eq!(
        moved("a, b, c", Language::Python, &["a()", "b()", "f\"{x}\""]),
        Some("f\"{x}\"".to_string())
    );
    assert_eq!(
        moved("a, b, c", Language::Python, &["a()", "b()", "\"{x}\""]),
        None
    );
}
