/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A call is found wherever it sits: borrowed, nested in another call, across lines (#2052).

use super::matcher::find_structural_matches_in_source;
use super::types::CompiledPattern;

fn count(source: &str, pattern: &str) -> usize {
    let pattern = CompiledPattern::parse(pattern).expect("valid pattern");
    find_structural_matches_in_source("src/lib.rs", source, &pattern).len()
}

#[test]
fn a_borrowed_nested_call_matches() {
    let source = "fn f(b: Option<&str>) -> String {
    match b {
        Some(b) => hex(&Sha256::digest(crate::canonical_json(b).as_bytes())),
        None => String::new(),
    }
}
";
    assert_eq!(count(source, "Sha256::digest($ARG)"), 1);
}

#[test]
fn a_call_with_a_multiline_argument_matches() {
    let source = "fn g(body: &[u8], key: &str) -> Vec<u8> {
    let digest = Sha256::digest(
        [key.as_bytes(), b\":\", body]
            .concat(),
    );
    digest.to_vec()
}
";
    assert_eq!(count(source, "Sha256::digest($ARG)"), 1);
}

/// The real cause: a lifetime earlier in the file read as a character literal swallowed the
/// code up to the next quote.
#[test]
fn a_call_after_a_lifetime_matches() {
    let source = "pub fn message(op: &SessionOp<'_>) -> String {
    op.join(\"\\n\")
}

pub fn binding_digest(binding: Option<&Value>) -> String {
    match binding {
        None => String::new(),
        Some(b) => hex(&Sha256::digest(crate::canonical_json(b).as_bytes())),
    }
}

fn labelled<'a>(x: &'a str) -> char {
    'outer: loop { break 'outer; }
    let c = 'x';
    Sha256::digest(x);
    c
}
";
    assert_eq!(count(source, "Sha256::digest($ARG)"), 2);
}

/// `code_codemod` reads a Rust file as Rust too: the call after a lifetime is rewritten.
#[test]
fn a_rewrite_after_a_lifetime_applies() {
    let rule = super::types::CodemodRule::parse("Sha256::digest($ARG) ==>> sha256($ARG)")
        .expect("valid rule");
    let source = "fn m(op: &Op<'_>) {}\nfn d(b: &[u8]) -> Vec<u8> { Sha256::digest(b).to_vec() }\n";
    let rewritten =
        super::matcher::rewrite_source_in("src/lib.rs", source, &rule).expect("should match");
    assert!(rewritten.contains("sha256(b).to_vec()"), "{rewritten}");
}

#[test]
fn a_plain_call_still_matches() {
    assert_eq!(
        count(
            "fn h() { let d = Sha256::digest(x); }\n",
            "Sha256::digest($ARG)"
        ),
        1
    );
}
