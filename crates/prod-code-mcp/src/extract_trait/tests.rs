/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::items::impl_block;
use super::rewrite::{declaration, rewrite, signature, visibility};

const SHAPES: &str = "pub struct Rect {\n    pub w: f64,\n}\n\nimpl Rect {\n    pub fn new(w: f64) -> Self {\n        Rect { w }\n    }\n\n    /// The area.\n    #[inline]\n    pub fn area(&self) -> f64 {\n        // a stray } here\n        self.w * self.w\n    }\n\n    pub(crate) fn half(&self) -> f64 {\n        let _ = '}';\n        self.w / 2.0\n    }\n}\n\nfn other() {}\n";

#[test]
fn the_block_and_its_items_are_found_with_their_names() {
    let imp = impl_block(SHAPES, SHAPES.find("impl Rect").unwrap() + 2).unwrap();
    assert_eq!(imp.self_ty, "Rect");
    let names: Vec<_> = imp.items.iter().map(|i| i.name.clone().unwrap()).collect();
    assert_eq!(names, ["new", "area", "half"]);
    assert!(SHAPES[imp.items[1].start..].starts_with("/// The area."));
    assert!(SHAPES[..imp.items[1].end].ends_with("self.w * self.w\n    }"));
    assert_eq!(
        impl_block("impl<T> Foo<T> {}", 0).unwrap().generic_args,
        ["T"]
    );
    assert_eq!(
        impl_block("impl<T: Iterator<Item = u8>> Foo<T> {}", 0)
            .unwrap()
            .generic_args,
        ["T"]
    );
    assert!(impl_block("impl Display for Foo {}", 0).is_err());
}

#[test]
fn a_generic_impl_header_is_preserved_and_reused() {
    let text = "impl<'a, T: Clone, const N: usize> Name<'a, T, N>\nwhere\n    T: Default,\n{\n    fn selected<U: Copy>(&self, value: U) -> Self\n    where\n        U: Into<T>,\n    {\n        let _ = value;\n        todo!()\n    }\n\n    fn kept(&self) -> usize { N }\n}\n";
    let imp = impl_block(text, text.find("impl").unwrap()).expect("generic impl");
    let (out, kept) = rewrite(text, &imp, &["selected".into()], "Selected").unwrap();
    assert_eq!(kept, ["kept"]);
    assert!(
        out.contains("trait Selected<'a, T: Clone, const N: usize>\nwhere\n    T: Default,"),
        "{out}"
    );
    assert!(
        out.contains(
            "impl<'a, T: Clone, const N: usize> Selected<'a, T, N> for Name<'a, T, N>\nwhere\n    T: Default,"
        ),
        "{out}"
    );
    assert!(
        out.contains("fn selected<U: Copy>(&self, value: U) -> Self"),
        "{out}"
    );
    assert!(
        out.contains("impl<'a, T: Clone, const N: usize> Name<'a, T, N>\nwhere\n    T: Default,"),
        "{out}"
    );

    let all = ["selected".into(), "kept".into()];
    let (all_out, kept) = rewrite(text, &imp, &all, "Everything").unwrap();
    assert!(kept.is_empty());
    assert!(!all_out.contains("impl<'a, T: Clone, const N: usize> Name"));
    assert!(all_out.contains("impl<'a, T: Clone, const N: usize> Everything<'a, T, N>"));
}

#[test]
fn generic_refusals_and_exact_replacement_are_explicit() {
    for (source, expected) in [
        ("impl<T = u8> Name<T> {}", "defaults"),
        (
            "impl<T> Trait<T> for Name<T> {}",
            "already implements a trait",
        ),
        ("#[cfg(test)]\nimpl Name {}", "attributes on impl blocks"),
        (
            "#[cfg(any())] impl<T> Name<T> { fn selected(&self) {} fn kept(&self) {} }",
            "attributes on impl blocks",
        ),
        (
            "#[cfg(\n    any()\n)]\n// the condition remains attached\n\nimpl<T> Name<T> { fn selected(&self) {} fn kept(&self) {} }",
            "attributes on impl blocks",
        ),
        (
            "const NOTE: &str = \"} ] #[cfg]\"; /* ] /* } */ [ */ #[cfg(any())] /* [ */ impl Name {}",
            "attributes on impl blocks",
        ),
        ("impl<T> Name<make!{T}> {}", "macros in an impl header"),
        (
            "macro_rules! make { () => { impl<T> Name<T> {} } }",
            "generated inside macros",
        ),
        ("default impl<T> Name<T> {}", "specialized"),
    ] {
        let err = impl_block(source, source.find("impl").unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains(expected), "{source}: {err:#}");
    }

    let sibling = "#[cfg(any())]\nstruct Disabled;\n\nimpl Name { fn value(&self) {} }";
    assert!(
        impl_block(sibling, sibling.find("impl Name").unwrap()).is_ok(),
        "an attribute on a prior sibling is not attached to the impl"
    );
    let sibling_literal = "const NOTE: &str = r#\"impl } ] #[cfg]\"#; /* } ] */ impl Name {}";
    assert!(
        impl_block(sibling_literal, sibling_literal.rfind("impl Name").unwrap()).is_ok(),
        "delimiters in literals and comments do not attach an attribute"
    );

    for source in [
        "impl<T: From<Self>> Name<T> { fn value(&self) {} }",
        "impl<T> Name<T> where T: From<Self> { fn value(&self) {} }",
    ] {
        let err = impl_block(source, source.find("impl").unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("Self"), "{source}: {err:#}");
    }

    let inline =
        "fn before() -> u8 { 1 } impl<T> Name<T> { fn value(&self) -> Self { todo!() } }\n";
    let imp = impl_block(inline, inline.find("impl").unwrap()).unwrap();
    let (out, _) = rewrite(inline, &imp, &["value".into()], "Value").unwrap();
    assert!(
        out.starts_with("fn before() -> u8 { 1 } trait Value<T>"),
        "{out}"
    );
    assert!(out.contains("fn value(&self) -> Self;"), "{out}");

    let err = rewrite(inline, &imp, &["value".into()], "2Value").unwrap_err();
    assert!(format!("{err:#}").contains("valid Rust identifier"));

    for conditional in [
        "impl Name { #[cfg(test)]\nfn value(&self) {} }",
        "impl Name { #[cfg(any())] fn value(&self) {} }",
        "impl Name { #[cfg_attr(\n    any(),\n    allow(dead_code)\n)] fn value(&self) {} }",
    ] {
        let imp = impl_block(conditional, 0).unwrap();
        let err = rewrite(conditional, &imp, &["value".into()], "Value").unwrap_err();
        assert!(
            format!("{err:#}").contains("conditional"),
            "{conditional}: {err:#}"
        );
    }
}

#[test]
fn comments_and_literals_do_not_change_item_structure() {
    let text = r###"impl Name {
    /* } /* nested ] */ } */
    #[inline]
    fn value(&self) {
        let _ = "} ]";
        let _ = b"} ]";
        let _ = c"} ]";
        let _ = r#"} ]"#;
        let _ = br#"} ]"#;
        let _ = cr#"} ]"#;
        let _ = '}';
        let _ = b'}';
        let _ = '\x7d';
        let _ = '\u{7d}';
        let _ = '\'';
    }
}"###;
    let imp = impl_block(text, 0).unwrap();
    assert_eq!(imp.items.len(), 1);
    assert_eq!(imp.items[0].name.as_deref(), Some("value"));
    let (out, kept) = rewrite(text, &imp, &["value".into()], "Value").unwrap();
    assert!(kept.is_empty());
    assert!(out.contains("trait Value"), "{out}");
    assert!(out.contains("let _ = r#\"} ]\"#;"), "{out}");
}

#[test]
fn a_signature_loses_its_visibility_and_body() {
    assert_eq!(visibility("pub(crate) fn f()"), ("pub(crate)", "fn f()"));
    assert_eq!(visibility("pub fn f()"), ("pub", "fn f()"));
    assert_eq!(visibility("fn f()"), ("", "fn f()"));
    assert_eq!(
        signature("fn get<T: Into<u8>>(&self, t: T) -> Option<u8> where T: Copy {"),
        Some("fn get<T: Into<u8>>(&self, t: T) -> Option<u8> where T: Copy")
    );
    let (leading, decl) = declaration("/// Doc.\n    #[inline]\n    fn f() {}");
    assert_eq!(
        (leading, decl),
        (vec!["/// Doc.", "#[inline]"], "fn f() {}")
    );
}

#[test]
fn only_the_named_methods_move_and_the_trait_is_as_visible_as_the_widest() {
    let imp = impl_block(SHAPES, SHAPES.find("impl Rect").unwrap()).unwrap();
    let (out, kept) = rewrite(SHAPES, &imp, &["area".into(), "half".into()], "Measure").unwrap();
    assert_eq!(kept, ["new"]);
    assert!(out.contains("impl Rect {\n    pub fn new(w: f64) -> Self {\n        Rect { w }\n    }\n}\n\npub trait Measure {\n    /// The area.\n    fn area(&self) -> f64;\n\n    fn half(&self) -> f64;\n}\n\nimpl Measure for Rect {\n    #[inline]\n    fn area(&self) -> f64 {\n        // a stray } here\n        self.w * self.w\n    }\n\n    fn half(&self) -> f64 {\n        let _ = '}';\n        self.w / 2.0\n    }\n}\n\nfn other() {}\n"), "{out}");

    // Every method taken: no empty `impl Rect {}` is left behind.
    let all: Vec<String> = ["new", "area", "half"].map(String::from).to_vec();
    let (out, kept) = rewrite(SHAPES, &imp, &all, "Measure").unwrap();
    assert!(kept.is_empty() && !out.contains("impl Rect {"), "{out}");
    let (out, _) = rewrite(SHAPES, &imp, &["half".into()], "Halve").unwrap();
    assert!(out.contains("pub(crate) trait Halve {"), "{out}");

    let err = rewrite(SHAPES, &imp, &["volume".into()], "M").unwrap_err();
    assert!(format!("{err}").contains("has no method `volume`; it has new, area, half"));
}

mod position_and_capture_tests {
    use super::*;

    #[test]
    fn macro_wrappers_are_refused_for_every_delimiter() {
        for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
            let text = format!(
                "struct Name; macro_rules! make {{ ($item:item) => {{ $item }} }} make!{open}impl Name {{ fn value(&self) {{}} }}{close};"
            );
            let error = impl_block(&text, text.find("impl Name").unwrap()).unwrap_err();
            assert!(error.to_string().contains("inside macros"), "{error:#}");
        }
        let definition = "macro_rules!\n make\n { () => { impl Name { fn value(&self) {} } } }";
        assert!(impl_block(definition, definition.find("impl Name").unwrap()).is_err());
        let normal = "fn main() { if !flag { impl Name { fn value(&self) {} } } }";
        assert!(impl_block(normal, normal.find("impl Name").unwrap()).is_ok());
    }

    #[test]
    fn unicode_header_positions_never_split_source_characters() {
        let source = "struct 名字;\nimpl 名字 { fn value(&self) {} }\n";
        let begin = source.find("impl").unwrap();
        let end = source[begin..].find('{').unwrap() + begin;
        for at in begin..=end {
            if source.is_char_boundary(at) {
                let block = impl_block(source, at).unwrap();
                assert_eq!(block.self_ty, "名字");
                assert!(rewrite(source, &block, &["value".into()], "Value").is_ok());
            } else {
                assert!(impl_block(source, at).is_err());
            }
        }
        assert!(impl_block(source, usize::MAX).is_err());
    }

    #[test]
    fn opaque_returns_are_refused_before_their_capture_contract_changes() {
        for method in [
            "fn value(&self) -> impl Copy { 7_u8 }",
            "fn value(&self) -> Option<impl Copy> { Some(7_u8) }",
            "fn value(&self) -> impl /* receiver would be captured */ Copy { 7_u8 }",
        ] {
            let source = format!("struct Example; impl Example {{ {method} }}");
            let block = impl_block(&source, source.find("impl Example").unwrap()).unwrap();
            let error = rewrite(&source, &block, &["value".into()], "Value").unwrap_err();
            assert!(error.to_string().contains("opaque return"), "{error:#}");
        }
        for method in [
            "fn value(&self, input: impl Copy) -> u8 { let _ = input; 7 }",
            "fn value(&self) -> /* impl Copy */ Self { Example }",
            "fn value(&self) -> u8 { let _ = \"impl Copy\"; 7 }",
        ] {
            let source = format!("struct Example; impl Example {{ {method} }}");
            let block = impl_block(&source, source.find("impl Example").unwrap()).unwrap();
            assert!(rewrite(&source, &block, &["value".into()], "Value").is_ok());
        }
    }

    #[test]
    fn cursor_selects_the_innermost_actual_impl_item() {
        let source = "struct Example; impl Example { fn value(&self, input: impl Copy) -> u8 { let \u{732b} = input; let _ = \u{732b}; 7 } }";
        for at in [
            source.find("impl Example").unwrap() + 2,
            source.find("let _").unwrap(),
        ] {
            let block = impl_block(source, at).unwrap();
            assert_eq!(block.self_ty, "Example");
            assert_eq!(block.start, source.find("impl Example").unwrap());
            assert_eq!(block.close, source.len() - 1);
        }

        let nested_closed = "struct Example; impl Example { fn value(&self, input: impl Copy) -> u8 { { impl Closed {} } let _ = input; 7 } }";
        assert_eq!(
            impl_block(nested_closed, nested_closed.find("let _").unwrap())
                .unwrap()
                .self_ty,
            "Example"
        );
        let raw_identifier = "struct Example; impl Example { fn value(&self, input: impl Copy) -> u8 { let r#impl = input; let _ = r#impl; 7 } }";
        assert_eq!(
            impl_block(raw_identifier, raw_identifier.find("let _").unwrap())
                .unwrap()
                .self_ty,
            "Example"
        );

        let comments_and_literals = "struct Example; impl Example { fn value(&self) { let note = \"impl NotAnItem {\"; // impl AlsoNotAnItem {\n let _ = note; } }";
        assert_eq!(
            impl_block(
                comments_and_literals,
                comments_and_literals.find("let _").unwrap()
            )
            .unwrap()
            .self_ty,
            "Example"
        );

        let nested_trait = "struct Example; trait Inner {} impl Example { fn value(&self) { impl Inner for Nested {} } }";
        let error = impl_block(nested_trait, nested_trait.find("impl Inner").unwrap()).unwrap_err();
        assert!(
            error.to_string().contains("already implements a trait"),
            "{error:#}"
        );
        assert!(impl_block("fn outside() {}", 3).is_err());
    }
}

mod parenthesized_item_probe {
    use super::*;

    #[test]
    fn an_impl_inside_a_block_expression_is_still_an_item() {
        for source in [
            "fn main() { let _ = ({ struct Local; impl Local { fn value(&self) {} } 0 }); }",
            "fn main() { let _ = [{ struct Local; impl Local { fn value(&self) {} } 0 }]; }",
        ] {
            let block = impl_block(source, source.find("impl Local").unwrap())
                .expect("an item inside an expression block is valid");
            assert_eq!(block.self_ty, "Local");
        }
    }
}
