/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::super::binding::call_args_span;
use super::super::rewrite::rewritten_args;
use super::super::rust_types::{
    needs_lifetime, parameter_text, struct_text, type_of, with_lifetime,
};
use super::super::syntax::{matching_bracket, split_args};
use super::super::types::{Language, default_binding};

#[test]
fn a_parameters_type_is_what_follows_its_first_top_level_colon() {
    assert_eq!(type_of("name: &str"), Some("&str"));
    assert_eq!(
        type_of("map: BTreeMap<String, Vec<u8>>"),
        Some("BTreeMap<String, Vec<u8>>")
    );
    assert_eq!(type_of("p: std::path::PathBuf"), Some("std::path::PathBuf"));
    assert_eq!(type_of("self"), None);
}

#[test]
fn a_borrow_without_a_lifetime_makes_the_struct_take_one() {
    assert!(needs_lifetime("&str"));
    assert!(needs_lifetime("&[u8]"));
    assert!(!needs_lifetime("String"));
    assert!(!needs_lifetime("&'static str"));
    assert_eq!(with_lifetime("&str"), "&'a str");
    assert_eq!(with_lifetime("&[u8]"), "&'a [u8]");
    assert_eq!(with_lifetime("String"), "String");
}

#[test]
fn the_struct_keeps_the_declared_types_and_the_field_order() {
    let fields = vec![
        ("text".to_string(), "&str".to_string()),
        ("count".to_string(), "usize".to_string()),
    ];
    let text = struct_text("Opts", &fields, "The parameters `f` takes together.");
    assert_eq!(
        text,
        "/// The parameters `f` takes together.\npub struct Opts<'a> {\n    pub text: &'a str,\n    pub count: usize,\n}\n"
    );
    assert_eq!(parameter_text("opts", "Opts", &fields), "opts: Opts<'_>");

    let owned = vec![("count".to_string(), "usize".to_string())];
    assert_eq!(
        struct_text("Opts", &owned, ""),
        "pub struct Opts {\n    pub count: usize,\n}\n"
    );
    assert_eq!(parameter_text("opts", "Opts", &owned), "opts: Opts");
}

#[test]
fn a_bracket_in_a_comment_or_a_literal_does_not_close_the_block() {
    let block = "impl A {\n    // don't stop at } here\n    /* nor } here */\n    fn f() -> &'static str { \"}\" }\n    fn g() -> char { '}' }\n}\ntail";
    let close = matching_bracket(block, block.find('{').unwrap()).expect("it closes");
    assert_eq!(&block[close..], "}\ntail");
    assert_eq!(
        matching_bracket("(a, [b)", 0),
        None,
        "an unclosed list has no end"
    );
    assert_eq!(
        matching_bracket("x", 0),
        None,
        "only a bracket opens a block"
    );
}

#[test]
fn an_argument_list_survives_closures_strings_and_chains() {
    let call = "f(a, |x, y| x + y, \"one, two\", b.iter().map(|v| v).collect())";
    let (start, end) = call_args_span(call, 1).expect("a call");
    let args = split_args(&call[start..end]);
    assert_eq!(
        args,
        vec![
            "a",
            "|x, y| x + y",
            "\"one, two\"",
            "b.iter().map(|v| v).collect()"
        ]
    );
    assert!(
        call_args_span("let g = f;", 8).is_none(),
        "a use that is not a call has no argument list"
    );
}

#[test]
fn the_bundled_arguments_become_one_literal_where_the_first_of_them_was() {
    let fields = vec![
        ("b".to_string(), "u8".to_string()),
        ("c".to_string(), "u8".to_string()),
    ];
    let args: Vec<String> = ["w", "x", "y", "z"].iter().map(|s| s.to_string()).collect();
    assert_eq!(
        rewritten_args(&args, &[1, 2], "Opts", &fields),
        "w, Opts { b: x, c: y }, z"
    );
    assert_eq!(
        rewritten_args(&args, &[0, 3], "Opts", &fields),
        "Opts { b: w, c: z }, x, y"
    );
    // A file that cannot import it names it in full.
    assert_eq!(
        rewritten_args(&args, &[1, 2], "the_crate::home::Opts", &fields),
        "w, the_crate::home::Opts { b: x, c: y }, z"
    );
    // A variable named like its field goes in shorthand (#342).
    let named: Vec<String> = ["w", "b", "self.c", "z"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        rewritten_args(&named, &[1, 2], "Opts", &fields),
        "w, Opts { b, c: self.c }, z"
    );
}

#[test]
fn the_language_is_the_files_and_javascript_is_its_own() {
    assert_eq!(Language::of(Path::new("a/b.rs")), Some(Language::Rust));
    assert_eq!(
        Language::of(Path::new("src/home.ts")),
        Some(Language::TypeScript)
    );
    assert_eq!(
        Language::of(Path::new("src/view.tsx")),
        Some(Language::TypeScript)
    );
    assert_eq!(
        Language::of(Path::new("app/home.py")),
        Some(Language::Python)
    );
    assert_eq!(
        Language::of(Path::new("shapes/home.go")),
        Some(Language::Go)
    );
    assert_eq!(
        Language::of(Path::new("src/home.js")),
        Some(Language::JavaScript)
    );
    assert_eq!(
        Language::of(Path::new("src/view.jsx")),
        Some(Language::JavaScript)
    );
    assert_eq!(
        Language::of(Path::new("lib/home.cjs")),
        Some(Language::JavaScript)
    );
    assert_eq!(Language::of(Path::new("notes.txt")), None);
    assert_eq!(Language::JavaScript.fence(), "javascript");
    assert_eq!(
        default_binding(Path::new("a.mjs"), "SyncRequest"),
        "syncRequest"
    );
    assert_eq!(
        default_binding(Path::new("a.rs"), "SyncRequest"),
        "sync_request"
    );
    assert_eq!(
        default_binding(Path::new("a.py"), "SyncRequest"),
        "sync_request"
    );
    assert_eq!(
        default_binding(Path::new("a.ts"), "SyncRequest"),
        "syncRequest"
    );
    assert_eq!(
        default_binding(Path::new("a.go"), "HTTPOptions"),
        "httpOptions"
    );
    assert_eq!(default_binding(Path::new("a.go"), "URL"), "url");
}
