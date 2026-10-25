/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use crate::parameter_object::Language;

use super::callers::{rewrite_caller_imports, rewrite_caller_imports_checked};

use Language::{C, Cpp, Go, Python, Swift, TypeScript};

fn rewrite(
    lang: Language,
    caller: (&str, &str),
    decl: &str,
    files: (&str, &str),
) -> Option<(String, String)> {
    rewrite_caller_imports(
        caller.1,
        decl,
        Path::new(caller.0),
        Path::new(files.0),
        Path::new(files.1),
        Path::new("/w"),
        lang,
    )
}

#[test]
fn typescript_caller_table() {
    let (caller, source, target) = ("/w/src/caller.ts", "/w/src/a.ts", "/w/src/b.ts");
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &str, Option<(&str, &str)>)] = &[
        (
            "Foo",
            "import { Foo } from \"./a\";\nFoo();\n",
            Some((
                "import { Foo } from \"./b\";\nFoo();\n",
                "rewrote import to `./b`",
            )),
        ),
        (
            "Foo",
            "import { Foo, Bar } from \"./a\";\nFoo(); Bar();\n",
            Some((
                "import { Bar } from \"./a\";\nimport { Foo } from \"./b\";\nFoo(); Bar();\n",
                "imported `Foo` from `./b`",
            )),
        ),
        (
            "Foo",
            "Foo();\n",
            Some((
                "import { Foo } from \"./b\";\nFoo();\n",
                "added import `Foo` from `./b`",
            )),
        ),
        ("Foo", "const x = 1;\n", None),
        ("Foo", "import { Foo } from \"./b\";\nFoo();\n", None),
        // identifiers that contain the source module name survive the rewrite
        (
            "Data",
            "import { Data } from \"./a\";\nData();\n",
            Some((
                "import { Data } from \"./b\";\nData();\n",
                "rewrote import to `./b`",
            )),
        ),
    ];
    for (idx, (decl, text, expected)) in cases.iter().enumerate() {
        let got = rewrite(TypeScript, (caller, text), decl, (source, target));
        let want = expected.map(|(a, b)| (a.to_string(), b.to_string()));
        assert_eq!(got, want, "case {idx}");
    }
    let got = rewrite(
        TypeScript,
        ("/w/src/x/c.ts", "import { Foo } from \"../a\";\nFoo();"),
        "Foo",
        (source, "/w/lib/b.ts"),
    );
    assert_eq!(
        got,
        Some((
            "import { Foo } from \"../../lib/b\";\nFoo();".to_string(),
            "rewrote import to `../../lib/b`".to_string()
        ))
    );
}

#[test]
fn python_caller_table() {
    let caller = "/w/pkg/c.py";
    let files = ("/w/pkg/a.py", "/w/pkg/b.py");
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &str, Option<(&str, &str)>)] = &[
        (
            "foo",
            "from pkg.a import foo\nfoo()\n",
            Some(("from pkg.b import foo\nfoo()\n", "from pkg.b import foo")),
        ),
        (
            "foo",
            "from pkg.a import foo, bar\nfoo()",
            Some((
                "from pkg.a import bar\nfrom pkg.b import foo\nfoo()",
                "from pkg.b import foo",
            )),
        ),
        ("foo", "from pkg.a import bar\n", None),
        ("foo", "import pkg.a\npkg.a.foo()\n", None),
        ("foo", "from pkg.ab import foo\nfoo()\n", None),
    ];
    for (idx, (decl, text, expected)) in cases.iter().enumerate() {
        let got = rewrite(Python, (caller, text), decl, files);
        let want = expected.map(|(a, b)| (a.to_string(), b.to_string()));
        assert_eq!(got, want, "case {idx}");
    }
}

#[test]
fn cpp_caller_and_unsupported_languages() {
    let caller = "/w/src/c.cpp";
    let files = ("/w/src/a.h", "/w/src/b.h");
    let got = rewrite(Cpp, (caller, "#include \"a.h\"\nint x;"), "foo", files);
    assert_eq!(
        got,
        Some((
            "#include \"a.h\"\n#include \"b.h\"\nint x;".to_string(),
            "included \"b.h\"".to_string()
        ))
    );
    let already = "#include \"a.h\"\n#include \"b.h\"\n";
    assert_eq!(rewrite(C, (caller, already), "foo", files), None);
    assert_eq!(rewrite(Cpp, (caller, "int x;\n"), "foo", files), None);
    assert_eq!(rewrite(Swift, (caller, "import A\n"), "foo", files), None);
}

struct GoTree {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
}

fn go_tree() -> GoTree {
    let temp = tempfile::Builder::new()
        .prefix("callers")
        .tempdir()
        .unwrap();
    let root = temp.path().to_path_buf();
    std::fs::write(root.join("go.mod"), "module example.test/m\n").unwrap();
    for (dir, pkg) in [
        ("lib", "lib"),
        ("util", "util"),
        ("app", "main"),
        ("same", "lib"),
    ] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        std::fs::write(root.join(dir).join("f.go"), format!("package {pkg}\n")).unwrap();
    }
    GoTree { _temp: temp, root }
}

fn go_rewrite(
    tree: &GoTree,
    caller: &str,
    text: &str,
    decl: &str,
    target: &str,
) -> Option<(String, String)> {
    rewrite_caller_imports(
        text,
        decl,
        &tree.root.join(caller),
        &tree.root.join("lib/f.go"),
        &tree.root.join(target),
        &tree.root,
        Go,
    )
}

#[test]
fn go_callers_follow_the_moved_function() {
    let tree = go_tree();
    let app = "package main\n\nimport \"example.test/m/lib\"\n\nfunc main() { lib.Foo() }\n";
    assert_eq!(
        go_rewrite(&tree, "app/m.go", app, "Foo", "util/f.go"),
        Some((
            "package main\n\nimport \"example.test/m/util\"\n\nfunc main() { util.Foo() }\n"
                .to_string(),
            "rewrote `lib.Foo` to `util.Foo`".to_string()
        ))
    );

    let both =
        "package main\n\nimport \"example.test/m/lib\"\n\nfunc main() { lib.Foo(); lib.Bar() }\n";
    assert_eq!(
        go_rewrite(&tree, "app/m.go", both, "Foo", "util/f.go"),
        Some((
            "package main\n\nimport \"example.test/m/util\"\n\nimport \"example.test/m/lib\"\n\nfunc main() { util.Foo(); lib.Bar() }\n"
                .to_string(),
            "rewrote `lib.Foo` to `util.Foo`".to_string()
        ))
    );

    // callers that never mention the old qualified call, and packages that do not change
    assert_eq!(
        go_rewrite(&tree, "app/m.go", "package main\n", "Foo", "util/f.go"),
        None
    );
    assert_eq!(go_rewrite(&tree, "app/m.go", app, "Foo", "same/f.go"), None);
    assert_eq!(go_rewrite(&tree, "app/m.go", app, "Foo", "lib/g.go"), None);
}

#[test]
fn go_caller_inside_the_source_package_qualifies_the_call() {
    let tree = go_tree();
    let text = "package lib\n\nfunc g() { Foo() }\n";
    assert_eq!(
        go_rewrite(&tree, "lib/other.go", text, "Foo", "util/f.go"),
        Some((
            "package lib\n\nimport \"example.test/m/util\"\n\nfunc g() { util.Foo() }\n"
                .to_string(),
            "imported \"example.test/m/util\"".to_string()
        ))
    );
    assert_eq!(
        go_rewrite(
            &tree,
            "lib/other.go",
            "package lib\n\nfunc g() {}\n",
            "Foo",
            "util/f.go"
        ),
        None
    );
    // a local variable named like the moved symbol cannot be qualified safely
    let shadowed = "package lib\n\nfunc g() { Foo := 1; _ = Foo }\n";
    let err = rewrite_caller_imports_checked(
        shadowed,
        "Foo",
        &tree.root.join("lib/other.go"),
        &tree.root.join("lib/f.go"),
        &tree.root.join("util/f.go"),
        &tree.root,
        Go,
    )
    .expect_err("shadowed binding");
    assert!(
        err.to_string().contains("shadowed by a local binding"),
        "{err}"
    );
    assert_eq!(
        go_rewrite(&tree, "lib/other.go", shadowed, "Foo", "util/f.go"),
        None
    );
}
