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
use crate::replace_constructor::types::FieldDecl;

#[test]
fn coverage_factory_languages() {
    let mut decl = StructDecl {
        name: "Item".into(),
        language: String::new(),
        fields: vec![FieldDecl {
            name: "value".into(),
            ty: "int".into(),
            vis: String::new(),
        }],
        generics: None,
        is_pub: false,
        decl_start: 0,
        decl_end: 0,
        line: 1,
        col: 1,
    };
    for (language, expected) in [
        (
            "rust",
            "\n\nimpl Item {\n    fn create(value: int) -> Self {\n        Self {\n            value,\n        }\n    }\n}",
        ),
        (
            "go",
            "\n\nfunc create(value int) *Item {\n    return &Item{\n        value: value,\n    }\n}",
        ),
        (
            "typescript",
            "\n    static create(value: int): Item {\n        return new Item(value);\n    }\n",
        ),
        (
            "typescriptreact",
            "\n    static create(value: int): Item {\n        return new Item(value);\n    }\n",
        ),
        (
            "javascript",
            "\n    static create(value) {\n        return new Item(value);\n    }\n",
        ),
        (
            "javascriptreact",
            "\n    static create(value) {\n        return new Item(value);\n    }\n",
        ),
        (
            "python",
            "\n    @classmethod\n    def create(cls, value: int) -> \"Item\":\n        return cls(value=value)\n",
        ),
        (
            "cpp",
            "\n    static Item create(int value) {\n        return Item{value};\n    }\n",
        ),
        (
            "c",
            "\n    static Item create(int value) {\n        return Item{value};\n    }\n",
        ),
        (
            "swift",
            "\n    static func create(value: int) -> Item {\n        return Item(value: value)\n    }\n",
        ),
        ("unknown", ""),
    ] {
        decl.language = language.into();
        assert_eq!(
            generate_factory_code(&decl, "create"),
            expected,
            "{language}"
        );
    }
    decl.language = "rust".into();
    decl.is_pub = true;
    decl.generics = Some("<T: Clone = String>".into());
    decl.fields[0].ty = "T".into();
    assert_eq!(
        generate_factory_code(&decl, "create"),
        "\n\nimpl <T: Clone> Item<T> {\n    pub fn create(value: T) -> Self {\n        Self {\n            value,\n        }\n    }\n}"
    );
}

#[test]
fn coverage_rust_generics_and_nested_defaults() {
    for (input, header, args) in [
        ("", "", ""),
        ("<>", "", ""),
        ("<, ,>", "", ""),
        ("<: Broken>", "", ""),
        ("T", "<T> ", "<T>"),
        (
            "<'a: 'b, T: Iterator<Item = u8> = Vec<u8>, const N: usize = 4>",
            "<'a: 'b, T: Iterator<Item = u8>, const N: usize> ",
            "<'a, T, N>",
        ),
    ] {
        assert_eq!(
            rust_impl_generics(input),
            (header.into(), args.into()),
            "{input}"
        );
    }
    for (input, expected) in [
        ("T: Trait<A = B> = C", "T: Trait<A = B> "),
        ("T: Fn(A = B) = C", "T: Fn(A = B) "),
        ("T: [A = B] = C", "T: [A = B] "),
        ("const N: usize = { 1 + 2 }", "const N: usize "),
        ("T: Trait<{ A = B }> = C", "T: Trait<{ A = B }> "),
        ("T: Clone", "T: Clone"),
    ] {
        assert_eq!(strip_rust_generic_default(input), expected);
    }
}
