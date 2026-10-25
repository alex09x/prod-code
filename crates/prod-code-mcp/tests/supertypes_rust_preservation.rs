/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

// The compiled fixture exercises restricted visibility and independent lifetime binders.
#![allow(clippy::needless_pub_self, clippy::needless_lifetimes)]

include!("supertypes_rust_preservation/fixture.rs");

use prod_code_mcp::supertypes::{Kind, Supertypes, supertypes};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};
use std::path::Path;

const SOURCE: &str = include_str!("supertypes_rust_preservation/fixture.rs");

#[path = "supertypes_rust_preservation/nested_const.rs"]
mod nested_const_tests;

fn position(source: &str, needle: &str, name: &str) -> (u32, u32) {
    let offset = source.find(needle).unwrap() + needle.find(name).unwrap();
    let before = &source[..offset];
    (
        before.bytes().filter(|b| *b == b'\n').count() as u32,
        before.rsplit('\n').next().unwrap().encode_utf16().count() as u32,
    )
}

fn location(file: &Path, (line, col): (u32, u32)) -> Value {
    json!({ "uri": url::Url::from_file_path(file).unwrap().to_string(),
        "range": { "start": { "line": line, "character": col },
                   "end": { "line": line, "character": col + 1 } } })
}

async fn hierarchy(needle: &str, name: &str, impls: &[(&str, &str)], depth: usize) -> Supertypes {
    let ws = Workspace::new(&[("src/lib.rs", SOURCE)]);
    let file = ws.path("src/lib.rs");
    let pos = position(SOURCE, needle, name);
    let definition = location(&file, pos);
    let implementations: Vec<_> = impls
        .iter()
        .map(|(needle, name)| location(&file, position(SOURCE, needle, name)))
        .collect();
    println!(
        "scripted definition (zero-based UTF-16): {pos:?}; implementations: {:?}",
        impls
            .iter()
            .map(|(needle, name)| position(SOURCE, needle, name))
            .collect::<Vec<_>>()
    );
    let shape = location(&file, position(SOURCE, "pub trait Shape", "Shape"));
    // Locations are scripted, not live rust-analyzer results. Source parsing, classification,
    // recursive definition lookup, and rendering run through the public hierarchy API.
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/definition" => {
            let asked = (
                params["position"]["line"].as_u64().unwrap() as u32,
                params["position"]["character"].as_u64().unwrap() as u32,
            );
            if asked == pos {
                definition.clone()
            } else {
                let line = SOURCE.lines().nth(asked.0 as usize).unwrap_or("");
                if line
                    .get(asked.1 as usize..)
                    .is_some_and(|s| s.starts_with("Shape"))
                {
                    shape.clone()
                } else {
                    Value::Null
                }
            }
        }
        "textDocument/implementation" => json!(implementations),
        _ => Value::Null,
    })
    .await;
    let result = supertypes(
        gateway.addr(),
        &ws.root(),
        &file,
        pos.0 + 1,
        pos.1 + 1,
        depth,
    )
    .await
    .unwrap();
    println!(
        "{name}: kind={:?}\n{}",
        result.kind,
        result.render(&ws.root())
    );
    result
}

#[tokio::test]
async fn restricted_in_visibility_is_a_trait_with_send() {
    let result = hierarchy("pub(in crate::api) trait Store", "Store", &[], 1).await;
    assert_eq!(result.kind, Kind::Trait);
    assert_eq!(
        result
            .list
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["Send"]
    );
}

#[tokio::test]
async fn restricted_self_visibility_is_a_trait_with_send() {
    let result = hierarchy("pub(self) trait Local", "Local", &[], 1).await;
    assert_eq!(result.kind, Kind::Trait);
    assert_eq!(
        result
            .list
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["Send"]
    );
}

#[tokio::test]
async fn long_generic_impl_retains_its_trait() {
    let result = hierarchy(
        "pub struct Target",
        "Target",
        &[("> Render for Target", "Target")],
        1,
    )
    .await;
    assert_eq!(result.kind, Kind::Type);
    assert_eq!(result.list.len(), 1);
    assert_eq!(result.list[0].name, "Render");
}

#[tokio::test]
async fn raw_identifier_trait_keeps_the_declaration_impl_keyword() {
    let result = hierarchy(
        "pub struct RawTarget",
        "RawTarget",
        &[("impl r#impl for RawTarget", "RawTarget")],
        1,
    )
    .await;
    assert_eq!(result.kind, Kind::Type);
    assert_eq!(result.list.len(), 1);
    assert_eq!(result.list[0].name, "r#impl");
}

#[tokio::test]
async fn multiline_generic_line_comment_retains_its_trait() {
    let result = hierarchy(
        "pub struct Commented",
        "Commented",
        &[("    Commented<T>", "Commented")],
        1,
    )
    .await;
    assert_eq!(result.kind, Kind::Type);
    assert_eq!(
        result
            .list
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["Render"]
    );
}

#[tokio::test]
async fn implementation_character_arguments_keep_values_and_distinct_specializations() {
    let result = hierarchy(
        "pub struct Tagged",
        "Tagged",
        &[
            ("impl Tag<'x'> for Tagged", "Tagged"),
            ("impl Tag<'y'> for Tagged", "Tagged"),
            ("impl Tag<'>'> for Tagged", "Tagged"),
            ("impl Tag<'{'> for Tagged", "Tagged"),
            ("impl Tag<'🦀'> for Tagged", "Tagged"),
            ("impl Tag /* for Fake { */ <'z'> for Tagged", "Tagged"),
        ],
        1,
    )
    .await;
    let names: Vec<_> = result.list.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names.len(), 6);
    for expected in ["Tag<'x'>", "Tag<'y'>", "Tag<'>'>", "Tag<'{'>", "Tag<'🦀'>"] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    // Comments remain in the displayed source spelling, but their fake `for` and brace
    // must never become structural separators or an invented implementation.
    assert!(names.contains(&"Tag /* for Fake { */ <'z'>"));
}

#[tokio::test]
async fn higher_ranked_self_preserves_binder_and_follows_shape() {
    let result = hierarchy("pub trait Higher", "Higher", &[], 2).await;
    assert_eq!(result.kind, Kind::Trait);
    assert_eq!(result.list.len(), 1);
    assert_eq!(result.list[0].name, "for<'a> Shape<'a>");
    assert_eq!(result.list[0].children.len(), 1);
    assert_eq!(result.list[0].children[0].name, "Send");
}

#[tokio::test]
async fn ordinary_visibility_colon_and_where_self_are_preserved() {
    for name in ["Crate", "Parent", "Shared"] {
        let result = hierarchy(&format!("trait {name}"), name, &[], 1).await;
        assert_eq!(result.kind, Kind::Trait);
        assert_eq!(result.list[0].name, "Send");
    }
    let result = hierarchy("pub trait Ordinary", "Ordinary", &[], 1).await;
    assert_eq!(
        result
            .list
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["Send", "Sync"]
    );
    assert!(
        hierarchy("pub trait Generic", "Generic", &[], 1)
            .await
            .list
            .is_empty()
    );
}

#[tokio::test]
async fn higher_ranked_multiple_bounds_keep_their_scope() {
    let result = hierarchy("pub trait Mixed", "Mixed", &[], 1).await;
    assert_eq!(
        result
            .list
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["Sync", "for<'a> Shape<'a>", "for<'a> Send", "'static"]
    );
}

#[tokio::test]
async fn impl_boundaries_exclude_bodies_unrelated_items_comments_and_strings() {
    // Deliberately adversarial analyzer locations: none of these is an impl header.
    // The ordinary inherent impl is also a real positive location with no implemented trait.
    for entry in [
        ("pub fn body() -> Plain", "Plain"),
        ("pub struct Unrelated", "Unrelated"),
        ("pub struct Neighbor", "Neighbor"),
        ("pub fn body() -> Sentinel", "Sentinel"),
        ("impl Fake for Unrelated", "Unrelated"),
        ("impl AlsoFake for Unrelated", "Unrelated"),
        ("impl Invented for Unrelated", "Unrelated"),
        ("impl Quoted for Unrelated", "Unrelated"),
        ("pub fn use_unrelated() -> Unrelated", "Unrelated"),
        ("impl After {}", "After"),
    ] {
        let result = hierarchy("pub struct Unrelated", "Unrelated", &[entry], 1).await;
        assert!(
            result.list.is_empty(),
            "attached {entry:?}: {:?}",
            result.list
        );
    }
}

#[tokio::test]
async fn external_gateway_source_and_missing_source_remain_supported() {
    let ws = Workspace::new(&[("src/lib.rs", "pub struct Query;\n")]);
    let external = tempfile::tempdir().unwrap();
    let file = external.path().join("dependency.rs");
    assert!(
        !file.exists(),
        "the external source must only exist at the gateway"
    );
    for (needle, name, present, expected) in [
        (
            "pub(in crate::api) trait Store",
            "Store",
            true,
            vec!["Send"],
        ),
        (
            "pub trait Higher",
            "Higher",
            true,
            vec!["for<'a> Shape<'a>"],
        ),
        ("pub trait Higher", "Higher", false, vec![]),
    ] {
        let definition = location(&file, position(SOURCE, needle, name));
        let remote_path = file.to_string_lossy().to_string();
        let gateway = ScriptedGateway::start(move |method, params| match method {
            "textDocument/definition" => definition.clone(),
            "prod-code/readFile" if present && params["path"] == remote_path => json!(SOURCE),
            _ => Value::Null,
        })
        .await;
        let result = supertypes(gateway.addr(), &ws.root(), &ws.path("src/lib.rs"), 1, 12, 1)
            .await
            .unwrap();
        println!(
            "external {name} present={present}: kind={:?}\n{}",
            result.kind,
            result.render(&ws.root())
        );
        assert_eq!(result.kind, if present { Kind::Trait } else { Kind::Other });
        assert_eq!(
            result
                .list
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(!file.exists());
    }
}

#[test]
fn higher_really_implies_shape_for_independent_lifetimes() {
    let first = String::from("one");
    let result = {
        let second = String::from("second");
        implied(&Unit, &first, &second)
    };
    assert_eq!(result, 9);
}
