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
async fn complete_arithmetic_subexpressions_remain_useful() {
    let ws = fixture(
        "struct Store {}\nimpl Store {\nfn limit(&self) -> usize {\nlet a = 8 * 8;\nlet b = 1 + 8 * 8;\nlet c = (8 * 8);\na + b + c\n}\n}\nfn main() { assert_eq!(Store {}.limit(), 193); }\n",
    );
    assert!(execute(&ws.root(), "arithmetic before").0);
    let result = promote(&ws, "8 * 8", true, true, false).await.unwrap();
    assert_eq!(result.replaced, 3);
    assert!(execute(&ws.root(), "arithmetic applied").0);
}

#[tokio::test]
async fn precedence_prefixes_are_skipped_and_invalid_single_selections_refused() {
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> usize {\nlet a = 60 + 4;\nlet b = 60 + 4 * 2;\na + b\n}\n}\nfn main() { assert_eq!(Store {}.limit(), 132); }\n";
    let ws = fixture(source);
    let result = promote(&ws, "60 + 4", true, true, false).await.unwrap();
    assert_eq!(result.replaced, 1);
    assert!(ws.read("src/main.rs").contains("let b = 60 + 4 * 2"));
    assert!(execute(&ws.root(), "precedence applied").0);
    ws.write(
        "src/main.rs",
        source.replace("let a = 60 + 4;\n", "").as_str(),
    );
    assert!(promote(&ws, "60 + 4", false, true, true).await.is_err());
}

#[tokio::test]
async fn literals_comments_utf8_and_nested_function_bytes_survive() {
    let ws = fixture(
        "struct Store {}\nimpl Store {\nfn limit(&self) -> usize {\nlet selected = 64;\nlet raw = r###\"64 \" } Self { cap: 64 } /*\"###;\nlet bytes = br#\"64\"#;\nlet ü = 164;\nlet lim\u{301}64 = 9;\nlet array = [0usize; 64];\nlet generic = std::array::from_fn::<_, 64, _>(|_| 0usize);\nenum Local { Count = 64 }\n// 64 Self { cap: 64 }\n/* 64 /* nested 64 */ 64 */\nfn nested() -> usize { 64 }\nassert_eq!(array.len(), generic.len());\nassert!(raw.starts_with(\"64\"));\nassert_eq!(bytes, b\"64\");\nselected + ü + lim\u{301}64 + nested()\n}\n}\nfn main() { assert_eq!(Store {}.limit(), 301); }\n",
    );
    assert!(execute(&ws.root(), "opaque before").0);
    let result = promote(&ws, "64", true, true, false).await.unwrap();
    assert_eq!(result.replaced, 1);
    assert_eq!(result.constructors, 1);
    let changed = ws.read("src/main.rs");
    assert!(changed.contains("/* 64 /* nested 64 */ 64 */"));
    assert!(changed.contains("fn nested() -> usize { 64 }"));
    assert!(changed.contains("lim\u{301}64"));
    assert!(execute(&ws.root(), "opaque applied").0);
}

#[tokio::test]
async fn shadowed_names_keep_their_binding_identity() {
    let ws = fixture(
        "const FACTOR: usize = 8;\nstruct Store {}\nimpl Store {\nfn limit(&self) -> usize {\nlet base = 8;\nlet a = base * FACTOR;\nlet b = base * FACTOR;\nlet base = 2;\nlet c = base * FACTOR;\nlet d = { let base = 3; base * FACTOR };\na + b + c + d\n}\n}\nfn main() { assert_eq!(Store {}.limit(), 168); }\n",
    );
    assert!(execute(&ws.root(), "bindings before").0);
    let result = promote(&ws, "base * FACTOR", true, true, false)
        .await
        .unwrap();
    assert_eq!(result.replaced, 2);
    assert!(execute(&ws.root(), "bindings applied").0);
}

#[tokio::test]
async fn private_same_module_single_selection_keeps_other_occurrences() {
    let ws = fixture(
        "struct Store { entries: usize }\nimpl Store {\nfn limit(&self) -> usize { let a = 64; let b = 64; a + b + self.entries }\n}\nfn main() { assert_eq!(Store { entries: 1 }.limit(), 129); }\n",
    );
    let result = promote(&ws, "64", false, true, false).await.unwrap();
    assert_eq!(result.replaced, 1);
    assert!(!ws.read("src/main.rs").contains("pub "));
    assert!(!ws.read("src/main.rs").contains("pub(crate)"));
    assert!(execute(&ws.root(), "private applied").0);
}
