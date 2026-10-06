/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::drop::{
    declared_async, drop_order, name_in, no_drop_glue, rust_code, rust_edition, spelled,
};
use super::super::types::Spelled;

/// Only what the spelling shows cannot run a destructor is taken for that, and a primitive's
/// name only for a type the analyzer has to resolve (#441).
#[test]
fn a_type_may_drop_unless_its_spelling_shows_it_cannot() {
    for inert in [
        "&str",
        "&mut Guard",
        "&'a [u8]",
        "*const Guard",
        "fn(u32) -> u32",
        "()",
        "!",
        "(&bool, *mut u8)",
    ] {
        assert_eq!(spelled(inert), Spelled::Inert, "{inert}");
    }
    for primitive in ["u32", "bool", "(u8, &str)", "[[u8; 4]; 2]", "[bool; 2]"] {
        assert_eq!(spelled(primitive), Spelled::Primitive, "{primitive}");
    }
    for owned in [
        "Guard",
        "String",
        "T",
        "impl Drop",
        "(u8, Guard)",
        "[Guard; 2]",
        "core::primitive::bool",
    ] {
        assert_eq!(spelled(owned), Spelled::MayDrop, "{owned}");
    }
    assert_eq!(name_in("mut m: u32", "m"), Some(4));
    assert_eq!(name_in("t: (u8, u8)", "t"), Some(0));
    assert_eq!(name_in("n: u32", "m"), None);
}

/// The hovers are rust-analyzer's on parameters, asked with `prod-code hover` on a build
/// node: `bool` there was the program's own struct with `Drop`, declared or imported.
#[test]
fn only_a_resolved_type_without_drop_glue_is_vouched_for() {
    for (hover, name) in [
        ("```rust\nn: u32\n```\n\n---\n\nno Drop", "n"),
        ("\n```rust\nmut m: u32\n```\n\n---\n\nno Drop", "m"),
        ("```rust\nt: (bool, &str)\n```\n\n---\n\nno Drop", "t"),
        ("```rust\nr: [u8; 2]\n```\n\n---\n\nno Drop", "r"),
    ] {
        assert_eq!(no_drop_glue(hover, name), Ok(()), "{hover}");
    }
    for (hover, name, why) in [
        (
            "```rust\na: bool\n```\n\n---\n\nneeds Drop",
            "a",
            "the analyzer reports `needs Drop` for it",
        ),
        (
            "```rust\nt: (bool, u32)\n```\n\n---\n\nneeds Drop",
            "t",
            "the analyzer reports `needs Drop` for it",
        ),
        (
            "```rust\nr: [bool; 2]\n```\n\n---\n\nneeds Drop",
            "r",
            "the analyzer reports `needs Drop` for it",
        ),
        (
            "```rust\na: bool\n```\n\n---\n\ntype param may need Drop",
            "a",
            "the analyzer reports `type param may need Drop` for it",
        ),
        (
            "```rust\nu: {unknown}\n```\n\n---\n\nno Drop",
            "u",
            "the analyzer does not resolve its type (`{unknown}`)",
        ),
        (
            "```rust\nu32\n```\n\n---\n\nThe 32-bit unsigned integer type.",
            "n",
            "the analyzer's hover is about `u32`, not it",
        ),
        (
            "```rust\nn: u32\n```",
            "n",
            "the analyzer's hover does not say whether its type has drop glue",
        ),
        (
            "no Drop",
            "n",
            "the analyzer's hover does not show its type",
        ),
    ] {
        assert_eq!(no_drop_glue(hover, name), Err(why.to_string()), "{hover}");
    }
}

/// `f(a, x, b, y)` with `a`, `b` bundled: `x` is dropped before the struct rather than
/// between its fields, and an `async` future dropped unpolled drops first to last.
#[test]
fn the_drop_order_is_compared_before_and_after_bundling() {
    let owned = [true, true, true, true];
    assert_eq!(
        drop_order(4, &[1, 2], &[2, 1], &owned, false),
        (vec![3, 2, 1, 0], vec![3, 2, 1, 0])
    );
    assert_eq!(
        drop_order(3, &[0, 2], &[2, 0], &owned, false),
        (vec![2, 1, 0], vec![1, 2, 0])
    );
    assert_eq!(
        drop_order(3, &[0, 2], &[2, 0], &[true, false, true], false),
        (vec![2, 0], vec![2, 0])
    );
    assert_eq!(
        drop_order(2, &[0, 1], &[1, 0], &owned, true),
        (vec![0, 1], vec![1, 0])
    );
}

#[test]
fn the_edition_is_the_crates_or_the_workspaces_it_inherits() {
    let dir = tempfile::tempdir().expect("a directory");
    let at = |rel: &str, text: &str| {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, text).expect("write");
        path
    };
    at(
        "Cargo.toml",
        "[workspace]\nmembers = [\"a\", \"b\"]\n\n[workspace.package]\nedition = \"2024\"\n",
    );
    at(
        "a/Cargo.toml",
        "[package]\nname = \"a\"\nedition.workspace = true\n",
    );
    at(
        "b/Cargo.toml",
        "[package]\nname = \"b\"\nedition = \"2018\"\n",
    );
    at(
        "c/Cargo.toml",
        "[package]\nname = \"c\"\n\n[dependencies]\n",
    );
    // Spellings TOML allows: an inline table, quoted keys, literal strings, comments.
    at(
        "d/Cargo.toml",
        "package = { name = 'd', edition = '2021' } # [package] edition = \"2015\"\n",
    );
    at(
        "e/Cargo.toml",
        "[package]\n\"name\" = \"e\"\n'edition' = \"2018\" # old\n",
    );
    // An explicit workspace outside the crate's directories wins over the ancestor's 2024.
    at(
        "f/Cargo.toml",
        "[package]\nname = \"f\"\nworkspace = \"../g/ws\"\nedition = { workspace = true }\n",
    );
    at(
        "g/ws/Cargo.toml",
        "[workspace.package]\nedition = \"2021\"\n\n[workspace]\nmembers = [\"../../f\"]\n",
    );
    // Text that only looks like an edition: in a string, another table, another key.
    at(
        "h/Cargo.toml",
        "[package]\nname = \"h\"\ndescription = '''\n[package]\nedition = \"2024\"\n'''\n\
         editions = \"2021\"\n\n[package.metadata.x]\nedition = \"2024\"\n",
    );
    assert_eq!(rust_edition(&at("a/src/lib.rs", "")), Ok(2024));
    assert_eq!(rust_edition(&at("b/src/lib.rs", "")), Ok(2018));
    assert_eq!(rust_edition(&at("c/src/lib.rs", "")), Ok(2015));
    assert_eq!(rust_edition(&at("d/src/lib.rs", "")), Ok(2021));
    assert_eq!(rust_edition(&at("e/src/bin/x.rs", "")), Ok(2018));
    assert_eq!(rust_edition(&at("f/src/lib.rs", "")), Ok(2021));
    assert_eq!(rust_edition(&at("h/src/lib.rs", "")), Ok(2015));

    // What Cargo would reject, or what does not say, is not an edition.
    let unknown = [
        (
            "i",
            "[package]\nname = \"i\"\nedition = \"2027\"\n",
            "not one this check knows",
        ),
        (
            "j",
            "[package]\nname = \"j\"\nedition = 2021\n",
            "not one this check knows",
        ),
        ("k", "[package\nedition = \"2021\"\n", "is not valid TOML"),
        (
            "l",
            "[package]\nedition = \"2021\"\nedition = \"2021\"\n",
            "is not valid TOML",
        ),
        ("m", "[workspace]\nmembers = []\n", "has no [package]"),
        (
            "n",
            "[package]\nname = \"n\"\nedition = { workspace = false }\n",
            "neither a string nor",
        ),
        (
            "o",
            "[package]\nname = \"o\"\nworkspace = \"../c\"\nedition.workspace = true\n",
            "which has no `workspace.package.edition`",
        ),
        (
            "p",
            "[package]\nname = \"p\"\nworkspace = \"../missing\"\nedition.workspace = true\n",
            "there is no",
        ),
    ];
    for (krate, manifest, why) in unknown {
        at(&format!("{krate}/Cargo.toml"), manifest);
        let err = rust_edition(&at(&format!("{krate}/src/lib.rs"), ""))
            .expect_err(&format!("{krate}: {manifest}"));
        assert!(err.contains(why), "{krate}: {err}");
    }

    // A member whose workspace has no `[workspace.package]` edition to give.
    let bare = tempfile::tempdir().expect("a directory");
    std::fs::write(bare.path().join("Cargo.toml"), "[workspace]\n").expect("write");
    std::fs::create_dir_all(bare.path().join("q/src")).expect("mkdir");
    std::fs::write(
        bare.path().join("q/Cargo.toml"),
        "[package]\nname = \"q\"\nedition.workspace = true\n",
    )
    .expect("write");
    let err = rust_edition(&bare.path().join("q/src/lib.rs")).expect_err("nothing to inherit");
    assert!(
        err.contains("which has no `workspace.package.edition`"),
        "{err}"
    );
}

/// `async` is read from the qualifiers before `fn`, over line breaks and comments; in a
/// comment, a string or another item it does not count.
#[test]
fn a_function_is_async_by_its_qualifiers_not_by_its_comments() {
    let at = |text: &str| declared_async(text, text.find("f(").expect("a name"));
    for text in [
        "async fn f(a: A) {}",
        "pub(crate)\nasync\nfn f(a: A) {}",
        "async /* the future\nowns both */ fn f(a: A) {}",
        "pub async unsafe extern \"C\" fn f(a: A) {}",
        "const async // why\nunsafe fn f(a: A) {}",
        "impl T {\n    async\n    fn f(a: A) {}\n}",
        "'a: loop {}\nlet c = '\"';\npub async fn f(a: A) {}",
        // A lifetime named in another script is not a character literal running on to the
        // next quote, and takes neither the comment nor the `async` with it.
        "fn g<'ä>(x: &'ä u8, y: &'ä u8) {}\npub async /* it's */ fn f(a: A) {}",
        "impl<'ä> S<'ä> {\n    fn g(&'ä self) -> char { 'é' }\n}\nasync fn f(a: A) {}",
    ] {
        assert_eq!(at(text), Some(true), "{text}");
    }
    for text in [
        "fn f(a: A) {}",
        "/* async */ fn f(a: A) {}",
        "pub(crate) // async once\n/* not async */ fn f(a: A) {}",
        "// async\nfn f(a: A) {}",
        "/* async /* nested */ async */ fn f(a: A) {}",
        "#[doc = \"async\"]\nfn f(a: A) {}",
        "const S: &str = r#\"\" async \"#;\nfn f(a: A) {}",
        "async fn g() {}\nfn f(a: A) {}",
        "unsafe extern \"C\" fn f(a: A) {}",
        "fn g<'ä>(x: &'ä u8, y: &'ä u8) {}\n/* it's not async */ fn f(a: A) {}",
    ] {
        assert_eq!(at(text), Some(false), "{text}");
    }
    assert_eq!(at("x f(a: A)"), None);
}

/// A lifetime or a label is code in any script; a character literal's inside is blanked.
#[test]
fn a_lifetime_in_any_script_is_not_a_character_literal() {
    let code = "fn g<'ä>(s: &'ä str) -> [char; 3] { 'l: loop { break 'l ['é', '\\'', '\"'] } }";
    let blanked = String::from_utf8(rust_code(code)).expect("still UTF-8");
    assert_eq!(
        blanked,
        "fn g<'ä>(s: &'ä str) -> [char; 3] { 'l: loop { break 'l ['  ', '  ', ' '] } }"
    );
}
