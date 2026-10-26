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
mod report {
    use super::super::types::{ModulePath, Move};

    fn moved() -> Move {
        Move {
            symbol: "Shape".into(),
            root: "/nonexistent".into(),
            from: "src/a.rs".into(),
            from_module: "crate::a".into(),
            to: "src/b.rs".into(),
            to_module: "crate::b".into(),
            new_path: "demo::b::Shape".into(),
            moved_lines: 7,
            rewritten: vec![],
            imports: vec![],
            left_alone: vec![],
            unmatched: vec![],
            diagnostics: vec![],
            applied: false,
            created: None,
        }
    }

    const HEAD: &str = "`Shape` moved\n\n- from: src/a.rs (crate::a)\n- to:   src/b.rs (crate::b)\n- callers now import: `demo::b::Shape`\n- 7 line(s) moved\n";

    #[test]
    fn a_plain_move_reports_its_header_and_that_nothing_was_written() {
        assert_eq!(
            moved().render(100),
            format!(
                "{HEAD}\n0 changed line(s) in 0 file(s)\n\n\nthe analyzer accepts the result: 0 errors\n\nnothing was written; pass `apply: true` to make these edits\n"
            )
        );
    }

    #[test]
    fn a_created_module_imports_leftovers_and_a_private_scope_hint_are_listed() {
        let dir = tempfile::Builder::new().prefix("mv").tempdir().unwrap();
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let mut m = moved();
        m.root = dir.path().to_path_buf();
        m.created = Some(("src/b.rs".into(), "src/lib.rs".into()));
        m.rewritten = vec![(path.to_string_lossy().into_owned(), "one\n2\n".into())];
        m.imports = vec!["src/c.rs: use crate::b::Shape".into()];
        m.unmatched = vec!["src/d.rs:3:1".into()];
        m.left_alone = vec!["src/e.rs:9:1 (a macro)".into()];
        m.diagnostics = vec!["src/b.rs:4: cannot find `helper` in this scope (src/b.rs)".into()];
        m.applied = true;
        let out = m.render(10_000);
        assert!(out.starts_with(&format!("{HEAD}- src/b.rs is new, declared in src/lib.rs\n\n2 changed line(s) in 1 file(s)\n\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n one\n-two\n+2\n")));
        assert!(out.contains("\nimports:\n  src/c.rs: use crate::b::Shape\n"));
        assert!(out.contains("\nnot rewritten; nothing is written while any remains, forced or not:\n  src/d.rs:3:1\n"));
        assert!(out.contains("\nleft alone:\n  src/e.rs:9:1 (a macro)\n"));
        assert!(out.contains("\nthe analyzer rejects the result:\n  src/b.rs:4: cannot find `helper` in this scope (src/b.rs)\n"));
        assert!(out.contains("\nnames it cannot see from `crate::b`: the item used something private to `crate::a`. Move that too, or widen it to `pub(crate)`, and run this again.\n"));
        assert!(out.ends_with("\n[applied to 1 file(s)]\n"));

        m.diagnostics = vec!["src/b.rs:4: mismatched types".into()];
        assert!(!m.render(10_000).contains("names it cannot see"));
        let cut = m.render(5);
        assert!(cut.contains("file(s)\n\n--- a\n… diff truncated\n"));
    }

    #[test]
    fn a_module_path_is_spelled_relative_to_the_crate_asking() {
        let nested = ModulePath {
            krate: "demo".into(),
            segments: vec!["a".into(), "b".into()],
        };
        assert_eq!(nested.spelled_from("demo"), "crate::a::b");
        assert_eq!(nested.spelled_from("other"), "demo::a::b");
        assert_eq!(nested.absolute(), "demo::a::b");
        let root = ModulePath {
            krate: "demo".into(),
            segments: vec![],
        };
        assert_eq!(root.spelled_from("demo"), "crate");
        assert_eq!(root.absolute(), "demo");
    }
}

#[cfg(test)]
mod refusals {
    use super::super::move_item;
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};

    fn crate_dir(manifest: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::Builder::new().prefix("mi").tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        for (rel, text) in [
            ("Cargo.toml", manifest),
            ("src/lib.rs", "pub fn a() {}\n"),
            ("src/b.rs", "pub fn b() {}\n"),
            ("src/b/mod.rs", "pub fn b() {}\n"),
            ("tools/x.rs", "pub fn x() {}\n"),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        (dir, root)
    }

    async fn refusal(root: &Path, file: &str, target: &str) -> String {
        let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let moved = move_item(
            addr,
            root,
            &root.join(file),
            1,
            8,
            &root.join(target),
            false,
            false,
        )
        .await
        .unwrap_err();
        moved.to_string().replace(root.to_str().unwrap(), "<root>")
    }

    #[tokio::test]
    async fn a_move_that_cannot_start_says_why_before_asking_the_analyzer() {
        let (_dir, root) = crate_dir("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n");
        let cases = [
            (
                "src/lib.rs",
                "src/lib.rs",
                "the item is already in src/lib.rs",
            ),
            (
                "src/gone.rs",
                "src/lib.rs",
                "cannot read <root>/src/gone.rs",
            ),
            (
                "src/lib.rs",
                "src/x.txt",
                "src/x.txt is not a Rust source file",
            ),
            (
                "src/lib.rs",
                "src/sub/x.rs",
                "target has no unique parent in the source Cargo root: no module declares <root>/src/sub; create the parent module first; nothing was written",
            ),
            (
                "src/b.rs",
                "src/b/mod.rs",
                // No module declares src/b.rs, so ownership refuses before comparing paths.
                "<root>/src/b.rs has no known Cargo/module owner; nothing was written",
            ),
            (
                "tools/x.rs",
                "src/lib.rs",
                "<root>/tools/x.rs has no known Cargo/module owner; nothing was written",
            ),
        ];
        for (file, target, want) in cases {
            assert_eq!(
                refusal(&root, file, target).await,
                want,
                "{file} -> {target}"
            );
        }
    }

    #[tokio::test]
    async fn a_manifest_without_a_package_name_is_refused() {
        let (_dir, root) = crate_dir("[workspace]\n");
        assert_eq!(
            refusal(&root, "src/lib.rs", "src/b.rs").await,
            "no Cargo target roots found; nothing was written"
        );
    }

    #[tokio::test]
    async fn an_unreachable_analyzer_stops_a_planned_move() {
        let (_dir, root) = crate_dir("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n");
        assert!(!refusal(&root, "src/lib.rs", "src/b.rs").await.is_empty());
    }
}
