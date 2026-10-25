/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "parameter_object_rust_preservation/controls.rs"]
mod controls;
#[path = "parameter_object_rust_preservation/ownership.rs"]
mod ownership;
#[path = "parameter_object_rust_preservation/scopes.rs"]
mod scopes;
#[path = "parameter_object_rust_preservation/support.rs"]
mod support;

use support::preserves;

#[tokio::test]
async fn method_stays_in_module_and_keeps_annotations() {
    preserves(
        "method",
        r#"mod inner {
    pub struct Calculator;
    /// Calculator methods.
    #[allow(dead_code)]
    impl Calculator {

        /// Arithmetic method.
        #[inline(never)]
        pub fn sum(&self, left: i32, right: i32) -> i32 {
            left + right
        }
    }
}
fn main() { println!("{}", inner::Calculator.sum(2, 3)); }
"#,
        &["left", "right"],
        "5\n",
        &[
            "/// Calculator methods.\n    #[allow(dead_code)]\n    impl Calculator",
            "/// Arithmetic method.\n        #[inline(never)]\n        pub fn sum(&self, pair: Pair)",
        ],
    )
    .await;
}

#[tokio::test]
async fn same_line_body_uses_are_not_declarations() {
    preserves(
        "same-line",
        "fn sum(left: i32, right: i32) -> i32 { left + right }\nfn main() { println!(\"{}\", sum(2, 3)); }\n",
        &["left", "right"],
        "5\n",
        &["fn sum(pair: Pair) -> i32 { pair.left + pair.right }"],
    )
    .await;
}

#[tokio::test]
async fn explicit_lifetime_does_not_hide_later_anonymous_borrow() {
    preserves(
        "mixed-lifetimes",
        r#"fn sum(left: (&'static str, &str), right: usize) -> usize {
    left.0.len() + left.1.len() + right
}
fn main() { println!("{}", sum(("ab", "c"), 4)); }
"#,
        &["left", "right"],
        "7\n",
        &["pub left: (&'static str, &'a str)", "pair: Pair<'_>"],
    )
    .await;
}

#[tokio::test]
async fn recursive_arguments_compose_parameter_uses() {
    preserves(
        "recursive",
        r#"fn sum(left: i32, right: i32) -> i32 {
    if left == 0 { return right; }
    sum(
        left - 1,
        right + 1,
    )
}
fn main() { println!("{}", sum(3, 2)); }
"#,
        &["left", "right"],
        "5\n",
        &["sum(Pair { left: pair.left - 1, right: pair.right + 1 })"],
    )
    .await;
}
