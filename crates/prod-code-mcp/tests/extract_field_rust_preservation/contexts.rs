/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;

#[tokio::test]
async fn exhaustive_patterns_block_apply_and_rest_patterns_remain_unchanged() {
    for pattern in [
        "Store { entries }",
        "Some(Store { entries })",
        "[Store { entries }]",
        "(Store { entries },)",
    ] {
        let value = if pattern.starts_with("Some") {
            "Some(Store { entries: 1 })"
        } else if pattern.starts_with('[') {
            "[Store { entries: 1 }]"
        } else if pattern.starts_with('(') {
            "(Store { entries: 1 },)"
        } else {
            "Store { entries: 1 }"
        };
        let source = format!(
            "struct Store {{ entries: usize }}\nimpl Store {{\nfn limit(&self) -> usize {{ 64 }}\n}}\nfn main() {{ let value = {value}; match value {{ {pattern} => assert_eq!(entries, 1), _ => {{}} }} }}\n"
        );
        let ws = fixture(&source);
        assert!(execute(&ws.root(), "exhaustive before").0);
        let result = promote(&ws, "64", false, false, false).await.unwrap();
        assert_eq!(result.blocked.len(), 1, "{pattern}: {result:?}");
        assert!(promote(&ws, "64", false, true, false).await.is_err());
        assert_eq!(ws.read("src/main.rs"), source);
        let rest = pattern.replace("entries }", "entries, /* ) => */ .. }");
        ws.write("src/main.rs", &source.replace(pattern, &rest));
        let result = promote(&ws, "64", false, true, false).await.unwrap();
        assert!(result.blocked.is_empty());
        assert!(ws.read("src/main.rs").contains(&rest));
        assert!(execute(&ws.root(), "rest applied").0);
    }
}

#[tokio::test]
async fn range_values_nested_rests_and_update_values_are_not_top_level_rest() {
    let ws = fixture(
        "struct Store { range: std::ops::RangeFrom<usize>, pair: (usize, usize) }\nimpl Store {\nfn limit(&self) -> usize { 64 }\n}\nfn main() { let a = Store { range: 0.., pair: (1, 2) }; let b = Store { range: 1.., ..a }; let Store { pair: (x, ..), .. } = b; assert_eq!(x, 1); assert_eq!(b.limit(), 64); }\n",
    );
    assert!(execute(&ws.root(), "update before").0);
    let result = promote(&ws, "64", false, true, false).await.unwrap();
    assert_eq!(result.constructors, 2);
    assert!(execute(&ws.root(), "update applied").0);
}

#[tokio::test]
async fn trailing_field_comments_and_opaque_argument_delimiters_survive() {
    let ws = fixture(
        "struct Store {\nentries: usize // keep this field comment\n}\nimpl Store {\nfn limit(&self) -> usize { 64 }\n}\nfn wrap(s: Store, _: &str) -> usize { s.limit() }\nfn main() { assert_eq!(wrap(Store { entries: 1 }, r#\") => \" } /*\"#), 64); }\n",
    );
    assert!(execute(&ws.root(), "trivia before").0);
    let result = promote(&ws, "64", false, true, false).await.unwrap();
    assert_eq!(result.constructors, 1);
    assert!(
        ws.read("src/main.rs")
            .contains("entries: usize, // keep this field comment")
    );
    assert!(execute(&ws.root(), "trivia applied").0);
}

#[tokio::test]
async fn match_arm_constructor_and_descendant_module_keep_private_access() {
    let ws = fixture(
        "mod model {\npub struct Store { pub entries: usize }\nimpl Store {\npub fn limit(&self) -> usize { 64 }\n}\npub mod child {\nuse super::Store;\npub fn make(n: usize) -> Store { match n { 0 => Store { entries: 1 }, _ => Store { entries: 2 }, } }\n}\n}\nfn main() { assert_eq!(model::child::make(0).limit(), 64); }\n",
    );
    let result = promote(&ws, "64", false, true, false).await.unwrap();
    assert_eq!(result.constructors, 2);
    assert!(!ws.read("src/main.rs").contains("pub(crate) cap"));
    assert!(execute(&ws.root(), "descendant applied").0);
}
