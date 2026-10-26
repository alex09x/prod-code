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
async fn replace_all_preserves_quoted_bytes_and_larger_tokens() {
    let ws = fixture(
        "struct Store {}\nimpl Store {\nfn limit(&self) -> usize {\nlet selected = 64;\nlet again = 64;\nlet quoted = \"64\";\nlet larger = 164;\nassert_eq!(quoted, \"64\");\nassert_eq!(larger, 164);\nselected + again\n}\n}\nfn main() { assert_eq!(Store {}.limit(), 128); }\n",
    );
    assert!(execute(&ws.root(), "tokens before").0);
    let result = promote(&ws, "64", true, false, false).await.unwrap();
    returned(&ws, &result);
    let (ok, report) = execute(&ws.root(), "tokens returned");
    assert!(ok, "{report}");
    assert_eq!(result.replaced, 2);
}

#[tokio::test]
async fn sibling_module_constructor_remains_accessible() {
    let ws = fixture(
        "mod model {\npub struct Store { pub entries: usize }\nimpl Store {\npub fn limit(&self) -> usize { 64 }\n}\n}\nmod app {\nuse super::model::Store;\npub fn run() -> usize { Store { entries: 1 }.limit() }\n}\nfn main() { assert_eq!(app::run(), 64); }\n",
    );
    assert!(execute(&ws.root(), "visibility before").0);
    let result = promote(&ws, "64", false, false, false).await.unwrap();
    returned(&ws, &result);
    let (ok, report) = execute(&ws.root(), "visibility returned");
    assert!(ok, "{report}");
}

#[tokio::test]
async fn field_range_from_is_a_constructor_value() {
    let ws = fixture(
        "struct Store { range: std::ops::RangeFrom<usize> }\nimpl Store {\nfn limit(&self) -> usize { 64 }\n}\nfn main() { let s = Store { range: 0.. }; assert_eq!(s.limit(), 64); }\n",
    );
    assert!(execute(&ws.root(), "range before").0);
    let result = promote(&ws, "64", false, false, false).await.unwrap();
    returned(&ws, &result);
    let (ok, report) = execute(&ws.root(), "range returned");
    assert!(ok, "{report}");
    assert_eq!(result.constructors, 1);
}

#[tokio::test]
async fn comment_delimiters_do_not_turn_arguments_into_patterns() {
    let ws = fixture(
        "struct Store { entries: usize }\nimpl Store {\nfn limit(&self) -> usize { 64 }\n}\nfn wrap(s: Store, _: usize) -> usize { s.limit() }\nfn main() { let entries = 1; let other = 2; assert_eq!(wrap(Store { entries }, /* ) => */ other), 64); }\n",
    );
    assert!(execute(&ws.root(), "comments before").0);
    let result = promote(&ws, "64", false, false, false).await.unwrap();
    returned(&ws, &result);
    let (ok, report) = execute(&ws.root(), "comments returned");
    assert!(ok, "{report}");
    assert!(result.blocked.is_empty());
}

#[tokio::test]
async fn nine_nested_patterns_never_receive_an_initializer() {
    let wrap = |s: &str| format!("{}{}{}", "Some(".repeat(9), s, ")".repeat(9));
    let source = format!(
        "struct Store {{ entries: usize }}\nimpl Store {{\nfn limit(&self) -> usize {{ 64 }}\n}}\nfn main() {{ let v = {}; match v {{ {} => assert_eq!(entries, 1), _ => unreachable!() }} }}\n",
        wrap("Store { entries: 1 }"),
        wrap("Store { entries }")
    );
    let ws = fixture(&source);
    assert!(execute(&ws.root(), "deep before").0);
    let result = promote(&ws, "64", false, false, false).await.unwrap();
    returned(&ws, &result);
    let (ok, _) = execute(&ws.root(), "deep returned");
    assert!(
        !result.blocked.is_empty() || !result.unmatched.is_empty(),
        "unclassified pattern; rustc success={ok}"
    );
    assert!(ws.read("src/main.rs").contains("Store { entries }"));
    // Applying uncertainty must still fail even if analyzer errors are overridden.
    ws.write("src/main.rs", &source);
    assert!(promote(&ws, "64", false, true, false).await.is_err());
    assert!(promote(&ws, "64", false, true, true).await.is_err());
    assert_eq!(ws.read("src/main.rs"), source);
}
