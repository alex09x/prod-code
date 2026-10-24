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
use ImportMatch::{None, Proven, Uncertain};

#[test]
fn import_spellings_resolve_bindings_and_paths() {
    let mut mismatches = Vec::new();
    for (source, caller, decl, expected) in [
        ("", "/ws/app.ts", "/ws/value.ts", None),
        (
            "import { value } from './value.js';",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
        (
            "import{value}from'./value'",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
        (
            "import{value}from\"./value\"",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
        (
            "export{value}from './value'",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
        (
            "import type { value as renamed } from './value'",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
        (
            "import { type value } from './value'",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
        (
            "import { other as value } from './value'",
            "/ws/app.ts",
            "/ws/value.ts",
            None,
        ),
        (
            "import value, { other } from './value'",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
        (
            "import * as module from './value'",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
        (
            "const module = require('./value');",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
        (
            "const module = import('./value');",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
        (
            "import {\r\nvalue,\r\nother\r\n} from './value';",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
        (
            "from .value import (\nvalue,\n)",
            "/ws/app.py",
            "/ws/value.py",
            Proven,
        ),
        (
            "from ..value import value",
            "/ws/pkg/app.py",
            "/ws/value.py",
            Proven,
        ),
        (
            "from .value import other",
            "/ws/app.py",
            "/ws/value.py",
            None,
        ),
        ("from missing", "/ws/app.py", "/ws/value.py", None),
        (
            "pub use crate::value::{\n value,\n};",
            "/ws/app.rs",
            "/ws/value.rs",
            Uncertain,
        ),
        (
            "use crate::value::*;",
            "/ws/app.rs",
            "/ws/value.rs",
            Uncertain,
        ),
        (
            "use crate::value::other;",
            "/ws/app.rs",
            "/ws/value.rs",
            Uncertain,
        ),
        (
            "#include \"value.h\"",
            "/ws/app.cpp",
            "/ws/value.cpp",
            Uncertain,
        ),
        (
            "#include<value.hpp>",
            "/ws/app.cpp",
            "/ws/value.cpp",
            Uncertain,
        ),
        ("#include \"value.h\"", "/ws/app.cpp", "/ws/value.h", Proven),
        (
            "@testable import Package",
            "/ws/tests/a.swift",
            "/ws/Package/value.swift",
            Uncertain,
        ),
        (
            "import { value } from '@pkg/value'",
            "/ws/app.ts",
            "/ws/pkg/value.ts",
            None,
        ),
        (
            "import { value } from 'pkg/value'",
            "/ws/app.ts",
            "/ws/pkg/value.ts",
            Uncertain,
        ),
        (
            "import { value } from './other'",
            "/ws/app.ts",
            "/ws/value.ts",
            None,
        ),
        (
            "import { value } from './value.py'",
            "/ws/app.ts",
            "/ws/value.ts",
            None,
        ),
        (
            "import { value } from '.'",
            "/ws/app.ts",
            "/ws/index.ts",
            Proven,
        ),
        (
            "import { value } from './pkg/'",
            "/ws/app.ts",
            "/ws/pkg/index.ts",
            Proven,
        ),
        (
            "import { value } from 'index'",
            "/ws/app.ts",
            "/ws/pkg/index.ts",
            None,
        ),
        (
            "import { value } from '@'",
            "/ws/app.ts",
            "/ws/value.ts",
            None,
        ),
        (
            "import { value } from './雪'",
            "/ws/app.ts",
            "/ws/雪.ts",
            Proven,
        ),
        (
            "import { value } from 'value.ts'",
            "/ws/app.ts",
            "/ws/value.ts",
            Proven,
        ),
    ] {
        let actual = imports_declaration(source, Path::new(caller), Path::new(decl), "value");
        if actual != expected {
            mismatches.push(format!(
                "{source} -> {decl}: expected {expected:?}, got {actual:?}"
            ));
        }
    }
    assert_eq!(mismatches, Vec::<String>::new());
}

#[test]
fn competing_module_files_prevent_false_proof() {
    let ws = prod_code_testkit::Workspace::new(&[
        ("app.ts", ""),
        ("value.ts", ""),
        ("value.js", ""),
        ("pkg/index.ts", ""),
        ("pkg.ts", ""),
        ("python.py", ""),
        ("python/__init__.py", ""),
        ("rust/mod.rs", ""),
        ("rust.rs", ""),
    ]);
    for (spec, decl, expected) in [
        ("./value", "value.ts", Uncertain),
        ("./value.js", "value.ts", None),
        ("./value.ts", "value.ts", Proven),
        ("./pkg", "pkg/index.ts", None),
        ("./pkg/", "pkg/index.ts", Proven),
        ("./python", "python.py", None),
        ("./python", "python/__init__.py", Uncertain),
        ("./rust", "rust/mod.rs", Uncertain),
    ] {
        assert_eq!(
            imports_declaration(
                &format!("import {{ value }} from '{spec}'"),
                &ws.path("app.ts"),
                &ws.path(decl),
                "value"
            ),
            expected,
            "{spec} -> {decl}"
        );
    }
}

#[test]
fn extension_and_binding_boundaries_are_explicit() {
    for (name, expected) in [
        ("x.ts", ("x", Some("ts"))),
        (".ts", (".ts", Option::None)),
        ("a..ts", ("a..ts", Option::None)),
        ("a/.ts", ("a/.ts", Option::None)),
        ("a.TS", ("a.TS", Option::None)),
        ("x", ("x", Option::None)),
    ] {
        assert_eq!(split_known_ext(name), expected);
    }
    for (a, b, expected) in [
        ("TS", "ts", true),
        ("js", "tsx", true),
        ("jsx", "tsx", true),
        ("mjs", "mts", true),
        ("cjs", "cts", true),
        ("hpp", "cc", true),
        ("py", "ts", false),
    ] {
        assert_eq!(ext_compatible(a, b), expected);
    }
    for (clause, expected) in [
        ("export type { value as alias }", true),
        ("import value", true),
        ("{ type value }", true),
        ("{ other as value }", false),
        ("{value,}", true),
        ("{value other}", false),
        ("{", false),
        ("", false),
    ] {
        assert_eq!(clause_binds_symbol(clause, "value"), expected, "{clause}");
    }
    assert_eq!(normalize_segments(Path::new("/a/./b/../c")), vec!["a", "c"]);
    assert_eq!(
        imports_declaration("import value", Path::new("a"), Path::new("/"), "value"),
        None
    );
}
