/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::*;
use crate::move_item::ModulePath;

#[test]
fn a_declaration_is_found_with_its_attributes_and_nothing_nested() {
    let text = "pub mod a;\n/// The b module.\n#[cfg(feature = \"x\")]\npub(crate) mod b;\nfn f() {}\nmod inner {\n    mod b;\n}\n";
    let (start, end, line) = declaration(text, "b").unwrap();
    assert_eq!(
        &text[start..end],
        "/// The b module.\n#[cfg(feature = \"x\")]\npub(crate) mod b;\n"
    );
    assert_eq!(line, "pub(crate) mod b;");
    assert!(declaration(text, "c").is_none());
    assert!(declaration("mod bb;\nmod ab;\n", "b").is_none());
    assert!(declaration("    mod b;\n", "b").is_none());
    assert!(declaration("mod b {}\n", "b").is_none());
}

#[test]
fn a_block_is_declared_where_a_bare_one_would_go() {
    let out = declare_block(
        "pub mod x;\n\nfn f() {}\n",
        "b",
        "#[cfg(test)]\npub mod b;\n",
    );
    assert_eq!(out, "pub mod x;\n#[cfg(test)]\npub mod b;\n\nfn f() {}\n");
}

#[test]
fn super_becomes_the_old_parent_but_not_inside_a_longer_name() {
    let (out, n) = resolve_super(
        "use super::helper;\nfn f() { super::x(); my_super::y(); super::super::z(); }\n",
        "crate::a",
    );
    assert_eq!(n, 2);
    assert_eq!(
        out,
        "use crate::a::helper;\nfn f() { crate::a::x(); my_super::y(); super::super::z(); }\n"
    );
}

#[test]
fn a_reference_is_bare_grouped_or_qualified() {
    let t = "use crate::a::b;\nuse crate::a::{b, c};\nfn f() { b::g(); super::a::b::g(); }\n";
    let at = |needle: &str, nth: usize| t.match_indices(needle).nth(nth).unwrap().0;
    assert_eq!(
        spelling(t, at("b;", 0)),
        Spelling::Qualified {
            path_start: at("crate", 0)
        }
    );
    assert_eq!(spelling(t, at("b, c", 0)), Spelling::Grouped);
    assert_eq!(spelling(t, at("b::g", 0)), Spelling::Bare);
    assert_eq!(
        spelling(t, at("b::g", 1)),
        Spelling::Qualified {
            path_start: at("super", 0)
        }
    );
}

#[test]
fn the_declaring_file_and_the_files_below_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(src.join("a/b/deep")).unwrap();
    std::fs::write(src.join("lib.rs"), "pub mod a;\n").unwrap();
    std::fs::write(src.join("a.rs"), "pub mod b;\n").unwrap();
    std::fs::write(src.join("a/b/mod.rs"), "pub mod deep;\n").unwrap();
    std::fs::write(src.join("a/b/deep/x.rs"), "").unwrap();
    std::fs::write(src.join("a/b/deep.rs"), "pub mod x;\n").unwrap();
    assert_eq!(
        declaring_file(&src.join("a/b/mod.rs")),
        Some(src.join("a.rs"))
    );
    assert_eq!(declaring_file(&src.join("a.rs")), Some(src.join("lib.rs")));
    let found = files_under(&src.join("a/b"));
    assert_eq!(found.len(), 3, "{found:?}");
    let parent = parent_of(&ModulePath {
        krate: "k".into(),
        segments: vec!["a".into(), "b".into()],
    });
    assert_eq!(parent.segments, vec!["a".to_string()]);
}
