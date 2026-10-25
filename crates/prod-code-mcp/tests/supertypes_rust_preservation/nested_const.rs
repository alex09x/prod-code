/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{Kind, hierarchy};

#[tokio::test]
async fn nested_const_completed_empty_impl_keeps_outer_header() {
    let result = hierarchy(
        "pub struct Target;",
        "Target",
        &[(
            "impl Tag<{struct Inner; impl Inner {} 0}> for Target",
            "Target",
        )],
        1,
    )
    .await;
    assert_eq!(result.kind, Kind::Type);
    assert_eq!(
        result
            .list
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["Tag<{struct Inner; impl Inner {} 0}>"]
    );
}

#[tokio::test]
async fn nested_const_completed_nonempty_impl_keeps_outer_header() {
    let result = hierarchy(
        "pub struct WithBody;",
        "WithBody",
        &[("}> for WithBody", "WithBody")],
        1,
    )
    .await;
    assert_eq!(
        result
            .list
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["Tag<{struct Local; impl Local { fn body() { let value = 0; } } 1}>"]
    );
}

#[tokio::test]
async fn nested_const_inner_bodies_and_unrelated_completed_owners_are_rejected() {
    for entry in [
        ("impl Inner {}", "Inner"),
        ("impl Inner {}", "{"),
        ("impl Inner {}", "}"),
        ("fn body() { let value", "body"),
        ("let value = 0;", "0"),
        ("0; } } 1", "}"),
        ("0; } } 1", "} 1"),
        ("pub struct Unrelated;\n}", "Unrelated"),
    ] {
        let result = hierarchy("pub struct Target;", "Target", &[entry], 1).await;
        assert!(
            result.list.is_empty(),
            "attached {entry:?}: {:?}",
            result.list
        );
    }
}

#[tokio::test]
async fn nested_const_derived_declaration_keeps_only_its_own_traits() {
    let mut observed = Vec::new();
    for entry in [
        ("#[derive(Clone)]", "#"),
        ("#[derive(Clone)]", "Clone"),
        ("struct DerivedLocal;", "DerivedLocal"),
    ] {
        let result = hierarchy("struct DerivedLocal;", "DerivedLocal", &[entry], 1).await;
        assert_eq!(result.kind, Kind::Type);
        observed.push(result.list.into_iter().map(|s| s.name).collect::<Vec<_>>());
    }
    assert_eq!(observed, vec![vec!["Clone"]; 3]);
}

#[tokio::test]
async fn nested_const_derived_item_does_not_hide_outer_self_type() {
    let result = hierarchy(
        "pub struct WithDerived;",
        "WithDerived",
        &[("}> for WithDerived", "WithDerived")],
        1,
    )
    .await;
    let expected = concat!(
        "Tag<{\n",
        "        struct Inner;\n",
        "        impl Inner {}\n",
        "        #[derive(Clone)]\n",
        "        struct DerivedLocal;\n",
        "        0\n",
        "    }>"
    )
    .replace('\n', " ");
    assert_eq!(
        result
            .list
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        [expected]
    );
}
