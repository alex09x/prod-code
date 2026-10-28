/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::modifiers::in_async_fn;
use crate::signature::rewrite::{attribute, call_tokens};

#[test]
fn nearest_async_boundary_includes_blocks_and_closures() {
    let text = "fn load() {}\nfn sync() { let _ = async { load(); }; let _ = async || { load(); }; }\nasync fn outer() { let _ = || { load(); }; fn inner() { load(); } let _ = async move { load(); }; }\n";
    let calls: Vec<_> = text.match_indices("load()").map(|(i, _)| i).collect();
    let actual: Vec<_> = calls
        .iter()
        .skip(1)
        .map(|&i| in_async_fn(text, i))
        .collect();
    assert_eq!(actual, [true, true, false, false, true]);
    let expression = "async fn outer() { let f = || load(7); }";
    let end = expression.find("load(7)").unwrap() + "load(7)".len();
    assert!(!in_async_fn(expression, end));
}

#[test]
fn token_proof_preserves_punctuation_jointness_and_word_boundaries() {
    let code = |text| call_tokens(text).unwrap().1;
    assert_ne!(
        code("(stringify!(& &), stringify!(&&))"),
        code("(stringify!(&&), stringify!(& &))")
    );
    assert_ne!(
        code("(stringify!(first second))"),
        code("(stringify!(firstsecond))")
    );
    assert_eq!(code("(call ( 1 ), value)"), code("(call(1),value)"));
    assert_eq!(code("(x, -y)"), code("(x,-y)"));
    assert_ne!(code("(a && b)"), code("(a & &b)"));
    assert_eq!(
        code("(make::<Vec<Vec<u8> > >())"),
        code("(make::<Vec<Vec<u8>>>())")
    );
}

#[test]
fn attribution_distinguishes_reordered_macro_tokens_from_a_noop() {
    let old = "fn caller() { join(stringify!(& &), stringify!(&&)); }";
    let new = "fn caller() { join(stringify!(&&), stringify!(& &)); }";
    let pos = crate::signature::util::line_col_at(old, old.find("join").unwrap()).unwrap();
    let needs = |args: &[String], _: bool| {
        !crate::signature::plan::reorders_to_itself(args, &[Some(1), Some(0)])
    };
    let (unmatched, unexpected) = attribute("main.rs", old, new, "join", &[pos], &needs);
    assert!(
        unmatched.is_empty() && unexpected.is_empty(),
        "{unmatched:?} {unexpected:?}"
    );
    let (unmatched, _) = attribute("main.rs", old, old, "join", &[pos], &needs);
    assert_eq!(unmatched.len(), 1);
}

#[test]
fn pre_2024_gen_identifier_is_not_a_parse_failure() {
    let text = "async fn outer() { let gen = 7; load(gen); }";
    assert!(in_async_fn(text, text.find("load(gen)").unwrap()));
    assert!(call_tokens("(gen, other)").is_some());
}

#[test]
fn synchronous_generator_does_not_inherit_async_function_context() {
    let text = "async fn outer() { let _ = gen { load(7); }; }";
    assert!(!in_async_fn(text, text.find("load(7)").unwrap()));
    let macro_input = "async fn outer() { emit!(load(7)); }";
    assert!(!in_async_fn(
        macro_input,
        macro_input.find("load(7)").unwrap()
    ));
}
