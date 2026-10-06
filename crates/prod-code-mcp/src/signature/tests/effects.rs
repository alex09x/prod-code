/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::call_sites::call_arguments;
use crate::signature::effects::hazards::{
    coercion_name, drop_free_names, effect_hazards, hover_is_adt, hover_is_builtin,
};
use crate::signature::parse::{parse_declared, split_at_top_level};
use crate::signature::types::{CallSite, Declared, ParamFacts, REF_COERCION, UNCONFIRMED_TYPE};
use crate::signature::util::line_col_at;

/// Facts for parameters the analyzer described as expected: every name in their types is the
/// built-in type, or a struct or an enum, it is spelled as.
fn confirmed(declared: &[Declared]) -> Vec<ParamFacts> {
    declared
        .iter()
        .map(|d| {
            let ty = split_at_top_level(&d.raw, ':').map_or("", |(_, ty)| ty);
            ParamFacts {
                drop_free: drop_free_names(ty).is_some(),
                coercion_free: coercion_name(ty).is_some(),
                reference: ty.trim_start().starts_with('&'),
                unconfirmed: Vec::new(),
            }
        })
        .collect()
}

/// The calls to `name` in `text`, each reported by its line.
fn sites(text: &str, name: &str) -> Vec<CallSite> {
    text.match_indices(name)
        .filter_map(|(at, _)| {
            call_arguments(text, at).unwrap().map(|args| CallSite {
                at: line_col_at(text, at).unwrap().0.to_string(),
                args,
            })
        })
        .collect()
}

/// #442: rust-analyzer's hovers, verbatim in shape: a built-in type is described by its name
/// alone, a declared one by its module and then its declaration.
#[test]
fn a_hover_confirms_a_builtin_or_a_declared_type_only_as_it_is_written() {
    let builtin = "\n```rust\nu32\n```\n\n---\n\nThe 32-bit unsigned integer type.";
    let option = "\n```rust\ncore::option\n```\n\n```rust\npub enum Option<T> {\n    None,\n    Some( /* … */ ),\n}\n```\n\n---\n\nThe `Option` type.";
    let own_option = "```rust\nfixture\n```\n\n```rust\npub enum Option<T> {\n    None,\n    Some( /* … */ ),\n}\n```";
    let own_u32 = "```rust\nfixture\n```\n\n```rust\npub(crate) struct u32\n```";
    let alias = "```rust\nfixture\n```\n\n```rust\npub type Inner = &'static Wrap\n```";
    let generic = "```rust\nT\n```";
    assert!(hover_is_builtin(builtin, "u32"));
    assert!(hover_is_builtin(option, "Option"));
    assert!(!hover_is_builtin(own_option, "Option"));
    assert!(!hover_is_builtin(own_u32, "u32"));
    assert!(!hover_is_builtin(builtin, "u64"));
    assert!(!hover_is_builtin(generic, "T"));
    assert!(hover_is_adt(option, "Option"));
    assert!(hover_is_adt(own_option, "Option"));
    assert!(hover_is_adt(own_u32, "u32"));
    assert!(!hover_is_adt(alias, "Inner"));
    assert!(!hover_is_adt(generic, "T"));
    assert!(!hover_is_adt(own_u32, "u3"));
}

#[test]
fn a_conversion_into_a_type_is_ruled_out_by_its_shape_or_its_name() {
    assert_eq!(coercion_name("&Inner"), None);
    assert_eq!(coercion_name(" &'a mut [u8]"), None);
    assert_eq!(coercion_name("impl AsRef<str>"), None);
    assert_eq!(coercion_name("dyn Fn()"), None);
    for free in ["*const u8", "fn(u32) -> u32", "(u8, &str)", "[u8; 4]", "()"] {
        assert_eq!(coercion_name(free), Some(None), "{free}");
    }
    assert_eq!(coercion_name(" u32"), Some(Some((1, "u32".into()))));
    assert_eq!(
        coercion_name("std::option::Option<&T>"),
        Some(Some((13, "Option".into())))
    );
    assert_eq!(coercion_name("Vec<u8"), None);
}

/// A type is drop-free by its shape, and by names the analyzer then has to confirm; each
/// name comes with where it is, which is where the analyzer is asked.
#[test]
fn only_types_without_drop_code_are_drop_free() {
    let names = |ty: &str| {
        drop_free_names(ty).map(|n| {
            n.into_iter()
                .map(|(at, name)| {
                    assert_eq!(&ty[at..at + name.len()], name, "{ty}");
                    name
                })
                .collect::<Vec<_>>()
        })
    };
    for (free, expected) in [
        ("u32", vec!["u32"]),
        (" u32", vec!["u32"]),
        ("&str", vec![]),
        ("&mut Vec<String>", vec![]),
        ("*const u8", vec![]),
        ("fn(u32) -> u32", vec![]),
        ("(u8, bool)", vec!["u8", "bool"]),
        ("( u8 ,bool, )", vec!["u8", "bool"]),
        ("[u8; 4]", vec!["u8"]),
        ("Option<&T>", vec!["Option"]),
        ("Option<(u32, [i8; 2])>", vec!["Option", "u32", "i8"]),
        ("()", vec![]),
    ] {
        let expected: Vec<String> = expected.into_iter().map(String::from).collect();
        assert_eq!(names(free), Some(expected), "{free}");
    }
    for owned in [
        "String",
        "Vec<u8>",
        "T",
        "impl Fn()",
        "Noisy",
        "(u8, String)",
        "[String; 2]",
        "Option<String>",
        "Box<u8>",
        "core::primitive::u32",
        "(u8,,u8)",
    ] {
        assert_eq!(names(owned), None, "{owned}");
    }
}

/// #442, the reproduction: a reorder of `f(mark("a"), mark("b"))` runs the marks the other
/// way round, a reorder of two owned parameters drops them the other way round, and removing
/// a parameter removes what its argument did. A literal, a string and a reference do not.
#[test]
fn a_reorder_or_removal_that_changes_effects_or_drops_is_named() {
    const DEMO: &str = "pub fn demo(k: &u32) {\n    eff_pair(mark(\"a\"), mark(\"b\"));\n    eff_owned(x, y);\n    eff_unused(1, mark(\"b\"));\n    eff_simple(3, \"lit\", k);\n    eff_pair(\"a\", mark(\"b\"));\n}\n";
    let swap = [Some(1), Some(0)];
    let pair = parse_declared("first: &str, second: &str").1;
    assert_eq!(
        effect_hazards(
            "eff_pair",
            &pair,
            &confirmed(&pair),
            false,
            &swap,
            &sites(DEMO, "eff_pair")
        ),
        ["2: `mark(\"a\")` and `mark(\"b\")` would be evaluated in the opposite order"],
        "a literal and a call on line 6 are independent"
    );
    let owned = parse_declared("x: Noisy, y: Noisy").1;
    let hazards = effect_hazards(
        "eff_owned",
        &owned,
        &confirmed(&owned),
        false,
        &swap,
        &sites(DEMO, "eff_owned"),
    );
    assert_eq!(hazards.len(), 1, "{hazards:?}");
    assert!(
        hazards[0].contains("`eff_owned` drops `y: Noisy` before `x: Noisy`"),
        "{hazards:?}"
    );
    let unused = parse_declared("a: u32, _b: &str").1;
    assert_eq!(
        effect_hazards(
            "eff_unused",
            &unused,
            &confirmed(&unused),
            false,
            &[Some(0)],
            &sites(DEMO, "eff_unused")
        ),
        [
            "4: `mark(\"b\")` is evaluated for `_b`, and removing the parameter removes what \
          it does"
        ]
    );
    let simple = parse_declared("n: u32, s: &str, r: &u32").1;
    let facts = confirmed(&simple);
    let calls = sites(DEMO, "eff_simple");
    assert!(
        effect_hazards(
            "s",
            &simple,
            &facts,
            false,
            &[Some(2), Some(0), Some(1)],
            &calls
        )
        .is_empty(),
        "a reference moved past literals keeps its meaning"
    );
    // `k` passed for `&u32` may be a `&Wrapper` that `Deref` converts: removing it may
    // remove that call.
    assert_eq!(
        effect_hazards("s", &simple, &facts, false, &[Some(1), Some(0)], &calls),
        [format!(
            "5: `k` is evaluated for `r`, and removing the parameter may remove what it \
             does: {REF_COERCION}"
        )]
    );
    // Two scalars the analyzer confirmed are reordered and removed freely; unconfirmed, the
    // same names may be a `struct u32` with a `Drop`.
    let scalars = parse_declared("a: u32, b: u32").1;
    let calls = sites("f(p, /* /* */ q */ q)", "f");
    let facts = confirmed(&scalars);
    assert!(effect_hazards("f", &scalars, &facts, false, &swap, &calls).is_empty());
    assert!(effect_hazards("f", &scalars, &facts, false, &[Some(0)], &calls).is_empty());
    let unconfirmed = vec![
        ParamFacts {
            unconfirmed: vec!["u32".into()],
            ..Default::default()
        };
        2
    ];
    let hazards = effect_hazards("f", &scalars, &unconfirmed, false, &swap, &calls);
    assert_eq!(hazards.len(), 2, "{hazards:?}");
    assert!(
        hazards[0].contains("drops `b: u32` before `a: u32`")
            && hazards[0].contains("does not confirm that `u32` is the built-in"),
        "{hazards:?}"
    );
    assert!(
        hazards[1].ends_with(&format!("in the opposite order ({UNCONFIRMED_TYPE})")),
        "{hazards:?}"
    );
    // An owned value that is removed is dropped somewhere else, or never.
    let guard = parse_declared("a: u32, g: Guard").1;
    let hazards = effect_hazards(
        "f",
        &guard,
        &confirmed(&guard),
        false,
        &[Some(0)],
        &sites("f(1, g)", "f"),
    );
    assert!(
        hazards[0].contains("`g` is moved into `g: Guard`"),
        "{hazards:?}"
    );
    // A call that does not pass what the declaration takes cannot be judged.
    let hazards = effect_hazards(
        "eff_pair",
        &pair,
        &confirmed(&pair),
        false,
        &swap,
        &sites("eff_pair(a)", "eff_pair"),
    );
    assert!(hazards[0].contains("passes 1 argument(s)"), "{hazards:?}");
}
