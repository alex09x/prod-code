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

async fn operand_case(label: &str, helpers: &str, expression: &str, expected: usize) {
    let mut failures = Vec::new();
    for all in [false, true] {
        let prefix = if all { "let selected = 64;" } else { "" };
        let source = format!(
            "{helpers}\nstruct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ {prefix} {expression} }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), {expected}); }}\n"
        );
        let ws = fixture(&source);
        let label = format!("{label} all={all}");
        assert!(execute(&ws.root(), &format!("{label} before")).0);
        match promote(&ws, "64", all, true, false).await {
            Ok(result) => {
                let (ok, _) = execute(&ws.root(), &format!("{label} returned"));
                if !ok || result.replaced != if all { 2 } else { 1 } {
                    failures.push(format!("{label}: {result:?}"));
                }
            }
            Err(e) => {
                let unchanged = ws.read("src/main.rs") == source;
                println!("{label} refused; byte_equal={unchanged}");
                assert!(unchanged);
                failures.push(format!("{label}: {e}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn sibling_closure_does_not_enclose_constructor() {
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> usize { 64 }\n}\nfn consume(s: Store, f: impl Fn() -> usize) -> usize { s.limit() + f() }\nfn main() { assert_eq!(consume(Store {}, ||1), 65); }\n";
    let mut failures = Vec::new();
    for all in [false, true] {
        let ws = fixture(source);
        let label = format!("sibling constructor all={all}");
        assert!(execute(&ws.root(), &format!("{label} before")).0);
        let result = promote(&ws, "64", all, false, false).await.unwrap();
        returned(&ws, &result);
        let (ok, _) = execute(&ws.root(), &format!("{label} returned"));
        if !ok || result.constructors != 1 || !result.blocked.is_empty() {
            failures.push(format!("{label}: {result:?}"));
        }
        ws.write("src/main.rs", source);
        match promote(&ws, "64", all, true, false).await {
            Ok(_) => assert!(execute(&ws.root(), &format!("{label} applied")).0),
            Err(e) => {
                let unchanged = ws.read("src/main.rs") == source;
                println!("{label} refused; byte_equal={unchanged}");
                assert!(unchanged);
                failures.push(format!("{label}: {e}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn sibling_closure_does_not_enclose_value_operand() {
    operand_case(
        "sibling operand",
        "fn consume(n: usize, f: impl Fn() -> usize) -> usize { n + f() }",
        "consume(64, ||1)",
        65,
    )
    .await;
}

#[tokio::test]
async fn unrelated_sibling_closures_do_not_exhaust_candidate_context() {
    let expression = format!("consume(64, {}1)", "|| ".repeat(12));
    operand_case(
        "deep sibling operand",
        "fn consume<F>(n: usize, _: F) -> usize { n }",
        &expression,
        64,
    )
    .await;
}

#[tokio::test]
async fn unary_not_is_not_a_macro_payload() {
    operand_case("unary not", "", "(!(64==0)) as usize", 1).await;
}

#[tokio::test]
async fn array_list_child_semicolon_is_not_a_repeat_separator() {
    operand_case(
        "array list",
        "",
        "let values = [{let n=0usize;n},64]; values[1]",
        64,
    )
    .await;
}

#[tokio::test]
async fn syntax_contexts_keep_constant_and_macro_payloads_unchanged() {
    let source = "#[macro_export] macro_rules! keep { ($e:expr) => { $e }; }\nstruct Store {}\nimpl Store {\nfn limit(&self) -> usize {\nlet selected = 64;\nlet a = keep!(64); let b = crate::keep!{64}; let c = keep![64];\nlet counts = [0usize; { let n = 64; n }];\nlet typed: [u8; 64] = [0; 64];\nlet generic = std::array::from_fn::<_, 64, _>(|_| 0usize);\nconst N: usize = 64; static S: usize = 64;\nlet constant = const { 64 };\nfn nested() -> usize { 64 }\nlet pattern = match selected { 64 => 1, _ => 0 };\nassert_eq!(a + b + c + N + S + constant + nested(), 448);\nassert_eq!(counts.len() + typed.len() + generic.len(), 192);\nselected + pattern\n}\n}\nfn main() { assert_eq!(Store {}.limit(), 65); }\n";
    let ws = fixture(source);
    assert!(execute(&ws.root(), "syntax exclusions before").0);
    let result = promote(&ws, "64", true, true, false).await.unwrap();
    let changed = ws.read("src/main.rs");
    assert_eq!(result.replaced, 1);
    assert!(changed.contains(
        &source[source.find("let a =").unwrap()..source.find("selected + pattern").unwrap()]
    ));
    assert!(execute(&ws.root(), "syntax exclusions returned").0);
}

#[tokio::test]
async fn real_macro_and_constant_selections_refuse_without_writing() {
    for body in [
        "let n = keep!(64); n",
        "let n = crate::keep!{64}; n",
        "let n = keep![64]; n",
        "let n = [0usize; { let n = 64; n }]; n.len()",
        "let n: [u8; 64] = [0; 64]; n.len()",
        "let n = const { 64 }; n",
        "const N: usize = 64; N",
        "fn nested() -> usize { 64 } nested()",
    ] {
        let source = format!(
            "#[macro_export] macro_rules! keep {{ ($e:expr) => {{ $e }}; }}\nstruct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ {body} }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 64); }}\n"
        );
        let ws = fixture(&source);
        assert!(execute(&ws.root(), "protected selection before").0);
        for force in [false, true] {
            assert!(promote(&ws, "64", false, true, force).await.is_err());
            let unchanged = ws.read("src/main.rs") == source;
            println!("protected selection force={force}; byte_equal={unchanged}");
            assert!(unchanged);
        }
    }
}

#[tokio::test]
async fn runtime_array_elements_and_complex_unary_groups_remain_supported() {
    for expression in [
        "[64; 2][0]",
        "[{let n = 0usize; n}, {let n = 64; n}][1]",
        "(!(!(64 == 0))) as usize + 64",
    ] {
        let source = format!(
            "struct Store {{}}\nimpl Store {{\nfn limit(&self) -> usize {{ {expression} }}\n}}\nfn main() {{ assert_eq!(Store {{}}.limit(), 64); }}\n"
        );
        let ws = fixture(&source);
        assert!(execute(&ws.root(), "runtime ancestry before").0);
        let result = promote(&ws, "64", true, true, false).await.unwrap();
        assert_eq!(
            result.replaced,
            if expression.contains("+ 64") { 2 } else { 1 }
        );
        assert!(execute(&ws.root(), "runtime ancestry returned").0);
    }
}

#[tokio::test]
async fn consumer_edition_and_parse_errors_are_not_discarded() {
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> usize { let async = 64; async }\n}\nfn main() { assert_eq!(Store {}.limit(), 64); }\n";
    let ws = fixture(source);
    ws.write(
        "Cargo.toml",
        "[package]\nname='preservation'\nversion='0.1.0'\nedition='2015'\n",
    );
    // The same bytes are a valid 2015 program and a parse error in 2021.
    assert!(execute_edition(&ws.root(), "edition 2015 before", "2015").0);
    let result = promote(&ws, "64", false, true, false).await.unwrap();
    assert_eq!(result.replaced, 1);
    assert!(execute_edition(&ws.root(), "edition 2015 returned", "2015").0);
    ws.write("src/main.rs", source);
    for manifest in [
        "[package]\nname='preservation'\nversion='0.1.0'\nedition='2021'\n",
        "[package]\nname='preservation'\nversion='0.1.0'\nedition='2027'\n",
    ] {
        ws.write("Cargo.toml", manifest);
        let error = promote(&ws, "64", false, true, true).await.unwrap_err();
        println!(
            "edition/parse refusal: {error}; byte_equal={}",
            ws.read("src/main.rs") == source
        );
        assert_eq!(ws.read("src/main.rs"), source);
    }
}
