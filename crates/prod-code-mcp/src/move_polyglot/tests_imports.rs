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

use super::imports::{
    carry_imports_polyglot, insert_or_merge_py_import, insert_or_merge_ts_import,
    remove_from_braced_ts_import, remove_from_py_from_import, top_import_insertion_pos,
    update_source_imports,
};

use Language::{C, Cpp, Go, Java, JavaScript, Python, Rust, Swift, TypeScript};

#[test]
fn insertion_position_skips_leading_comments_and_pragmas() {
    for (text, expected) in [
        ("", 0),
        ("const a = 1;\n", 0),
        ("#!/usr/bin/env node\n'use strict';\nconst a = 1;\n", 34),
        ("// c\n/* b */\n * x\nfoo", 18),
        ("\"use strict\";\nfoo", 14),
        ("// only", 7),
    ] {
        assert_eq!(top_import_insertion_pos(text), expected, "{text:?}");
    }
}

#[test]
fn ts_import_insert_or_merge_table() {
    let cases: &[(&str, &str, &str, &str, bool)] = &[
        (
            "const a = 1;\n",
            "B",
            "./b",
            "import { B } from \"./b\";\nconst a = 1;\n",
            true,
        ),
        (
            "// head\nconst a = 1;\n",
            "B",
            "./b",
            "// head\nimport { B } from \"./b\";\nconst a = 1;\n",
            true,
        ),
        (
            "import { A } from \"./b\";\nuse(A);\n",
            "B",
            "./b",
            "import { A, B } from \"./b\";\nuse(A);\n",
            true,
        ),
        (
            "import {} from \"./b\";\n",
            "B",
            "./b",
            "import { B } from \"./b\";\n",
            true,
        ),
        (
            "import { B } from \"./b\";\n",
            "B",
            "./b",
            "import { B } from \"./b\";\n",
            false,
        ),
        (
            "import B from \"./b\";\n",
            "B",
            "./b",
            "import B from \"./b\";\n",
            false,
        ),
        (
            "import * as ns from \"./b\";\n",
            "B",
            "./b",
            "import { B } from \"./b\";\nimport * as ns from \"./b\";\n",
            true,
        ),
        // a longer specifier that merely starts with the wanted one is another module
        (
            "import { A } from \"./bar\";\nuse(A);\n",
            "B",
            "./b",
            "import { B } from \"./b\";\nimport { A } from \"./bar\";\nuse(A);\n",
            true,
        ),
    ];
    for (idx, (content, sym, rel, expected, changed)) in cases.iter().enumerate() {
        let got = insert_or_merge_ts_import(content, sym, rel);
        assert_eq!(got, (expected.to_string(), *changed), "case {idx}");
    }
}

#[test]
fn py_import_insert_or_merge_table() {
    let cases: &[(&str, &str, &str, &str, bool)] = &[
        (
            "x = 1\n",
            "f",
            "pkg.m",
            "from pkg.m import f\nx = 1\n",
            true,
        ),
        (
            "#!/usr/bin/python\nx=1",
            "f",
            "pkg.m",
            "#!/usr/bin/python\nfrom pkg.m import f\nx=1",
            true,
        ),
        (
            "from pkg.m import a\nx\n",
            "f",
            "pkg.m",
            "from pkg.m import a, f\nx\n",
            true,
        ),
        (
            "from pkg.m import f\n",
            "f",
            "pkg.m",
            "from pkg.m import f\n",
            false,
        ),
        (
            "from pkg.mod import a\n",
            "f",
            "pkg.m",
            "from pkg.m import f\nfrom pkg.mod import a\n",
            true,
        ),
    ];
    for (idx, (content, sym, spec, expected, changed)) in cases.iter().enumerate() {
        let got = insert_or_merge_py_import(content, sym, spec);
        assert_eq!(got, (expected.to_string(), *changed), "case {idx}");
    }
}

#[test]
fn removal_from_import_lines() {
    let ts: &[(&str, &str, &str, bool)] = &[
        (
            "import { A, B } from \"./x\";",
            "A",
            "import { B } from \"./x\";",
            true,
        ),
        ("import { A } from \"./x\";", "A", "", true),
        (
            "import { A, B } from \"./x\";",
            "C",
            "import { A, B } from \"./x\";",
            false,
        ),
        (
            "import A from \"./x\";",
            "A",
            "import A from \"./x\";",
            false,
        ),
        ("import } A {", "A", "import } A {", false),
        (
            "import { A as AA, B } from 'x';",
            "A",
            "import { B } from 'x';",
            true,
        ),
        (
            "  import { A, B } from \"x\";",
            "B",
            "  import { A } from \"x\";",
            true,
        ),
        (
            "import { A, } from \"x\";",
            "Z",
            "import { A, } from \"x\";",
            false,
        ),
    ];
    for (idx, (line, sym, expected, removed)) in ts.iter().enumerate() {
        let got = remove_from_braced_ts_import(line, sym);
        assert_eq!(got, (expected.to_string(), *removed), "ts case {idx}");
    }
    let py: &[(&str, &str, &str, bool)] = &[
        ("from m import a, b", "a", "from m import b", true),
        ("from m import a", "a", "", true),
        ("from m import a, b", "c", "from m import a, b", false),
        ("import os", "os", "import os", false),
        ("from m import a as x, b", "a", "from m import b", true),
    ];
    for (idx, (line, sym, expected, removed)) in py.iter().enumerate() {
        let got = remove_from_py_from_import(line, sym);
        assert_eq!(got, (expected.to_string(), *removed), "py case {idx}");
    }
}

fn carry(lang: Language, source: &str, item: &str, target: &str) -> (String, Vec<String>) {
    carry_imports_polyglot(
        source,
        item,
        target,
        Path::new("src/a.ts"),
        Path::new("src/sub/b.ts"),
        lang,
        Path::new("."),
    )
}

#[test]
fn carry_typescript_imports_resolves_relative_specifiers() {
    let source = "import { A, B as C } from \"./dep\";\nimport D from \"lib\";\nimport { E } from \"lib2\";\nimport type X from nothing\n";
    let (out, notes) = carry(TypeScript, source, "A(); E();", "");
    assert_eq!(
        out,
        "import { E } from \"lib2\";\nimport { A } from \"../dep\";\n"
    );
    assert_eq!(
        notes,
        vec![
            "carried import `A` from `../dep`".to_string(),
            "carried import `E` from `lib2`".to_string()
        ]
    );
    // symbols the target already knows are not carried again
    let (same, none) = carry(
        JavaScript,
        source,
        "A();",
        "import { A } from \"../dep\";\n",
    );
    assert_eq!(same, "import { A } from \"../dep\";\n");
    assert!(none.is_empty());
}

#[test]
fn carry_python_go_swift_cpp_java_rust() {
    let py_source = "import os\nfrom typing import List, Optional\nimport json as js\n";
    let (out, notes) = carry(
        Python,
        py_source,
        "def f(x: List): return os.sep",
        "x = 1\n",
    );
    assert_eq!(out, "from typing import List, Optional\nimport os\nx = 1\n");
    assert_eq!(
        notes,
        vec![
            "carried `import os`".to_string(),
            "carried `from typing import List, Optional`".to_string()
        ]
    );

    let (out, notes) = carry(
        Go,
        "",
        "func f() { fmt.Println(strings.TrimSpace(x)) }",
        "package p\n",
    );
    assert_eq!(out, "package p\n\nimport \"strings\"\n\nimport \"fmt\"\n");
    assert_eq!(
        notes,
        vec![
            "carried import \"fmt\"".to_string(),
            "carried import \"strings\"".to_string()
        ]
    );
    let (out, notes) = carry(Go, "", "fmt.Println(1)", "package p\nimport \"fmt\"\n");
    assert_eq!(
        (out.as_str(), notes.len()),
        ("package p\nimport \"fmt\"\n", 0)
    );

    let (out, notes) = carry(
        Swift,
        "import Foundation\nimport UIKit\nlet x = 1\n",
        "",
        "import UIKit\nstruct S {}\n",
    );
    assert_eq!(out, "import Foundation\nimport UIKit\nstruct S {}\n");
    assert_eq!(notes, vec!["carried `import Foundation`".to_string()]);

    let (out, notes) = carry(
        Cpp,
        "#include <vector>\n#include \"a.h\"\nint x;",
        "",
        "int y;\n",
    );
    assert_eq!(out, "#include \"a.h\"\n#include <vector>\nint y;\n");
    assert_eq!(notes.len(), 2);
    let (out, _) = carry(C, "#include <stdio.h>\n", "", "#include <stdio.h>\n");
    assert_eq!(out, "#include <stdio.h>\n");

    let (out, notes) = carry(Java, "import java.util.List;\n", "", "class A {}\n");
    assert_eq!(out, "import java.util.List;\nclass A {}\n");
    assert_eq!(notes, vec!["carried `import java.util.List;`".to_string()]);

    let (out, notes) = carry(Rust, "use a::b;\n", "b()", "fn main() {}\n");
    assert_eq!((out.as_str(), notes.len()), ("fn main() {}\n", 0));
}

fn update(lang: Language, text: &str, name: &str) -> (String, Option<String>) {
    let ext = match lang {
        TypeScript => "ts",
        Python => "py",
        _ => "ext",
    };
    update_source_imports(
        text,
        name,
        Path::new(&format!("/w/src/a.{ext}")),
        Path::new(&format!("/w/src/b.{ext}")),
        Path::new("/w"),
        lang,
    )
    .unwrap()
}

#[test]
fn update_source_imports_by_language() {
    let (text, note) = update(TypeScript, "foo();\n", "foo");
    assert_eq!(text, "import { foo } from \"./b\";\nfoo();\n");
    assert_eq!(note.as_deref(), Some("imported `foo` from `./b`"));
    let (same, note) = update(TypeScript, "import { foo } from \"./b\";\nfoo();\n", "foo");
    assert_eq!(
        (same.as_str(), note),
        ("import { foo } from \"./b\";\nfoo();\n", None)
    );

    let (text, note) = update(Python, "foo()\n", "foo");
    assert_eq!(text, "from src.b import foo\nfoo()\n");
    assert_eq!(note.as_deref(), Some("from src.b import foo"));

    let (text, note) = update(Cpp, "int x = foo();\n", "foo");
    assert_eq!(text, "#include \"b.ext\"\nint x = foo();\n");
    assert_eq!(note.as_deref(), Some("included \"b.ext\""));
    let (same, note) = update(C, "#include \"b.ext\"\nfoo();\n", "foo");
    assert_eq!(
        (same.as_str(), note),
        ("#include \"b.ext\"\nfoo();\n", None)
    );

    let (text, note) = update(Swift, "foo()\n", "foo");
    assert_eq!(
        (text.as_str(), note.as_deref()),
        ("foo()\n", Some("same module: direct access"))
    );
    for lang in [Rust, Java] {
        assert_eq!(
            update(lang, "foo()\n", "foo"),
            ("foo()\n".to_string(), None)
        );
    }
    // a name the source never mentions leaves it untouched
    assert_eq!(
        update(Python, "x = 1\n", "foo"),
        ("x = 1\n".to_string(), None)
    );
}

#[test]
fn update_go_source_imports_same_and_cross_package() {
    let temp = tempfile::Builder::new().prefix("imp").tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("go.mod"), "module example.test/m\n").unwrap();
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::fs::create_dir_all(root.join("util")).unwrap();
    std::fs::write(root.join("lib/lib.go"), "package lib\n").unwrap();
    std::fs::write(root.join("lib/more.go"), "package lib\n").unwrap();
    std::fs::write(root.join("util/u.go"), "package util\n").unwrap();
    let text = "package lib\n\nfunc g() { Foo() }\n";

    let (same, note) = update_source_imports(
        text,
        "Foo",
        &root.join("lib/lib.go"),
        &root.join("lib/more.go"),
        root,
        Go,
    )
    .unwrap();
    assert_eq!(same, text);
    assert_eq!(note.as_deref(), Some("same package `lib`: direct access"));

    let (cross, note) = update_source_imports(
        text,
        "Foo",
        &root.join("lib/lib.go"),
        &root.join("util/u.go"),
        root,
        Go,
    )
    .unwrap();
    assert_eq!(
        cross,
        "package lib\n\nimport \"example.test/m/util\"\n\nfunc g() { util.Foo() }\n"
    );
    assert_eq!(note.as_deref(), Some("imported \"example.test/m/util\""));
}
