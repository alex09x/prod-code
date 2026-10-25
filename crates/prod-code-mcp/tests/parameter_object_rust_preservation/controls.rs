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

#[tokio::test]
async fn ordinary_free_function_keeps_adjacent_docs_and_multiline_attributes() {
    preserves("annotations", r#"#![allow(dead_code)]
//! Crate documentation.
const PRIOR: i32 = 1;
/// Arithmetic function.
#[cfg_attr(
    all(),
    inline(never)
)]
fn sum(left: i32, right: i32) -> i32 {
    left + right
}
fn main() { println!("{}", sum(2, 3)); }
"#, &["left", "right"], "5\n", &[
        "/// Arithmetic function.\n#[cfg_attr(\n    all(),\n    inline(never)\n)]\nfn sum(pair: Pair)",
        "#![allow(dead_code)]\n//! Crate documentation.",
    ]).await;
}

#[tokio::test]
async fn module_inner_docs_and_outer_block_docs_stay_attached() {
    preserves(
        "module-docs",
        r#"mod inner {
    /**/ /* Module preamble. */
    /*! Module documentation. */
    /** Function documentation. */
    #[inline(never)]
    pub fn sum(left: i32, right: i32) -> i32 {
        left + right
    }
}
fn main() { println!("{}", inner::sum(2, 3)); }
"#,
        &["left", "right"],
        "5\n",
        &[
            "/** Function documentation. */\n    #[inline(never)]\n    pub fn sum(pair: Pair)",
            "inner::sum(inner::Pair { left: 2, right: 3 })",
        ],
    )
    .await;
}

#[tokio::test]
async fn multiple_and_nested_recursive_calls_are_composed_inside_out() {
    preserves("nested-recursion", r#"fn sum(left: i32, right: i32) -> i32 {
    if left == 0 { return right; }
    if left == 1 { return sum(0, right + 1); }
    sum(
        0,
        sum(left - 1, right + 1),
    )
}
fn main() { println!("{}", sum(3, 2)); }
"#, &["left", "right"], "5\n", &[
        "sum(Pair { left: 0, right: pair.right + 1 })",
        "sum(Pair { left: 0, right: sum(Pair { left: pair.left - 1, right: pair.right + 1 }) })",
    ]).await;
}

#[tokio::test]
async fn recursive_receiver_and_unbundled_argument_are_retained() {
    preserves(
        "receiver-recursion",
        r#"struct Calculator;
impl Calculator {
    fn sum(&self, left: i32, right: i32, kept: i32) -> i32 {
        if left == 0 { return right + kept; }
        self.sum(left - 1, right + 1, kept)
    }
}
fn main() { println!("{}", Calculator.sum(3, 2, 4)); }
"#,
        &["left", "right"],
        "9\n",
        &[
            "fn sum(&self, pair: Pair, kept: i32)",
            "self.sum(Pair { left: pair.left - 1, right: pair.right + 1 }, kept)",
        ],
    )
    .await;
}

#[tokio::test]
async fn generics_and_named_lifetimes_on_unbundled_parameters_are_preserved() {
    preserves(
        "generic-unbundled",
        r#"fn sum<'b, T>(left: i32, right: i32, kept: &'b T) -> i32 {
    let _ = kept;
    left + right
}
fn main() { println!("{}", sum(2, 3, &())); }
"#,
        &["left", "right"],
        "5\n",
        &["fn sum<'b, T>(pair: Pair, kept: &'b T)"],
    )
    .await;
}

#[tokio::test]
async fn explicit_and_multiple_anonymous_borrows_keep_static_lifetimes() {
    preserves(
        "borrow-chain",
        r#"fn sum(left: (&'static str, &str, &'static str, &str), right: &str) -> usize {
    left.0.len() + left.1.len() + left.2.len() + left.3.len() + right.len()
}
fn main() { println!("{}", sum(("a", "bb", "ccc", "dddd"), "eeeee")); }
"#,
        &["left", "right"],
        "15\n",
        &[
            "pub left: (&'static str, &'a str, &'static str, &'a str)",
            "pub right: &'a str",
        ],
    )
    .await;
}

#[tokio::test]
async fn owned_fields_keep_observable_drop_order() {
    preserves(
        "drop-order",
        r#"struct Token(&'static str);
impl Drop for Token {
    fn drop(&mut self) { println!("drop {}", self.0); }
}
fn sum(left: Token, right: Token) -> usize {
    left.0.len() + right.0.len()
}
fn main() { println!("{}", sum(Token("a"), Token("bb"))); }
"#,
        &["left", "right"],
        "drop bb\ndrop a\n3\n",
        &["pub right: Token,\n    pub left: Token"],
    )
    .await;
}

#[tokio::test]
async fn unmatched_and_failed_reference_queries_refuse_without_writing() {
    let source = "fn sum(left: i32, right: i32) -> i32 { left + right }\nfn main() { println!(\"{}\", sum(2, 3)); }\n";
    assert_eq!(evidence("refusal original", source).status, 0);
    for (unmatched, failed) in [(true, false), (false, true)] {
        let fixture = Fixture::new(source, &["left", "right"], unmatched, failed).await;
        let error = fixture.introduce(true).await.unwrap_err();
        let error = format!("{error:#}");
        assert!(
            error.contains(if unmatched {
                "reference(s) were not rewritten"
            } else {
                "references to `left`"
            }),
            "{error}"
        );
        assert_eq!(fixture.ws.read("src/main.rs"), source);
        println!("refusal unmatched={unmatched} failed_refs={failed}: bytes unchanged");
    }
}

#[tokio::test]
async fn actual_compiler_errors_refuse_apply_and_leave_bytes_unchanged() {
    let source = r#"fn sum<'b>(left: &'b str, right: &'b str) -> usize {
    left.len() + right.len()
}
fn main() { println!("{}", sum("ab", "c")); }
"#;
    assert_eq!(evidence("named-lifetime original", source).status, 0);
    let fixture = Fixture::new(source, &["left", "right"], false, false).await;
    let plan = fixture.introduce(false).await.unwrap();
    let generated = &plan.rewritten[0].1;
    assert_ne!(
        evidence("named-lifetime refused proposal", generated).status,
        0
    );
    assert!(!plan.diagnostics.is_empty());
    let error = fixture.introduce(true).await.unwrap_err().to_string();
    assert!(error.contains("does not compile"), "{error}");
    assert_eq!(fixture.ws.read("src/main.rs"), source);
}

#[tokio::test]
async fn effect_and_drop_interleaving_still_refuse() {
    let sources = [
        (
            r#"fn mark(value: i32) -> i32 { println!("{}", value); value }
fn sum(left: i32, middle: i32, right: i32) -> i32 {
    left + middle + right
}
fn main() { println!("{}", sum(mark(1), mark(2), mark(3))); }
"#,
            "evaluated before",
        ),
        (
            r#"fn sum(left: String, middle: String, right: String) -> usize {
    left.len() + middle.len() + right.len()
}
fn main() { println!("{}", sum(String::from("a"), String::from("b"), String::from("c"))); }
"#,
            "drop",
        ),
    ];
    for (source, reason) in sources {
        assert_eq!(evidence("safety original", source).status, 0);
        let fixture = Fixture::new(source, &["left", "right"], false, false).await;
        let error = fixture.introduce(true).await.unwrap_err().to_string();
        assert!(error.contains(reason), "{error}");
        assert_eq!(fixture.ws.read("src/main.rs"), source);
        println!("safety refusal {reason}: bytes unchanged");
    }
}
