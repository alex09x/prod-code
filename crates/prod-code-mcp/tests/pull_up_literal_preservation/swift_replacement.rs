/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::{
    pull_push::{parse_classes_in_text, strip_override_modifiers},
    replace_inheritance::languages::transform_swift,
};
use std::{io::Write, path::Path, process::Command};

fn replace(source: &str) -> String {
    let classes = parse_classes_in_text(source, "swift", Path::new("pets.swift"));
    let sub = classes.iter().find(|c| c.name == "Dog").unwrap();
    let base = classes.iter().find(|c| c.name == "Pet").unwrap();
    for member in &sub.members {
        assert_eq!(
            &source[member.start_offset..member.end_offset],
            member.full_text
        );
    }
    let after = transform_swift(source, sub, "Pet", "pet", &[], Some(base)).unwrap();
    let hashes = Command::new("python3")
        .args([
            "-c",
            "import hashlib,json,sys; print(json.dumps([hashlib.sha256(s.encode()).hexdigest() for s in sys.argv[1:]]))",
            source,
            &after,
        ])
        .output()
        .unwrap();
    assert!(hashes.status.success());
    let hashes: Vec<String> = serde_json::from_slice(&hashes.stdout).unwrap();
    let evidence = serde_json::json!({
        "before_source": source, "before_sha256": hashes[0],
        "after_source": after, "after_sha256": hashes[1],
        "runtime": null, "limitation": "Swift source assertions only; no Swift toolchain on this node"
    });
    writeln!(std::io::stderr(), "SWIFT_SOURCE_EVIDENCE {evidence}").unwrap();
    after
}

// These are declaration/source regressions, not Swift compiler evidence. The
// acceptance node has no Swift toolchain; Python/JS/C++ runtime proofs are separate.
#[test]
fn replacement_strips_explicit_swift_initializer_override() {
    for prefix in ["override", "@available(*, deprecated) override"] {
        let source = format!(
            "class Pet {{\n    init() {{}}\n}}\nclass Dog: Pet {{\n\
                 {prefix} init() {{\n        super.init()\n    }}\n\
                 func speak() -> String {{ return \"override final init()\" }}\n}}\n"
        );
        let after = replace(&source);
        assert!(!after.contains("class Dog: Pet"), "{after}");
        assert!(!after.contains("override init()"), "{after}");
        assert!(after.contains("self.pet = Pet()"), "{after}");
        assert!(after.contains("return \"override final init()\""));
        if prefix.starts_with('@') {
            assert!(after.contains("@available(*, deprecated) init()"));
        }
        let classes = parse_classes_in_text(&after, "swift", Path::new("pets.swift"));
        let sub = classes.iter().find(|c| c.name == "Dog").unwrap();
        let init = sub.members.iter().find(|m| m.name == "init").unwrap();
        assert!(init.full_text.contains("self.pet = Pet()"));
        assert!(!init.full_text.contains("func speak"));
    }
}

#[test]
fn replacement_strips_override_after_swift_attributes() {
    let attribute = "@available(*, deprecated, message: \"override ) final\")";
    let source = format!(
        "class Pet {{\n    func speak() -> String {{ return \"base\" }}\n}}\n\
         class Dog: Pet {{\n    {attribute} @MainActor public override /* override */ func speak() -> String {{\n\
                 return \"override final\"\n    }}\n}}\n"
    );
    let after = replace(&source);
    assert!(
        after.contains(&format!(
            "{attribute} @MainActor public /* override */ func speak"
        )),
        "{after}"
    );
    assert!(after.contains("return \"override final\""));
    assert!(!after.contains("class Dog: Pet"));
}

#[test]
fn swift_attribute_arguments_are_preserved_byte_for_byte() {
    for attributes in [
        "@available(*, deprecated, message: \"override ) final\")",
        "@Example(nested(value: \"(override)\")) @MainActor",
        "@available(*, deprecated)\n    @MainActor",
    ] {
        let declaration = format!(
            "    {attributes} public override func speak() {{ print(\"override final\") }}"
        );
        assert_eq!(
            strip_override_modifiers(&declaration, "swift"),
            format!("    {attributes} public func speak() {{ print(\"override final\") }}")
        );
    }
}
