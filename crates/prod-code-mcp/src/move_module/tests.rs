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
    assert!(declaration("    mod b;\n", "b").is_some());
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
    assert_eq!(n, 3);
    assert_eq!(
        out,
        "use crate::a::helper;\nfn f() { crate::a::x(); my_super::y(); crate::z(); }\n"
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

mod report {
    use super::super::types::ModuleMove;

    fn moved(root: &std::path::Path) -> ModuleMove {
        ModuleMove {
            module: "util".into(),
            root: root.to_path_buf(),
            from_module: "demo::a".into(),
            to_module: "demo::b".into(),
            moved: vec![("a/util.rs".into(), "b/util.rs".into())],
            rewritten: vec![],
            removed: vec!["a/util.rs".into()],
            notes: vec![],
            unmatched: vec![],
            diagnostics: vec![],
            applied: false,
        }
    }

    #[test]
    fn the_report_lists_moves_notes_unmatched_paths_and_the_analyzers_verdict() {
        let dir = tempfile::Builder::new().prefix("mm").tempdir().unwrap();
        let mut m = moved(dir.path());
        assert_eq!(
            m.render(100),
            "module `util` moved: demo::a -> demo::b\n\n- a/util.rs -> b/util.rs\n\n\nthe analyzer accepts the result: 0 errors\n\nnothing was written (call `write` to apply)\n"
        );
        let kept = dir.path().join("lib.rs");
        std::fs::write(&kept, "mod a;\nmod b;\n").unwrap();
        m.rewritten = vec![
            (
                dir.path().join("b/util.rs").to_string_lossy().into_owned(),
                "fn f() {}\n".into(),
            ),
            (kept.to_string_lossy().into_owned(), "mod b;\n".into()),
        ];
        m.notes = vec!["lib.rs: `mod a;` dropped".into()];
        m.unmatched = vec!["src/x.rs:3:5".into()];
        m.diagnostics = vec!["cannot find `f` (b/util.rs:1:1)".into()];
        m.applied = true;
        let out = m.render(10_000);
        assert!(out.starts_with("module `util` moved: demo::a -> demo::b\n\n- a/util.rs -> b/util.rs\n\n- lib.rs: `mod a;` dropped\n\nnot rewritten; nothing is written while any remains, forced or not:\n  src/x.rs:3:5\n\n--- a/b/util.rs\n+++ b/b/util.rs\n@@ -0,0 +1 @@\n+fn f() {}\n"));
        assert!(out.contains("--- a/lib.rs\n+++ b/lib.rs\n@@ -1,2 +1 @@\n-mod a;\n mod b;\n"));
        assert!(out.ends_with(
            "\nthe analyzer rejects the result:\n  cannot find `f` (b/util.rs:1:1)\n\n[applied]\n"
        ));
        let cut = m.render(6);
        assert!(cut.contains("\n\n--- a/\n… diff truncated\n"));
    }

    #[test]
    fn a_move_with_unmatched_paths_or_errors_is_not_written() {
        let dir = tempfile::Builder::new().prefix("mm").tempdir().unwrap();
        let old = dir.path().join("a/util.rs");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "fn f() {}\n").unwrap();
        let mut m = moved(dir.path());
        m.unmatched = vec!["x.rs:1:1".into(), "y.rs:2:2".into()];
        assert_eq!(
            m.write(true).unwrap_err().to_string(),
            "2 path(s) to `util` were not rewritten; nothing was written:\n  x.rs:1:1\n  y.rs:2:2"
        );
        m.unmatched.clear();
        m.diagnostics = vec!["boom (a:1:1)".into()];
        assert_eq!(
            m.write(false).unwrap_err().to_string(),
            "the move does not compile (1 error(s)); nothing was written:\n  boom (a:1:1)"
        );
        assert!(old.exists() && !m.applied);
    }

    #[test]
    fn a_forced_write_creates_the_new_file_deletes_the_old_one_and_its_empty_directory() {
        let dir = tempfile::Builder::new().prefix("mm").tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let old = root.join("a/util.rs");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "fn f() {}\n").unwrap();
        let new = root.join("b/util.rs");
        let mut m = moved(&root);
        m.rewritten = vec![(new.to_string_lossy().into_owned(), "fn f() {}\n".into())];
        m.diagnostics = vec!["ignored when forced".into()];
        m.write(true).unwrap();
        assert!(m.applied);
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "fn f() {}\n");
        assert!(!old.exists());
        assert!(
            !root.join("a").exists(),
            "the directory the module left is gone"
        );
    }
}
