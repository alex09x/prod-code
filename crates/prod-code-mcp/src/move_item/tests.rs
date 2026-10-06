/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[cfg(test)]
mod tests {
    use super::super::imports::{add_import, carry_imports, drop_import, mentions, requalify};
    use super::super::item::{cut, span_at, with_doc_comment};
    use super::super::module::{crate_name, declare_module, parent_module_file};
    use super::super::types::ModulePath;

    #[test]
    fn a_new_module_is_declared_after_the_last_mod_or_at_the_top() {
        let with_mods = "//! Root.\n\npub mod a;\nmod b;\n\npub fn f() {}\n";
        assert_eq!(
            declare_module(with_mods, "util", true),
            "//! Root.\n\npub mod a;\nmod b;\npub mod util;\n\npub fn f() {}\n"
        );
        let without = "//! Root.\n#![allow(dead_code)]\npub fn f() {}\n";
        assert_eq!(
            declare_module(without, "util", false),
            "//! Root.\n#![allow(dead_code)]\nmod util;\n\npub fn f() {}\n"
        );
        let nested = "fn f() {\n    mod inner;\n}\n";
        assert!(declare_module(nested, "util", false).starts_with("mod util;\n\nfn f()"));
        assert_eq!(
            declare_module("\nuse crate::x;\n", "util", true),
            "pub mod util;\n\nuse crate::x;\n"
        );
    }

    #[test]
    fn a_new_module_s_parent_is_the_crate_root_or_the_directory_s_module() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        std::fs::create_dir_all(src.join("a")).unwrap();
        std::fs::write(src.join("lib.rs"), "").unwrap();
        std::fs::write(src.join("a.rs"), "").unwrap();
        assert_eq!(
            parent_module_file(&src.join("util.rs")),
            Some(src.join("lib.rs"))
        );
        assert_eq!(
            parent_module_file(&src.join("a/util.rs")),
            Some(src.join("a.rs"))
        );
        assert_eq!(parent_module_file(&src.join("b/util.rs")), None);
    }

    #[test]
    fn a_crate_name_comes_from_lib_then_package_and_is_spelled_in_rust() {
        assert_eq!(
            crate_name("[package]\nname = \"prod-code-mcp\"\n").as_deref(),
            Some("prod_code_mcp")
        );
        assert_eq!(
            crate_name("[package]\nname = \"a\"\n\n[lib]\nname = \"b\"\n").as_deref(),
            Some("b")
        );
        assert_eq!(crate_name("[dependencies]\nname = \"x\"\n"), None);
    }

    #[test]
    fn a_module_spells_itself_one_way_inside_its_crate_and_another_outside() {
        let m = ModulePath {
            krate: "prod_code_mcp".into(),
            segments: vec!["fixture".into()],
        };
        assert_eq!(m.spelled_from("prod_code_mcp"), "crate::fixture");
        assert_eq!(m.spelled_from("prod_code_client"), "prod_code_mcp::fixture");
        assert_eq!(m.absolute(), "prod_code_mcp::fixture");
    }

    #[test]
    fn a_doc_comment_and_its_attributes_are_part_of_the_item() {
        let text = "use a;\n\n/// What it does.\n#[inline]\npub fn f() {}\n";
        assert_eq!(with_doc_comment(text, 5), 3);
        let (rest, item) = cut(text, 3, 5);
        assert_eq!(item, "/// What it does.\n#[inline]\npub fn f() {}");
        assert_eq!(rest, "use a;\n\n");
    }

    #[test]
    fn an_import_is_dropped_whole_or_narrowed_to_what_is_left() {
        let (text, notes) = drop_import("use a::b::Name;\nuse c::D;\n", "Name");
        assert_eq!(text, "use c::D;\n");
        assert_eq!(notes.len(), 1);

        let (text, notes) = drop_import("use a::{B, Name, C};\n", "Name");
        assert_eq!(text, "use a::{B, C};\n");
        assert!(notes[0].contains("narrowed"));

        let (text, notes) = drop_import("use a::b::Other;\n", "Name");
        assert_eq!(text, "use a::b::Other;\n");
        assert!(notes.is_empty());
    }

    #[test]
    fn an_import_lands_after_the_ones_that_are_there_and_never_twice() {
        let text = "//! A module.\n\nuse a::B;\nuse c::D;\n\npub fn f() {}\n";
        let once = add_import(text, "use e::F;");
        assert_eq!(
            once,
            "//! A module.\n\nuse a::B;\nuse c::D;\nuse e::F;\n\npub fn f() {}\n"
        );
        assert_eq!(add_import(&once, "use e::F;"), once);
    }

    #[test]
    fn an_import_goes_under_the_header_when_the_file_has_none() {
        let text = "//! A module.\n\npub fn f() {}\n";
        assert_eq!(
            add_import(text, "use e::F;"),
            "//! A module.\n\nuse e::F;\npub fn f() {}\n"
        );
    }

    #[test]
    fn an_import_inside_a_body_is_not_one_of_the_files_imports() {
        let text = "use a::B;\n\nfn f() {\n    use std::io::Write;\n}\n";
        assert_eq!(
            add_import(text, "use e::F;"),
            "use a::B;\nuse e::F;\n\nfn f() {\n    use std::io::Write;\n}\n"
        );
        let (out, notes) = drop_import(text, "Write");
        assert_eq!(out, text);
        assert!(notes.is_empty());
    }

    #[test]
    fn an_item_carries_the_imports_it_spells_and_no_others() {
        let source = "use anyhow::{Context, Result};\nuse std::path::Path;\n\nfn f() {}\n";
        let item = "fn moved(p: &Path) -> Result<()> { Ok(()) }";
        let (target, notes) = carry_imports(source, item, "//! Target.\n");
        assert!(target.contains("use anyhow::Result;"), "{target}");
        assert!(target.contains("use std::path::Path;"), "{target}");
        assert!(
            !target.contains("Context"),
            "an import the item does not spell is not carried: {target}"
        );
        assert_eq!(notes.len(), 2);

        let (again, notes) = carry_imports(source, item, &target);
        assert_eq!(again, target);
        assert!(notes.is_empty());
    }

    #[test]
    fn a_name_inside_a_longer_identifier_is_not_a_mention() {
        assert!(mentions("fn f(p: &Path) {}", "Path"));
        assert!(!mentions("fn f(p: &PathBuf) {}", "Path"));
        assert!(!mentions("let my_path = 1;", "path"));
    }

    #[test]
    fn a_qualified_reference_is_requalified_and_a_bare_one_is_left_to_its_import() {
        let text = "fn g() {\n    crate::old::Name::new();\n    Name::new();\n}\n";
        let (out, bare) = requalify(text, &[(2, 17), (3, 5)], "Name", "crate::new_home");
        assert_eq!(
            out,
            "fn g() {\n    crate::new_home::Name::new();\n    Name::new();\n}\n"
        );
        assert_eq!(
            bare, 1,
            "the bare `Name` is the import's job, not this one's"
        );
    }

    #[test]
    fn the_smallest_declaration_containing_a_line_is_the_one_that_moves() {
        let symbols = serde_json::json!([
            { "name": "Outer", "range": { "start": { "line": 0 }, "end": { "line": 20 } },
              "children": [
                { "name": "inner", "range": { "start": { "line": 4 }, "end": { "line": 8 } } }
              ] }
        ]);
        assert_eq!(span_at(&symbols, 6), Some(("inner".to_string(), 5, 9)));
        assert_eq!(span_at(&symbols, 2), Some(("Outer".to_string(), 1, 21)));
    }
}
