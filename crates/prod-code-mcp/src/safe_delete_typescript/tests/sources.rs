/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::super::sources::{
    bind_original, collect_sources, display, inspect_source, module_marker, project_snapshot,
    refuse_linked_path, regular_unlinked_inside, unchanged,
};

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix("ts").tempdir().unwrap();
    for (rel, text) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn err<T>(r: anyhow::Result<T>) -> String {
    r.err().expect("an error").to_string()
}

#[test]
fn only_contained_ts_sources_are_collected() {
    let dir = project(&[
        ("a.ts", ""),
        ("sub/b.ts", ""),
        ("c.json", ""),
        ("sub/d.md", ""),
    ]);
    let mut found = BTreeSet::new();
    collect_sources(dir.path(), dir.path(), &mut found).unwrap();
    let want: BTreeSet<PathBuf> = ["a.ts", "sub/b.ts"]
        .iter()
        .map(|rel| std::fs::canonicalize(dir.path().join(rel)).unwrap())
        .collect();
    assert_eq!(found, want);

    for name in ["x.js", "x.jsx", "x.mjs", "x.cjs", "x.tsx"] {
        let dir = project(&[("sub/ok.ts", ""), (&format!("sub/{name}"), "")]);
        assert_eq!(
            err(collect_sources(
                dir.path(),
                dir.path(),
                &mut BTreeSet::new()
            )),
            format!("sub/{name} is JavaScript or TSX; only contained .ts projects are supported")
        );
    }
    let dir = project(&[("types.d.ts", "")]);
    assert_eq!(
        err(collect_sources(
            dir.path(),
            dir.path(),
            &mut BTreeSet::new()
        )),
        "ambient declaration source types.d.ts is not supported"
    );
    let gone = dir.path().join("gone");
    assert_eq!(
        err(collect_sources(dir.path(), &gone, &mut BTreeSet::new())),
        format!("cannot inspect TypeScript project {}", gone.display())
    );
}

#[cfg(unix)]
#[test]
fn a_linked_path_refuses_the_whole_project() {
    let dir = project(&[("a.ts", "")]);
    let other = project(&[("o.ts", "")]);
    std::os::unix::fs::symlink(other.path(), dir.path().join("link")).unwrap();
    let want = "the TypeScript project contains linked path link; deletion is refused";
    assert_eq!(
        err(collect_sources(
            dir.path(),
            dir.path(),
            &mut BTreeSet::new()
        )),
        want
    );
    assert_eq!(err(project_snapshot(dir.path())), want);
}

#[test]
fn generated_dynamic_and_regex_sources_are_outside_the_subset() {
    let ok = [
        "const a = 1;\n",
        "import x from 'y';\n",
        "const evaluate = 1;\n",
        "é = 'é';\n",
        "Function.prototype;\n",
        "",
    ];
    for text in ok {
        inspect_source(text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
    }
    let generated = "generated TypeScript source is not supported";
    for text in [
        "// @generated\nconst a = 1;",
        "/* Generated File */",
        "// DO NOT EDIT",
        "a;\n//# sourceMappingURL=a.map",
    ] {
        assert_eq!(err(inspect_source(text)), generated, "{text:?}");
    }
    let dynamic = "dynamic evaluation or name resolution is not supported";
    for text in [
        "eval('1')",
        "eval",
        "Function ('a')",
        "require/* c */('x')",
        "import('x')",
    ] {
        assert_eq!(err(inspect_source(text)), dynamic, "{text:?}");
    }
    assert_eq!(
        err(inspect_source("a = b / c;")),
        "regular-expression and division syntax is outside the safe-delete subset"
    );
    assert_eq!(
        err(inspect_source("a\\u0062 = 1;")),
        "escaped identifiers are outside the safe-delete subset"
    );
    assert_eq!(
        err(inspect_source("a = 'x")),
        "a quoted literal is not terminated"
    );
}

#[test]
fn a_module_is_a_top_level_import_or_export() {
    for (text, want) in [
        ("import x from 'y';", true),
        ("function f() {}\nexport {}", true),
        ("const a = { import: 1 };", false),
        ("const a = 1;", false),
        ("// import\nlet a;", false),
        ("'export'", false),
        ("let imported = 1;", false),
        ("", false),
    ] {
        assert_eq!(module_marker(text).unwrap(), want, "{text:?}");
    }
    assert_eq!(
        err(module_marker("a)")),
        "unmatched delimiter in TypeScript source"
    );
    assert_eq!(
        err(module_marker("(a]")),
        "mismatched delimiter in TypeScript source"
    );
    assert_eq!(
        err(module_marker("(a")),
        "unclosed delimiter in TypeScript source"
    );
}

#[test]
fn a_snapshot_skips_tooling_dirs_and_detects_changes() {
    let dir = project(&[
        ("a.ts", "x"),
        ("sub/b.ts", "y"),
        ("node_modules/m/i.js", "z"),
        (".git/HEAD", "h"),
        ("target/t", "t"),
    ]);
    let snap = project_snapshot(dir.path()).unwrap();
    let keys: Vec<&Path> = snap.keys().map(PathBuf::as_path).collect();
    assert_eq!(keys, [Path::new("a.ts"), Path::new("sub/b.ts")]);
    assert_eq!(snap[Path::new("a.ts")], b"x");
    unchanged(dir.path(), &snap, "the compiler").unwrap();
    std::fs::write(dir.path().join("a.ts"), "changed").unwrap();
    assert_eq!(
        err(unchanged(dir.path(), &snap, "the compiler")),
        "the TypeScript project changed while the compiler was inspected; nothing was written"
    );
}

#[cfg(unix)]
#[test]
fn a_path_through_a_link_or_outside_the_checkout_is_refused() {
    let dir = project(&[("src/a.ts", "")]);
    let other = project(&[("o.ts", "")]);
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::os::unix::fs::symlink(other.path(), root.join("link")).unwrap();
    refuse_linked_path(&root, &root, &root.join("src/a.ts")).unwrap();
    assert_eq!(
        err(refuse_linked_path(&root, &root, &root.join("link/o.ts"))),
        format!(
            "the requested TypeScript path contains linked path {}; deletion is refused",
            root.join("link").display()
        )
    );
    let outside = Path::new("/definitely/elsewhere/a.ts");
    assert_eq!(
        err(refuse_linked_path(&root, &root, outside)),
        "/definitely/elsewhere/a.ts is outside the checkout; nothing was written"
    );
    let parent = root.join("../up.ts");
    assert_eq!(
        err(refuse_linked_path(&root, &root, &parent)),
        format!(
            "{} is outside the checkout; nothing was written",
            parent.display()
        )
    );
    assert_eq!(
        err(refuse_linked_path(&root, &root, &root.join("src/none.ts"))),
        format!(
            "cannot inspect {}; nothing was written",
            root.join("src/none.ts").display()
        )
    );
    refuse_linked_path(Path::new("/other"), &root, &root.join("src/a.ts")).unwrap();
}

#[cfg(unix)]
#[test]
fn a_source_must_be_a_regular_unlinked_file_inside_the_project() {
    let dir = project(&[("src/a.ts", "")]);
    let outside = project(&[("o.ts", "")]);
    let root = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(
        regular_unlinked_inside(&root, Path::new("src/a.ts")).unwrap(),
        root.join("src/a.ts")
    );
    assert_eq!(
        regular_unlinked_inside(&root, &root.join("src/a.ts")).unwrap(),
        root.join("src/a.ts")
    );
    std::os::unix::fs::symlink(root.join("src/a.ts"), root.join("alias.ts")).unwrap();
    let not_regular = |p: &Path| {
        format!(
            "{} is not an unlinked regular source file; nothing was written",
            p.display()
        )
    };
    assert_eq!(
        err(regular_unlinked_inside(&root, Path::new("alias.ts"))),
        not_regular(&root.join("alias.ts"))
    );
    assert_eq!(
        err(regular_unlinked_inside(&root, Path::new("src"))),
        not_regular(&root.join("src"))
    );
    assert_eq!(
        err(regular_unlinked_inside(&root, Path::new("gone.ts"))),
        format!(
            "cannot inspect {}; nothing was written",
            root.join("gone.ts").display()
        )
    );
    let o = outside.path().join("o.ts");
    assert_eq!(
        err(regular_unlinked_inside(&root, &o)),
        format!(
            "{} is outside the TypeScript project; nothing was written",
            o.display()
        )
    );
    assert_eq!(display(&root, &root.join("src/a.ts")), "src/a.ts");
    assert_eq!(display(&root, Path::new("/x/y")), "/x/y");
}

/// The read and the snapshot are two steps: an edit between them, here `keep = 1` becoming
/// `keep = 2`, leaves the snapshot and every later check agreeing with each other but not with
/// the text the deletion is cut from. That mismatch is refused (#1398).
#[test]
fn the_snapshot_must_hold_the_text_the_deletion_is_planned_on() {
    let dir = project(&[
        ("tsconfig.json", "{}"),
        ("src/a.ts", "export const keep = 1;\nfunction unused() {}\n"),
    ]);
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let file = root.join("src/a.ts");
    let original = std::fs::read_to_string(&file).unwrap();
    // The same bytes in the snapshot: bound.
    let observed = project_snapshot(&root).unwrap();
    bind_original(&root, &observed, &file, &original).unwrap();
    // A concurrent edit after the first read, before the snapshot.
    std::fs::write(&file, "export const keep = 2;\nfunction unused() {}\n").unwrap();
    let observed = project_snapshot(&root).unwrap();
    let error = bind_original(&root, &observed, &file, &original)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("changed between its first read and the project snapshot"),
        "{error}"
    );
    // Later checks alone would not notice: the project is unchanged since that snapshot.
    unchanged(&root, &observed, "references").unwrap();
}
