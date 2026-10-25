/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::preserves;

#[tokio::test]
async fn leading_shebang_before_first_inline_module() {
    for (label, prefix) in [
        ("shebang", "#!/usr/bin/env rust-script\n"),
        (
            "bom-shebang",
            "\u{feff}#!/usr/bin/env rust-script ' { mod fake\n",
        ),
    ] {
        preserves(
            label,
            &format!(
                "{prefix}/// Keep module docs.\n#[allow(dead_code)]\nmod nested {{\n    pub fn sum(left: i32, right: i32) -> i32 {{ left + right }}\n}}\nfn main() {{ println!(\"{{}}\", nested::sum(2, 3)); }}\n"
            ),
            &["left", "right"],
            "5\n",
            &[
                "/// Keep module docs.\n#[allow(dead_code)]\nmod nested",
                "nested::sum(nested::Pair { left: 2, right: 3 })",
            ],
        )
        .await;
    }
}

#[tokio::test]
async fn leading_shebang_before_root_function() {
    utf8_preamble("root-shebang", "\u{feff}#!/usr/bin/env rust-script\n").await;
}

#[tokio::test]
async fn leading_inner_attribute_is_not_a_shebang() {
    preserves(
        "inner-attribute",
        "#! /* attribute trivia */\n[allow(dead_code)]\nfn sum(left: i32, right: i32) -> i32 { left + right }\nfn main() { println!(\"{}\", sum(2, 3)); }\n",
        &["left", "right"],
        "5\n",
        &["#! /* attribute trivia */\n[allow(dead_code)]"],
    )
    .await;
}

#[tokio::test]
async fn leading_bom_keeps_character_boundaries() {
    utf8_preamble("bom", "\u{feff}").await;
}

#[tokio::test]
async fn multibyte_whitespace_keeps_character_boundaries() {
    utf8_preamble("unicode-whitespace", "\u{200e}").await;
}

async fn utf8_preamble(label: &str, prefix: &str) {
    preserves(
        label,
        &format!(
            "{prefix}fn sum(left: i32, right: i32) -> i32 {{ left + right }}\nfn main() {{ println!(\"{{}}\", sum(2, 3)); }}\n"
        ),
        &["left", "right"],
        "5\n",
        &["fn sum(pair: Pair)"],
    )
    .await;
}

#[tokio::test]
async fn block_expression_statements_allow_following_module_items() {
    for (label, expression) in [
        ("if", "if true {}"),
        ("if-else", "if false {} else {}"),
        ("match", "match 1 { _ => {} }"),
        ("loop", "loop { break; }"),
        ("while", "while false {}"),
        ("for", "for _ in 0..0 {}"),
        ("if-let", "if let Some(()) = { Some(()) } {}"),
        ("block", "{}"),
        ("unsafe", "unsafe {}"),
        ("const-block", "const {}"),
        ("labeled-loop", "'done: loop { break 'done; }"),
    ] {
        preserves(
            label,
            &format!(
                "fn main() {{ {expression}\n    /// Keep module docs.\n    #[allow(dead_code)]\n    mod nested {{\n        /// Keep function docs.\n        #[inline(never)]\n        pub fn sum(left: i32, right: i32) -> i32 {{ left + right }}\n    }}\n    println!(\"{{}}\", nested::sum(2, 3));\n}}\n"
            ),
            &["left", "right"],
            "5\n",
            &[
                "/// Keep module docs.\n    #[allow(dead_code)]\n    mod nested",
                "/// Keep function docs.\n        #[inline(never)]\n        pub fn sum(pair: Pair)",
                "nested::sum(nested::Pair { left: 2, right: 3 })",
            ],
        )
        .await;
    }
}

#[tokio::test]
async fn braced_const_impl_header_is_not_an_item_boundary() {
    preserves(
        "const-header",
        r#"struct Calculator<const N: usize>;
impl Calculator<{ 1 + 1 }> {
    fn sum(&self, left: i32, right: i32) -> i32 { left + right }
}
fn main() { println!("{}", Calculator::<2>.sum(2, 3)); }
"#,
        &["left", "right"],
        "5\n",
        &["impl Calculator<{ 1 + 1 }> {", "fn sum(&self, pair: Pair)"],
    )
    .await;
}

#[tokio::test]
async fn raw_type_impl_is_not_module_ancestry() {
    preserves(
        "raw-type",
        r#"fn sum(left: i32, right: i32) -> i32 {
    left + right
}
#[allow(non_camel_case_types)]
struct r#mod<T>(T);
impl r#mod<i32> {
    fn call() -> i32 { sum(2, 3) }
}
fn main() { println!("{}", r#mod::<i32>::call()); }
"#,
        &["left", "right"],
        "5\n",
        &["fn call() -> i32 { sum(Pair { left: 2, right: 3 }) }"],
    )
    .await;
}

#[tokio::test]
async fn raw_unicode_and_sibling_modules_keep_real_paths() {
    preserves(
        "module-paths",
        r#"/// Outer module docs.
#[allow(dead_code)]
pub(crate) mod r#type {
    //! Inner module docs.
    #![allow(dead_code)]
    pub mod café {
        /// Keep this on the function.
        #[inline(never)]
        pub fn sum(left: i32, right: i32) -> i32 { left + right }
    }
    mod sibling {
        pub fn call() -> i32 { super::café::sum(2, 3) }
    }
    pub fn call() -> i32 { sibling::call() }
}
mod callers {
    pub fn call() -> i32 { crate::r#type::café::sum(3, 4) }
}
fn main() { println!("{} {} {}", r#type::call(), callers::call(), r#type::café::sum(4, 5)); }
"#,
        &["left", "right"],
        "5 7 9\n",
        &[
            "/// Outer module docs.\n#[allow(dead_code)]\npub(crate) mod r#type",
            "/// Keep this on the function.\n        #[inline(never)]\n        pub fn sum(pair: Pair)",
            "super::café::Pair { left: 2, right: 3 }",
            "super::r#type::café::Pair { left: 3, right: 4 }",
            "r#type::café::Pair { left: 4, right: 5 }",
        ],
    )
    .await;
}

#[tokio::test]
async fn expression_items_before_nested_module_do_not_move_its_boundary() {
    preserves(
        "expression-items",
        r#"macro_rules! truth { () => { true }; }
const unsafe extern "C" fn enabled() -> bool { true }
const FLAG: bool = unsafe { enabled() } && 1 < 2;
const OTHER: bool = truth! {};
const COUNT: usize = { if FLAG && OTHER { 2 } else { 1 } };
type SizedArray = [u8; { COUNT }];
mod nested {
    //! Module docs must precede the type.
    #![allow(dead_code)]
    pub struct Calculator<const N: usize>;
    /// Keep this impl's docs.
    #[allow(dead_code)]
    impl Calculator<{ ({ 1 }) + 1 }> {
        pub fn sum(&self, left: i32, right: i32) -> i32 { left + right }
    }
}
fn main() {
    let _: SizedArray = [0; COUNT];
    println!("{}", nested::Calculator::<2>.sum(2, 3));
}
"#,
        &["left", "right"],
        "5\n",
        &[
            "//! Module docs must precede the type.\n    #![allow(dead_code)]",
            "/// Keep this impl's docs.\n    #[allow(dead_code)]\n    impl Calculator<{ ({ 1 }) + 1 }>",
            "pub fn sum(&self, pair: Pair)",
        ],
    )
    .await;
}
