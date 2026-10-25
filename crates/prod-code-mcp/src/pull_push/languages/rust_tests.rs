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
fn coverage_w2_trait_members_preserve_source_spans_and_kinds() {
    let text = "pub trait Store<T>: Read + Write + {\n    // contract\n    type Item: Clone;\n    const LIMIT: usize = 8;\n    fn read(&self) -> T {\n        loop { break; }\n    }\n    async fn flush(&self);\n}\n";
    let path = Path::new("store.rs");
    let traits = parse_rust_traits(text, path);
    assert_eq!(traits.len(), 1);
    let class = &traits[0];
    assert_eq!(class.name, "Store");
    assert_eq!(class.language, "rust");
    assert_eq!(class.file_path, path);
    assert_eq!(class.super_names, ["Read", "Write"]);
    assert_eq!(class.indent, "    ");
    assert_eq!(&text[class.decl_start..class.decl_end], text.trim_end());
    assert_eq!(
        &text[class.body_start..class.body_end],
        text.split_once('{').unwrap().1.rsplit_once('}').unwrap().0
    );
    let expected = [
        ("Item", MemberKind::AssociatedType, "    type Item: Clone;"),
        ("LIMIT", MemberKind::Constant, "    const LIMIT: usize = 8;"),
        (
            "read",
            MemberKind::Method,
            "    fn read(&self) -> T {\n        loop { break; }\n    }",
        ),
        ("flush", MemberKind::Method, "    async fn flush(&self);"),
    ];
    assert_eq!(class.members.len(), expected.len());
    for (member, (name, kind, source)) in class.members.iter().zip(expected) {
        assert_eq!(member.name, name);
        assert_eq!(member.kind, kind);
        assert_eq!(member.full_text, source);
        assert_eq!(&text[member.start_offset..member.end_offset], source);
        assert!(!member.is_override);
    }
}

#[test]
fn coverage_w2_incomplete_traits_and_empty_members_are_skipped() {
    for text in ["trait Missing", "trait Unclosed {", "portrait Fake {}", ""] {
        assert_eq!(
            parse_rust_traits(text, Path::new("input.rs")).len(),
            0,
            "{text}"
        );
    }
    let text = "trait Empty<T> {\n    type ;\n    const : usize;\n    fn ();\n    unrelated\n}\ntrait Next {}";
    let traits = parse_rust_traits(text, Path::new("input.rs"));
    assert_eq!(
        traits.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["Empty", "Next"]
    );
    assert!(
        traits
            .iter()
            .all(|c| c.members.is_empty() && c.super_names.is_empty())
    );
    assert!(parse_rust_trait_members("fn broken() {", 0).is_empty());
}
