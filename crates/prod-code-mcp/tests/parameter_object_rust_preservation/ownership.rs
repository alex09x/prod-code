/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{Fixture, evidence, preserves};
use std::io::Write;

#[tokio::test]
async fn ownership_previous_return_type_macro_keeps_real_body() {
    check(
        "header-macro",
        "macro_rules! ty { () => { u32 }; }\nfn prior() -> ty!{} { 1 }\nfn sum(left: i32, right: i32) -> i32 { left + right }\nfn main() { println!(\"{} {}\", prior(), sum(2, 3)); }\n",
        "1 5\n",
        "fn prior() -> ty!{} { 1 }",
    )
    .await;
}

#[tokio::test]
async fn ownership_unicode_trivia_keeps_all_module_inner_attributes() {
    check(
        "attribute-trivia",
        "mod inner {\n#!\u{200e}[allow(dead_code)]\n#! /* keep trivia */\u{200f}[allow(unused_imports)]\npub fn sum(left: i32, right: i32) -> i32 { left + right }\n}\nfn main() { println!(\"{}\", inner::sum(2, 3)); }\n",
        "5\n",
        "#!\u{200e}[allow(dead_code)]\n#! /* keep trivia */\u{200f}[allow(unused_imports)]",
    )
    .await;
}

#[tokio::test]
async fn ownership_macro_delimiters_and_never_type_keep_headers() {
    for (label, return_type) in [
        ("macro-parens", "ty!()"),
        ("macro-brackets", "ty![]"),
        ("macro-comment", "ty /* path */ ! /* tree */ {}"),
    ] {
        preserves(
            label,
            &format!(
                "macro_rules! ty {{ () => {{ u32 }}; }}\nfn prior() -> {return_type} {{ 1 }}\n#[allow(dead_code)]\nfn diverges() -> ! {{ panic!() }}\nfn sum(left: i32, right: i32) -> i32 {{ left + right }}\nfn main() {{ println!(\"{{}} {{}}\", prior(), sum(2, 3)); }}\n"
            ),
            &["left", "right"],
            "1 5\n",
            &[
                &format!("fn prior() -> {return_type} {{ 1 }}"),
                "#[allow(dead_code)]\nfn diverges() -> ! { panic!() }",
            ],
        )
        .await;
    }
}

// Exercise checked apply even when compilation fails, retaining the original
// refusal as evidence instead of stopping at a generated-source assertion.
async fn check(label: &str, source: &str, output: &str, preserved: &str) {
    let original = evidence(&format!("{label} original"), source);
    assert_eq!(original.status, 0);
    assert_eq!(original.stdout, output);
    let fixture = Fixture::new(source, &["left", "right"], false, false).await;
    let plan = fixture.introduce(false).await.expect("public planning");
    assert!(!plan.applied);
    assert_eq!(fixture.ws.read("src/main.rs"), source);
    let generated = &plan
        .rewritten
        .iter()
        .find(|(path, _)| path == fixture.ws.path("src/main.rs").to_str().unwrap())
        .unwrap()
        .1;
    let compiled = evidence(&format!("{label} generated"), generated);
    let applied = fixture.introduce(true).await;
    let (did_apply, detail) = match &applied {
        Ok(result) => (
            result.applied,
            format!("diagnostics={:?}", result.diagnostics),
        ),
        Err(error) => (false, format!("error={error}")),
    };
    let unchanged = fixture.ws.read("src/main.rs") == source;
    writeln!(
        std::io::stdout().lock(),
        "{label}: preview diagnostics={:?}\ngenerated source:\n{generated}\nchecked apply force=false applied={did_apply} unchanged={unchanged} {detail}",
        plan.diagnostics,
    )
    .unwrap();
    if !did_apply {
        assert!(unchanged, "refused apply must preserve original bytes");
    }
    assert_eq!(compiled.status, 0, "generated program must compile");
    assert_eq!(compiled.stdout, original.stdout);
    assert!(plan.diagnostics.is_empty(), "{:?}", plan.diagnostics);
    assert!(generated.contains(preserved), "item ownership changed");
    assert!(did_apply, "valid plan must apply without force: {detail}");
    assert_eq!(fixture.ws.read("src/main.rs"), *generated);
}
