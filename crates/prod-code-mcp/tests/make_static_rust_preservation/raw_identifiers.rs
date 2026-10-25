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

#[test]
fn impl_keywords_require_whole_non_raw_identifier_tokens() {
    use prod_code_mcp::make_static::helpers::rust_impl_is_trait;

    // Scanner controls only: raw self types remain outside the locator's support.
    for name in [
        "r#where",
        "r#for",
        "r#whereλ",
        "r#where\u{301}",
        "λwhere",
        "whereλ",
    ] {
        assert!(rust_impl_is_trait(&format!("impl {name} for S")));
        assert!(!rust_impl_is_trait(&format!("impl {name}")));
    }
    assert!(!rust_impl_is_trait("impl S where F: for<'a> Fn(&'a str)"));
}

#[tokio::test]
async fn raw_identifier_trait_impls_refuse_before_planning_or_writing() {
    let mut violations = Vec::new();
    for name in [
        "r#where",
        "r#for",
        "r#whereλ",
        "r#where\u{301}",
        "λwhere",
        "whereλ",
    ] {
        let source = format!(
            "struct S;\ntrait {name} {{ fn consume(&self); }}\nimpl {name} for S {{\n    fn consume(&self) {{}}\n}}\nfn main() {{}}\n"
        );
        println!("TRAIT {name}: original source\n{source}");
        assert_eq!(run(&source, name), "");
        for (apply, force) in [(false, false), (false, true), (true, false), (true, true)] {
            let ws = workspace(&source);
            let remote = gateway(serde_json::json!([])).await;
            let (line, col) = position(&source, "consume(&self) {}");
            let result = prod_code_mcp::make_static::make_static(
                remote,
                &ws.root(),
                &ws.path("src/main.rs"),
                line,
                col,
                apply,
                force,
            )
            .await;
            let label = format!("{name}, apply={apply}, force={force}");
            match result {
                Err(err) => {
                    println!("TRAIT {label}: {err:#}");
                    if !err.to_string().contains("implements a trait method") {
                        violations.push(format!("{label}: wrong refusal: {err:#}"));
                    }
                }
                Ok(plan) => {
                    println!("TRAIT {label}: accepted\n{}", plan.render(12000));
                    violations.push(format!("{label}: accepted, applied={}", plan.applied));
                }
            }
            if ws.read("src/main.rs") != source {
                println!("TRAIT {label}: changed source\n{}", ws.read("src/main.rs"));
                violations.push(format!("{label}: source bytes changed"));
            } else {
                untouched(&ws, &source);
            }
        }
    }
    assert!(
        violations.is_empty(),
        "trait refusals bypassed:\n{}",
        violations.join("\n")
    );
}
