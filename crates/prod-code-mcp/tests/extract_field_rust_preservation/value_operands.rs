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
async fn single_bitwise_operand_remains_selectable() {
    for body in [
        "64 | 1",
        "(64 | 1)",
        "64 | { /* ) => */ 1 }",
        "64 | if true { 1 } else { 0 }",
    ] {
        let source = format!(
            "struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ {body} }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 65); }}\n"
        );
        let ws = fixture(&source);
        assert!(execute(&ws.root(), "bitwise single before").0);
        let result = promote(&ws, "64", false, false, false).await.unwrap();
        returned(&ws, &result);
        assert!(execute(&ws.root(), "bitwise single returned").0);
        assert_eq!(result.replaced, 1);
        assert!(ws.read("src/main.rs").contains("self.cap |"));
    }
}

#[tokio::test]
async fn replace_all_distinguishes_bitwise_values_from_pattern_alternatives() {
    let ws = fixture(
        "struct Store {}\nimpl Store {\nfn limit(&self) -> usize {\nlet selected = 64;\nlet flags = 64 | 1;\nlet matched = match flags { 64 | 65 => 1, _ => 0 };\nselected + flags + matched\n}\n}\nfn main() { assert_eq!(Store {}.limit(), 130); }\n",
    );
    assert!(execute(&ws.root(), "bitwise duplicates before").0);
    let result = promote(&ws, "64", true, false, false).await.unwrap();
    returned(&ws, &result);
    assert!(execute(&ws.root(), "bitwise duplicates returned").0);
    assert_eq!(result.replaced, 2);
    assert!(ws.read("src/main.rs").contains("64 | 65 =>"));
}

#[tokio::test]
async fn break_values_including_labels_remain_selectable() {
    for body in [
        "loop { break 64; }",
        "'done: loop { break 'done 64; }",
        "'done: { break 'done 64; }",
    ] {
        let source = format!(
            "struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ {body} }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 64); }}\n"
        );
        let ws = fixture(&source);
        assert!(execute(&ws.root(), "break before").0);
        let result = promote(&ws, "64", false, false, false).await.unwrap();
        returned(&ws, &result);
        assert!(execute(&ws.root(), "break returned").0);
        assert_eq!(result.replaced, 1);
    }
}

#[tokio::test]
async fn alternative_patterns_are_refused_even_under_force() {
    for body in [
        "match self.entries { 64 | 65 => 1, _ => 0 }",
        "if let Some(64 | 65) = Some(self.entries) { 1 } else { 0 }",
        "let Some(64 | 65) = Some(self.entries) else { return 0; }; 1",
        "match Some(self.entries) { Some(64 | 65) if true => 1, _ => 0 }",
    ] {
        let source = format!(
            "struct Store {{ entries: usize }}\nimpl Store {{\nfn limit(&self) -> usize {{ {body} }}\n}}\nfn main() {{ assert_eq!(Store {{ entries: 64 }}.limit(), 1); }}\n"
        );
        let ws = fixture(&source);
        assert!(execute(&ws.root(), "alternatives before").0);
        for force in [false, true] {
            assert!(promote(&ws, "64", false, true, force).await.is_err());
            assert_eq!(ws.read("src/main.rs"), source);
        }
    }
}

#[tokio::test]
async fn constructor_alternatives_remain_patterns_and_keep_their_rests() {
    let source = "struct Store { entries: usize }\nimpl Store {\nfn limit(&self) -> usize { 64 }\n}\nfn main() { let s = Store { entries: 1 }; assert_eq!(s.limit(), 64); match s { Store { entries: 0 } | Store { entries: 1 } => {}, _ => unreachable!() } }\n";
    let ws = fixture(source);
    assert!(execute(&ws.root(), "constructor alternatives before").0);
    let result = promote(&ws, "64", false, false, false).await.unwrap();
    assert_eq!(result.blocked.len(), 2);
    assert!(result.unmatched.is_empty());
    assert_eq!(result.constructors, 1);
    returned(&ws, &result);
    assert!(!execute(&ws.root(), "blocked constructor alternatives returned").0);
    assert!(
        ws.read("src/main.rs")
            .contains("Store { entries: 0 } | Store { entries: 1 }")
    );
    ws.write("src/main.rs", source);
    assert!(promote(&ws, "64", false, true, false).await.is_err());
    assert_eq!(ws.read("src/main.rs"), source);
    let rest = source.replace(
        "Store { entries: 0 } | Store { entries: 1 }",
        "Store { entries: 0, .. } | Store { entries: 1, .. }",
    );
    ws.write("src/main.rs", &rest);
    let result = promote(&ws, "64", false, true, false).await.unwrap();
    assert!(result.blocked.is_empty());
    assert_eq!(result.constructors, 1);
    assert!(execute(&ws.root(), "rest constructor alternatives applied").0);
}
