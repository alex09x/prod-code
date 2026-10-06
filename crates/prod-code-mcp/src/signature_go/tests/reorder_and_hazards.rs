/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::{Modifiers, Param};
use crate::signature_go::hazards::{
    classify, droppable, effect_hazards, is_subsequence, permutation, permuted, removed_names,
};
use crate::signature_go::types::{ArgKind, Call, GoParam, STILL_OPEN, refuse_non_result_modifiers};
use std::path::PathBuf;

fn call(args: &[&str]) -> Call {
    Call {
        path: PathBuf::from("m.go"),
        at: "m.go:1:1".into(),
        open: 0,
        close: 0,
        args: args.iter().map(|a| a.to_string()).collect(),
    }
}

fn declared(names: &[&str]) -> Vec<GoParam> {
    names
        .iter()
        .map(|n| GoParam {
            name: n.to_string(),
            ty: if n.starts_with("xs") { "...int" } else { "int" }.to_string(),
        })
        .collect()
}

#[test]
fn a_reorder_that_changes_evaluation_order_is_a_hazard() {
    let d = declared(&["a", "b", "c"]);
    let order = [1, 0, 2];
    let safe = [
        call(&["x", "y", "g()"]),
        call(&["1", "g()", "h()"]),
        call(&["x.f", "&y", "2"]),
    ];
    assert!(effect_hazards("f", &d, &order, false, &safe).is_empty());
    let unsafe_ = [call(&["g()", "h()", "1"]), call(&["x", "g()", "1"])];
    let found = effect_hazards("f", &d, &order, false, &unsafe_);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].contains("`g()` and `h()`"), "{found:?}");
    let short = effect_hazards("f", &d, &order, false, &[call(&["g()"])]);
    assert!(short[0].contains("cannot be checked"), "{short:?}");
    let v = declared(&["a", "b", "xs"]);
    assert!(effect_hazards("f", &v, &order, true, &[call(&["1", "2", "3", "4"])]).is_empty());
    assert!(effect_hazards("f", &v, &order, true, &[call(&["1", "2", "ys..."])]).is_empty());
    assert!(!effect_hazards("f", &v, &order, true, &[call(&["1", "ys..."])]).is_empty());
}

/// `true`, `false` and `nil` are predeclared identifiers that a scope can redeclare
/// (`true := 1; f(true, bump(&true))`), so they are variables as far as their spelling goes,
/// and a reorder against a call is a hazard. Literals that cannot be redeclared still are.
#[test]
fn predeclared_names_are_not_trusted_as_literals() {
    use ArgKind::*;
    for name in ["true", "false", "nil", "(true)", "&nil"] {
        assert_eq!(classify(name), Place, "{name}");
    }
    for literal in ["1", "0x1F", "1_000", "2.5e+3", "'x'", "\"true\"", "`nil`"] {
        assert_eq!(classify(literal), Literal, "{literal}");
    }
    let d = declared(&["a", "b"]);
    for name in ["true", "false", "nil"] {
        let bump = format!("bump(&{name})");
        let found = effect_hazards("f", &d, &[1, 0], false, &[call(&[name, &bump])]);
        assert_eq!(found.len(), 1, "{name}: {found:?}");
        assert!(
            found[0].contains(&format!("`{name}` and `{bump}`")),
            "{found:?}"
        );
    }
    // Two reads, or a read beside a real literal, stay independent.
    assert!(effect_hazards("f", &d, &[1, 0], false, &[call(&["true", "nil"])]).is_empty());
    assert_eq!(
        effect_hazards("f", &d, &[1, 0], false, &[call(&["false", "g()"])]).len(),
        1
    );
    assert!(effect_hazards("f", &d, &[1, 0], false, &[call(&["1", "g()"])]).is_empty());
}

#[test]
fn permutations_keep_variadic_tails_in_place() {
    let args: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
    assert_eq!(
        permuted(&args[..3], &[2, 0, 1], 3, false),
        vec!["c", "a", "b"]
    );
    assert_eq!(
        permuted(&args, &[1, 0, 2], 3, true),
        vec!["b", "a", "c", "d"]
    );
    assert_eq!(permuted(&args[..2], &[1, 0, 2], 3, true), vec!["b", "a"]);
    assert_eq!(
        permuted(&args[..3], &[1, 0, 2], 3, true),
        vec!["b", "a", "c"]
    );
}

/// The arguments after a removal come from the declared arity, not from how many
/// parameters are left: with three declared and one kept, the tail starts at the third
/// argument, not the first.
#[test]
fn removals_keep_the_declared_arity_and_the_variadic_tail() {
    let args: Vec<String> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(permuted(&args[..3], &[2, 0], 3, false), vec!["c", "a"]);
    assert_eq!(permuted(&args[..3], &[], 3, false), Vec::<String>::new());
    // `f(a, b, xs ...int)` without `a`: the tail stays whole and last.
    assert_eq!(permuted(&args, &[1, 2], 3, true), vec!["b", "c", "d", "e"]);
    assert_eq!(permuted(&args[..2], &[1, 2], 3, true), vec!["b"]);
    let spread = vec!["a".to_string(), "b".to_string(), "ys...".to_string()];
    assert_eq!(permuted(&spread, &[1, 2], 3, true), vec!["b", "ys..."]);
    // Without `xs`: the whole tail goes, however long, spread or not.
    assert_eq!(permuted(&args, &[1, 0], 3, true), vec!["b", "a"]);
    assert_eq!(permuted(&spread, &[0], 3, true), vec!["a"]);
}

#[test]
fn dropped_arguments_must_do_nothing_when_evaluated() {
    use ArgKind::*;
    for pure in [
        "1",
        "\"s\"",
        "'r'",
        "x",
        "&x",
        "(x)",
        "nil",
        "func() { g() }",
        "x /* c */",
    ] {
        assert!(droppable(pure), "{pure}");
    }
    // A selector is a `Place` for a reorder, but it can dereference nil and panic.
    assert_eq!(classify("p.n"), Place);
    for effect in [
        "p.n", "&p.n", "g()", "<-ch", "xs[i]", "i+1", "T(x)", "*p", "-x", "x.(T)", "[]int{1}",
    ] {
        assert!(!droppable(effect), "{effect}");
    }
    let d = declared(&["a", "b", "c"]);
    let safe = [call(&["g()", "1", "x"]), call(&["g()", "&y", "\"s\""])];
    assert!(effect_hazards("f", &d, &[0], false, &safe).is_empty());
    let found = effect_hazards(
        "f",
        &d,
        &[0],
        false,
        &[call(&["1", "h()", "p.n"]), call(&["x", "<-ch", "a[0]"])],
    );
    assert_eq!(found.len(), 4, "{found:?}");
    assert!(
        found[0].contains("`h()` is passed for the removed `b`")
            && found[1].contains("`p.n` is passed for the removed `c`")
            && found[2].contains("`<-ch`")
            && found[3].contains("`a[0]`"),
        "{found:?}"
    );
    // A reorder of what is kept is checked too.
    let both = effect_hazards("f", &d, &[2, 0], false, &[call(&["g()", "1", "h()"])]);
    assert_eq!(both.len(), 1, "{both:?}");
    assert!(both[0].contains("opposite order"), "{both:?}");
    // A pair passed as the arguments, and too few or too many: the arity cannot be matched.
    for args in [
        &["two()"][..],
        &["1", "2"],
        &["1", "2", "3", "4"],
        &["1", "2", "xs..."],
    ] {
        let odd = effect_hazards("f", &d, &[0], false, &[call(args)]);
        assert!(odd[0].contains("cannot be checked"), "{args:?}: {odd:?}");
    }
}

#[test]
fn a_removed_variadic_parameter_takes_its_whole_tail() {
    let v = declared(&["a", "xs"]);
    let calls = [
        call(&["1"]),
        call(&["1", "2", "x"]),
        call(&["1", "ys..."]),
        call(&["g()", "x"]),
    ];
    assert!(effect_hazards("f", &v, &[0], true, &calls).is_empty());
    let found = effect_hazards(
        "f",
        &v,
        &[0],
        true,
        &[call(&["1", "2", "g()"]), call(&["1", "p.xs..."])],
    );
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found[0].contains("`g()` is passed for the removed `xs`")
            && found[1].contains("`p.xs...` is passed for the removed `xs`"),
        "{found:?}"
    );
    // Kept, the tail is not dropped, and a removed fixed parameter is checked alone.
    let kept = effect_hazards("f", &v, &[1], true, &[call(&["x", "g()", "h()"])]);
    assert!(kept.is_empty(), "{kept:?}");
    let fixed = effect_hazards("f", &v, &[1], true, &[call(&["g()", "1"])]);
    assert!(
        fixed[0].contains("`g()` is passed for the removed `a`"),
        "{fixed:?}"
    );
}

#[test]
fn requests_that_are_not_permutations_are_refused_with_the_open_requirement() {
    let d = declared(&["a", "b"]);
    let keep = |n: &str| Param::Keep(n.to_string());
    assert_eq!(
        permutation(&d, &[keep("b"), keep("a")]).unwrap(),
        vec![1, 0]
    );
    // A parameter left out is to be removed, alone, with a reorder, or all of them.
    assert_eq!(permutation(&d, &[keep("b")]).unwrap(), vec![1]);
    assert_eq!(permutation(&d, &[keep("a")]).unwrap(), vec![0]);
    assert_eq!(permutation(&d, &[]).unwrap(), Vec::<usize>::new());
    let three = declared(&["a", "b", "c"]);
    assert_eq!(
        permutation(&three, &[keep("c"), keep("a")]).unwrap(),
        vec![2, 0]
    );
    assert!(is_subsequence(&[0, 2]) && !is_subsequence(&[2, 0]));
    assert_eq!(removed_names(&three, &[0, 2]), "`a`, `c`");
    let added = permutation(
        &d,
        &[
            keep("a"),
            keep("b"),
            Param::Add {
                name: "c".into(),
                ty: "int".into(),
                value: "0".into(),
            },
        ],
    )
    .unwrap_err()
    .to_string();
    assert!(added.contains("adding the parameter `c`"), "{added}");
    assert!(permutation(&d, &[keep("a"), keep("b")]).is_err());
    assert!(permutation(&d, &[keep("b"), keep("b")]).is_err());
    assert!(permutation(&d, &[keep("z"), keep("a")]).is_err());
    for m in [
        Modifiers {
            visibility: Some("pub".into()),
            ..Default::default()
        },
        Modifiers {
            asyncness: Some(true),
            ..Default::default()
        },
    ] {
        let err = refuse_non_result_modifiers(&m).unwrap_err().to_string();
        assert!(err.contains(STILL_OPEN), "{err}");
    }
}
