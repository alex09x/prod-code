/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::*;

#[test]
fn generic_parameters_refuse_incomplete_syntax() {
    for (text, message) in [
        ("<T", "the generic parameters of `Item` are not closed"),
        (
            "<#[tag] T>",
            "`Item` has an attributed generic parameter on line 1; attributes on generic parameters are not supported",
        ),
        (
            "<T: Self>",
            "the generic parameters of `Item` spells `Self` (line 1), which would name the builder inside its impl; spell `Item` explicitly",
        ),
        (
            "<T: bound!()>",
            "the generic parameters of `Item` invokes a macro on line 1; macro-expanded generic syntax is not supported",
        ),
        (
            "<const N: usize = { 1 }>",
            "the generic parameters of `Item` has an unsupported const expression on line 1; const blocks are not supported",
        ),
        ("<>", "`Item` has an empty generic parameter list"),
        ("<,T>", "`Item` has an empty generic parameter on line 1"),
        (
            "<'a = 'b>",
            "lifetime parameter `'a` of `Item` cannot have a default",
        ),
        (
            "<'a X>",
            "cannot read lifetime parameter `'a` of `Item` on line 1",
        ),
        ("<'a:>", "lifetime parameter `'a` of `Item` has no bound"),
        ("<T:>", "type parameter `T` of `Item` has no bound"),
        ("<const N:>", "const parameter `N` of `Item` has no type"),
        (
            "<const N>",
            "cannot read const parameter `N` of `Item`; expected `const NAME: TYPE`",
        ),
        (
            "<const 1>",
            "cannot read a const parameter of `Item` on line 1",
        ),
        (
            "<T X>",
            "cannot read type parameter `T` of `Item` on line 1; expected a bound or default",
        ),
        ("<1>", "cannot read a generic parameter of `Item` on line 1"),
        (
            "<const N: usize = 1 + 2>",
            "const parameter `N` of `Item` has an unsupported default expression; use a literal or const path",
        ),
    ] {
        let src = Source::new(text).unwrap();
        assert_eq!(
            read_generics(&src, 0, "Item").unwrap_err().to_string(),
            message,
            "{text}"
        );
    }
}

#[test]
fn defaults_are_preserved_only_on_declaration() {
    let nested = Source::new("<T: Trait<Other = [u8; 4]>>").unwrap();
    let (g, end) = read_generics(&nested, 0, "Item").unwrap();
    assert_eq!(end, nested.tokens.len() - 1);
    assert_eq!(g.declaration, nested.text);
    assert_eq!(g.impl_declaration, nested.text);
    assert_eq!(g.arguments, "<T>");
    let src = Source::new("<'a: 'b, T: Fn(u8, u8) = fn(u8, u8), const N: usize = ::limits::MAX,>")
        .unwrap();
    let (g, end) = read_generics(&src, 0, "Item").unwrap();
    assert_eq!(end, src.tokens.len() - 1);
    assert_eq!(g.declaration, src.text);
    assert_eq!(
        g.impl_declaration,
        "<'a: 'b, T: Fn(u8, u8), const N: usize>"
    );
    assert_eq!(g.arguments, "<'a, T, N>");
}
