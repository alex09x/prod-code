/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::{
    derives_above, header_from, impl_header_start, impl_trait, is_trait_decl, skip_generics,
    supertraits, word_at,
};
use super::types::{Kind, Supertype, Supertypes};
use super::validate::locations;
use std::path::{Path, PathBuf};

#[test]
fn an_impl_header_names_its_trait_and_an_inherent_one_none() {
    assert_eq!(
        impl_trait("impl Default for SearchIndexes ").as_deref(),
        Some("Default")
    );
    assert_eq!(
        impl_trait("impl<T: Clone> Into<Vec<T>> for Wrapper<T> where T: Send").as_deref(),
        Some("Into<Vec<T>>")
    );
    assert_eq!(
        impl_trait("unsafe impl Send for Handle").as_deref(),
        Some("Send")
    );
    assert_eq!(impl_trait("impl SearchIndexes "), None);
    assert_eq!(impl_trait("impl<T> Wrapper<T> where T: Fn() -> u8"), None);
    assert_eq!(impl_trait("fn main() {}"), None);
}

#[test]
fn a_trait_header_names_its_supertraits() {
    assert_eq!(supertraits("pub trait Embed: Send "), vec!["Send"]);
    assert_eq!(
        supertraits(
            "trait Store<K: Ord>: Clone + Iterator<Item = (K, u8)> + 'static where K: Send"
        ),
        vec!["Clone", "Iterator<Item = (K, u8)>", "'static"]
    );
    assert!(supertraits("pub trait Plain ").is_empty());
    assert_eq!(
        supertraits("pub trait Circle where Self: Shape "),
        vec!["Shape"]
    );
    assert_eq!(
        supertraits("trait Both: Clone where Self: Shape + Clone, T: Copy, Self: Debug"),
        vec!["Clone", "Shape", "Debug"]
    );
    assert!(supertraits("trait Other where T: Copy ").is_empty());
    assert_eq!(supertraits("trait Near: Somewhere "), vec!["Somewhere"]);
    assert!(supertraits("struct Nope ").is_empty());
    assert!(is_trait_decl("pub(crate) unsafe trait Raw {"));
    assert!(!is_trait_decl("pub struct Traits;"));
}

#[test]
fn a_header_is_joined_up_to_its_brace_and_a_word_is_read_at_a_column() {
    let lines = [
        "impl<T>",
        "    Default for Holder<T>",
        "where T: Default {",
        "}",
    ];
    assert_eq!(
        header_from(&lines, 0),
        "impl<T>     Default for Holder<T> where T: Default "
    );
    assert_eq!(
        word_at("#[derive(Clone, Debug)]", 10).as_deref(),
        Some("Clone")
    );
    assert_eq!(
        word_at("#[derive(Clone, Debug)]", 18).as_deref(),
        Some("Debug")
    );
    assert_eq!(word_at("#[derive(Clone)]", 1), None);
    assert_eq!(word_at("x", 0), None);
    assert_eq!(skip_generics("<A<B>> rest"), " rest");
    assert_eq!(skip_generics("<unclosed"), "");
}

#[test]
fn derives_are_read_from_the_attributes_above_the_declaration() {
    let lines = [
        "}",
        "",
        "/// A report.",
        "#[derive(Debug, Clone,",
        "    serde::Serialize)]",
        "#[serde(tag = \"event\")]",
        "pub enum RunEvent {",
    ];
    assert_eq!(
        derives_above(&lines, 6),
        vec![
            ("Debug".to_string(), 4, 10),
            ("Clone".to_string(), 4, 17),
            ("serde::Serialize".to_string(), 5, 5),
        ]
    );
    assert!(derives_above(&["pub struct Bare;"], 0).is_empty());
    let impls = [
        "impl<T>",
        "    Default",
        "    for Holder<T> {",
        "}",
        "pub enum E {",
    ];
    assert_eq!(impl_header_start(&impls, 2), Some(0));
    assert_eq!(impl_header_start(&impls, 4), None);
}

#[test]
fn the_report_says_what_there_is_and_what_there_is_not() {
    let root = Path::new("/w");
    let st = Supertypes {
        of: "Cache".into(),
        kind: Kind::Type,
        list: vec![
            Supertype::new("Clone", true, Some((PathBuf::from("/w/src/lib.rs"), 1, 10))),
            Supertype::new(
                "Default",
                false,
                Some((PathBuf::from("/w/src/lib.rs"), 5, 18)),
            ),
        ],
        depth: 1,
        unsupported: None,
    };
    assert_eq!(
        st.render(root),
        "`Cache` implements 2 trait(s):\n  • Clone  (derived)  src/lib.rs:1:10\n  • Default  src/lib.rs:5:18"
    );
    let none = Supertypes {
        of: "Embed".into(),
        kind: Kind::Trait,
        list: vec![],
        depth: 1,
        unsupported: None,
    };
    assert_eq!(none.render(root), "`Embed` requires no supertrait.");
    let other = Supertypes {
        kind: Kind::Other,
        ..none.clone()
    };
    assert_eq!(other.render(root), "`Embed` has no supertype.");
    let no_server = Supertypes {
        unsupported: Some("No type hierarchy".into()),
        ..none
    };
    assert_eq!(no_server.render(root), "No type hierarchy");
    let link = serde_json::json!({ "targetUri": "file:///w/a.rs",
        "targetSelectionRange": { "start": { "line": 2, "character": 4 } } });
    assert_eq!(locations(&link), vec![(PathBuf::from("/w/a.rs"), 2, 4)]);
    assert!(locations(&serde_json::Value::Null).is_empty());
}

#[test]
fn derives_above_never_panics_on_empty_lines_or_out_of_bounds_decl() {
    assert!(derives_above(&[], 0).is_empty());
    assert!(derives_above(&[], 232).is_empty());
    let lines = ["struct Foo;"];
    assert!(derives_above(&lines, 50).is_empty());
}

#[test]
fn multi_level_supertypes_render_as_nested_tree() {
    let root = Path::new("/w");
    let mut parent = Supertype::new(
        "Store",
        false,
        Some((PathBuf::from("/w/src/lib.rs"), 10, 5)),
    );
    let child = Supertype::new("Send", false, None);
    let mut repeated_child = Supertype::new(
        "Store",
        false,
        Some((PathBuf::from("/w/src/lib.rs"), 10, 5)),
    );
    repeated_child.repeated = true;
    parent.children.push(child);
    parent.children.push(repeated_child);

    let st = Supertypes {
        of: "Cache".into(),
        kind: Kind::Type,
        list: vec![parent],
        depth: 2,
        unsupported: None,
    };
    assert_eq!(
        st.render(root),
        "`Cache` implements 1 trait(s), 3 in all to depth 2:\n  • Store  src/lib.rs:10:5\n    • Send\n    • Store  src/lib.rs:10:5  (shown above)"
    );
}
