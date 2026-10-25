/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{compiles, compiles_value, fixture};
use prod_code_mcp::fixture::parse_shape;
use prod_code_testkit::Workspace;

#[tokio::test]
async fn multiline_field_attribute_preserves_complete_record() {
    let source = "struct S {\n    #[allow(\n        dead_code\n    )]\n    a: u32,\n}\n";
    // Prove the source and expected complete value are valid even when public
    // generation fails before it can return a value for the generated probe.
    let control = Workspace::new(&[("src/lib.rs", source)]);
    println!("typed reference control: S {{ a: 0 }}");
    compiles_value(
        &control,
        source,
        "S",
        "S { a: 0 }",
        "assert_eq!(value.a, 0);",
    );
    let (ws, result) = fixture(source, "S", 2).await;
    compiles(&ws, source, &result, "assert_eq!(value.a, 0);");
    assert!(result.value.contains("a: 0"));
    assert!(result.fallbacks.is_empty());
}

#[tokio::test]
async fn restricted_visibility_keeps_multiline_attribute_record_boundary() {
    for visibility in ["pub(crate)", "pub(in crate)", "pub(self)", "pub"] {
        let source = format!(
            "  {visibility} struct S {{\n #[doc =\n \"}}\"\n ]\n a: u32,\n}}\nstruct Next;\n"
        );
        let control = Workspace::new(&[("src/lib.rs", &source)]);
        println!("visibility: {visibility}; typed reference control: S {{ a: 0 }}");
        compiles_value(
            &control,
            &source,
            "S",
            "S { a: 0 }",
            "assert_eq!(value.a, 0);",
        );
        let (ws, result) = fixture(&source, "S", 2).await;
        compiles(&ws, &source, &result, "assert_eq!(value.a, 0);");
        assert!(result.value.contains("a: 0"));
        assert!(result.fallbacks.is_empty());
    }
}

#[tokio::test]
async fn attribute_literals_and_comments_do_not_consume_following_fields() {
    let source = r####"struct Decorated {
    before: u32, // #[not_an_attribute( ]
    #[doc = r##"
        // ] ) } these are documentation, not token boundaries
        #[ still documentation
    "##]
    #[doc = "comment boundary"] // ] ) } after an attribute
    #[allow(
        dead_code, // ] ) } ignored by the lexer
        /* ] /* nested ) */ } */
        reason = "escaped quote \" and delimiters ] ) } //"
    )] #[doc = "inline attribute"] r#type: u32, // trailing comment
    // A comment after the attribute must not swallow this field.
    after: bool,
}
"####;
    let (ws, result) = fixture(source, "Decorated", 2).await;
    compiles(
        &ws,
        source,
        &result,
        "assert_eq!(value.before + value.r#type, 0); assert!(!value.after);",
    );
    for field in ["before: 0", "r#type: 0", "after: false"] {
        assert!(result.value.contains(field), "{}", result.value);
    }
    assert!(result.fallbacks.is_empty());
}

#[tokio::test]
async fn multiline_attributes_preserve_enum_payload_shapes() {
    for (source, assertion) in [
        (
            "enum Event {\n #[allow(\n dead_code\n )]\n Data { #[doc = \"field\"] r#type: u8, flag: bool },\n}\n",
            "assert!(matches!(value, Event::Data { r#type: 0, flag: false }));",
        ),
        (
            "enum Event {\n #[allow(\n dead_code\n )]\n Data(#[doc = \"field\"] u8, bool),\n}\n",
            "assert!(matches!(value, Event::Data(0, false)));",
        ),
    ] {
        let (ws, result) = fixture(source, "Event", 2).await;
        compiles(&ws, source, &result, assertion);
        assert!(result.fallbacks.is_empty());
    }
}

#[test]
fn unreadable_or_conditional_attributes_remain_unsupported() {
    for source in [
        "struct S { #[allow(\n dead_code\n ) a: u32, }",
        "struct S { #[ }",
        "struct S { #[allow(dead_code]] a: u32, }",
        "struct S { #[doc = \"unterminated] a: u32, }",
        "struct S { #![allow(dead_code)] a: u32, }",
        "struct S { #[cfg(any())] a: u32, b: bool }",
        "enum Event { #[cfg_attr(any(), doc = \"hidden\")] Data(u8) }",
    ] {
        assert!(parse_shape(source).is_none(), "{source}");
    }
}
