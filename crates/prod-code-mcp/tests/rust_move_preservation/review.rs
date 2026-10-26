/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    cases::{item, module_refs},
    support::*,
};
use prod_code_testkit::Workspace;
use serde_json::{Value, json};

const MANIFEST: &str = "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\n";

fn finish(id: &str, ws: &Workspace, before: Value, mut record: Value) -> Value {
    record["before_compiler"] = before;
    record["after_compiler"] = cargo(&ws.root());
    save_named(id, record.clone());
    record
}

#[tokio::test]
async fn review_grouped_self_imports_remain_prefix_leaves() {
    let source = "pub fn f() {\n use std::{fmt::{self, Debug}, io::{self as input, Read}};\n let _: Option<&dyn Debug> = None; let _: Option<&dyn Read> = None;\n let _ = fmt::Error; let _ = input::ErrorKind::Other;\n}\n";
    let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("src/lib.rs", source)]);
    let before = cargo(&ws.root());
    let result = item(&ws, "src/lib.rs", "src/child.rs", "f", 1, 5).await;
    let record = finish("review-grouped-self", &ws, before, result);
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
    assert!(ws.read("src/child.rs").contains("fmt::{self, Debug}"));
    assert!(
        ws.read("src/child.rs")
            .contains("io::{self as input, Read}")
    );
}

#[tokio::test]
async fn review_child_visibility_restrictions_rebase_once() {
    let mut records = Vec::new();
    let mut passed = true;
    for (vis, expected) in [
        ("pub(super)", "pub(in super::super)"),
        ("pub(self)", "pub(super)"),
        ("pub(in super)", "pub(in super::super)"),
    ] {
        let source = format!("{vis} fn f() -> u8 {{ self::VALUE }}\nconst VALUE: u8 = 7;\n");
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", "pub mod a;\n"),
            ("src/a.rs", &source),
        ]);
        let before = cargo(&ws.root());
        let result = item(&ws, "src/a.rs", "src/a/child.rs", "f", 1, 1).await;
        let record = finish(
            &format!("review-visibility-{}", records.len()),
            &ws,
            before,
            result,
        );
        passed &= record["before_compiler"]["passed"] == true
            && record["returned"]["applied"] == true
            && record["after_compiler"]["passed"] == true
            && ws.path("src/a/child.rs").exists()
            && ws
                .read("src/a/child.rs")
                .contains(&format!("{expected} fn f()"));
        records.push(record);
    }
    assert!(passed, "{records:#?}");
}

#[tokio::test]
async fn review_whole_module_visibility_is_not_an_ordinary_path() {
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "pub mod a;\npub mod c;\n"),
        ("src/a.rs", "pub mod b;\n"),
        ("src/c.rs", ""),
        (
            "src/a/b.rs",
            "pub(super) fn f() {}\nmod nested { pub(in super::super) fn g() {} }\n",
        ),
    ]);
    let before = cargo(&ws.root());
    let result = module_refs(&ws, "src/a/b.rs", "src/c/b.rs", Vec::new()).await;
    let record = finish("review-module-visibility", &ws, before, result);
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
    assert!(ws.read("src/c/b.rs").contains("pub(super) fn f()"));
    assert!(
        ws.read("src/c/b.rs")
            .contains("pub(in super::super) fn g()")
    );
}

#[tokio::test]
async fn review_inline_path_attributes_use_lexical_destination() {
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            "#[path=\"a_impl.rs\"] pub mod a;\npub mod c {}\n",
        ),
        ("src/a_impl.rs", "pub const VALUE: u8 = 7;\n"),
    ]);
    let before = cargo(&ws.root());
    let result = module_refs(&ws, "src/a_impl.rs", "src/c/a.rs", Vec::new()).await;
    let record = finish("review-inline-path", &ws, before, result);
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
    assert!(!ws.path("src/a_impl.rs").exists());
    assert!(ws.path("src/c/a.rs").exists());
    assert!(ws.read("src/lib.rs").contains("#[path=\"a.rs\"]"));
}

#[tokio::test]
async fn review_destination_import_collision_is_removed_only_in_owner_scope() {
    let mut records = Vec::new();
    let mut passed = true;
    for (lib, c, refs) in [
        (
            "pub mod a;\npub mod c;\n",
            "use crate::a;\nmod nested { use crate::a; }\n",
            vec![("src/c.rs", 1, 12), ("src/c.rs", 2, 25)],
        ),
        (
            "pub mod a;\npub mod c { use crate::a; mod nested { use crate::a; } }\n",
            "",
            vec![("src/lib.rs", 2, 24), ("src/lib.rs", 2, 51)],
        ),
    ] {
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", lib),
            ("src/a.rs", "pub const VALUE: u8 = 7;\n"),
            ("src/c.rs", c),
        ]);
        let before = cargo(&ws.root());
        let result = module_refs(&ws, "src/a.rs", "src/c/a.rs", refs).await;
        let record = finish(
            &format!("review-import-{}", records.len()),
            &ws,
            before,
            result,
        );
        passed &= record["before_compiler"]["passed"] == true
            && record["returned"]["applied"] == true
            && record["after_compiler"]["passed"] == true;
        records.push(record);
    }
    assert!(passed, "{records:#?}");
}

#[tokio::test]
async fn review_all_cargo_target_kinds_own_shared_modules() {
    let mut manifest = MANIFEST.to_string();
    manifest.push_str("build='custom/build.rs'\n[[example]]\nname='explicit_example'\npath='custom/example.rs'\n[[bench]]\nname='explicit_bench'\npath='custom/bench.rs'\nharness=false\n");
    let consumer = "#[path=\"../src/a.rs\"] mod a;\n#[path=\"../src/c.rs\"] mod c;\nfn main() { assert_eq!(a::VALUE, 7); }\n#[test] fn runtime_consumer() { main(); }\n";
    let nested = "#[path=\"../../src/a.rs\"] mod a;\n#[path=\"../../src/c.rs\"] mod c;\nfn main() { assert_eq!(a::VALUE, 7); }\n#[test] fn runtime_consumer() { main(); }\n";
    let roots = [
        "custom/build.rs",
        "custom/example.rs",
        "custom/bench.rs",
        "examples/automatic.rs",
        "examples/nested/main.rs",
        "benches/automatic.rs",
        "benches/nested/main.rs",
    ];
    let ws = Workspace::new(&[
        ("Cargo.toml", &manifest),
        ("src/lib.rs", "pub mod a;\npub mod c;\n"),
        (
            "src/main.rs",
            "mod a;\nmod c;\nfn main() { assert_eq!(a::VALUE, 7); }\n",
        ),
        ("src/a.rs", "pub const VALUE: u8 = 7;\n"),
        ("src/c.rs", ""),
        (roots[0], consumer),
        (roots[1], consumer),
        (roots[2], consumer),
        (roots[3], consumer),
        (roots[4], nested),
        (roots[5], consumer),
        (roots[6], nested),
    ]);
    let mut refs = vec![("src/main.rs", 3, 24)];
    refs.extend(roots.iter().map(|p| (*p, 3, 24)));
    let before = cargo(&ws.root());
    let result = module_refs(&ws, "src/a.rs", "src/c/a.rs", refs).await;
    let mut record = finish("review-all-target-kinds", &ws, before, result);
    record["all_roots"] = json!(roots);
    save_named("review-all-target-kinds", record.clone());
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
    for root in roots {
        assert!(ws.path(root).is_file());
        assert!(!ws.read(root).contains("mod a;"), "{root}");
        assert!(ws.read(root).contains("use crate::c::a;"), "{root}");
    }
    assert!(!ws.path("src/a.rs").exists());
    assert!(ws.path("src/c/a.rs").is_file());
}

#[tokio::test]
async fn review_grouped_destination_imports_keep_other_bindings_and_aliases() {
    let c = "use crate::{a, a as alias, keep};\nmod nested { use crate::a; }\n#[test] fn consumer() { assert_eq!(a::VALUE + alias::VALUE + keep::VALUE, 21); }\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            "pub mod a;\npub mod c;\npub mod keep { pub const VALUE: u8 = 7; }\n",
        ),
        ("src/a.rs", "pub const VALUE: u8 = 7;\n"),
        ("src/c.rs", c),
    ]);
    let before = cargo(&ws.root());
    let refs = vec![
        ("src/c.rs", 1, 13),
        ("src/c.rs", 1, 16),
        ("src/c.rs", 2, 25),
    ];
    let result = module_refs(&ws, "src/a.rs", "src/c/a.rs", refs).await;
    let record = finish("review-grouped-destination-import", &ws, before, result);
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
    assert!(
        ws.read("src/c.rs")
            .contains("mod nested { use crate::c::a; }")
    );
}

#[tokio::test]
async fn review_unused_shared_module_updates_every_target_declaration() {
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "pub mod a;\npub mod c;\n"),
        ("src/a.rs", "pub const VALUE: u8 = 7;\n"),
        ("src/c.rs", ""),
        (
            "examples/demo.rs",
            "#[path=\"../src/a.rs\"] mod a;\n#[path=\"../src/c.rs\"] mod c;\nfn main() {}\n",
        ),
        (
            "benches/demo.rs",
            "#[path=\"../src/a.rs\"] mod a;\n#[path=\"../src/c.rs\"] mod c;\nfn main() {}\n",
        ),
        (
            "build.rs",
            "#[path=\"src/a.rs\"] mod a;\n#[path=\"src/c.rs\"] mod c;\nfn main() {}\n",
        ),
    ]);
    let before = cargo(&ws.root());
    let result = module_refs(&ws, "src/a.rs", "src/c/a.rs", Vec::new()).await;
    let record = finish("review-unused-target-declarations", &ws, before, result);
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
    for root in [
        "src/lib.rs",
        "examples/demo.rs",
        "benches/demo.rs",
        "build.rs",
    ] {
        assert!(!ws.read(root).contains("mod a;"));
    }
}

#[tokio::test]
async fn review_explicit_nested_target_roots_work_with_autodiscovery_disabled() {
    let manifest = "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\nautobins=false\nautoexamples=false\nautobenches=false\nbuild=false\n[[example]]\nname='demo'\n[[bench]]\nname='measure'\n";
    let consumer =
        "#[path=\"../../src/a.rs\"] mod a;\n#[path=\"../../src/c.rs\"] mod c;\nfn main() {}\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", manifest),
        ("src/lib.rs", "pub mod a;\npub mod c;\n"),
        ("src/a.rs", "pub const VALUE: u8 = 7;\n"),
        ("src/c.rs", ""),
        ("examples/demo/main.rs", consumer),
        ("benches/measure/main.rs", consumer),
        (
            "examples/ignored.rs",
            "compile_error!(\"disabled automatic example\");\n",
        ),
        (
            "benches/ignored.rs",
            "compile_error!(\"disabled automatic bench\");\n",
        ),
        ("build.rs", "compile_error!(\"disabled build script\");\n"),
    ]);
    let before = cargo(&ws.root());
    let result = module_refs(&ws, "src/a.rs", "src/c/a.rs", Vec::new()).await;
    let record = finish("review-explicit-nested-targets", &ws, before, result);
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
    for root in ["examples/demo/main.rs", "benches/measure/main.rs"] {
        assert!(!ws.read(root).contains("mod a;"));
    }
}

#[tokio::test]
async fn review_child_move_keeps_nested_module_parent_meaning() {
    let source = "mod child;\nconst VALUE: u8 = 7;\n#[test]\nfn f() {\n mod inner {\n  pub(super) fn value() -> u8 { super::VALUE }\n }\n assert_eq!(inner::value(), 7);\n}\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", source),
        ("src/child.rs", "const VALUE: u8 = 99;\n"),
    ]);
    let before = cargo(&ws.root());
    let result = item(&ws, "src/lib.rs", "src/child.rs", "f", 4, 9).await;
    let record = finish("review-nested-child-scope", &ws, before, result);
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
}
