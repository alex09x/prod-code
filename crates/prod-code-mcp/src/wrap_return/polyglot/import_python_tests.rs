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
fn coverage_python_statement_and_module_resolution() {
    assert_eq!(split_semicolon_statements("x = 'a;b'; y = \"c;d\"; import value"), vec!["x = 'a;b'", " y = \"c;d\"", " import value"]);
    assert_eq!(python_simple_statements("# note\nfrom .value import \\\n    work as local\nimport (\n\n value as api\n)\n"), vec!["from .value import  work as local", "import ( value as api )"]);
    assert_eq!(python_simple_statements("import value \\"), vec!["import value"]);
    let caller = Path::new("pkg/sub/caller.py");
    for (module, decl, expected) in [
        ("", "pkg/value.py", false),
        ("..value", "pkg/value.py", true),
        ("..value", "pkg/value.pyi", true),
        ("..value", "pkg/value/__init__.py", true),
        ("..other", "pkg/value.py", false),
        ("pkg.value", "pkg/value", true),
        ("pkg.value", "root/pkg/value.py", true),
        ("pkg", "pkg/__init__.py", true),
        ("wrong", "pkg/value.py", false),
    ] { assert_eq!(py_mod_matches_decl(module, caller, Path::new(decl)), expected, "{module} {decl}"); }
    for (module, decl, expected) in [
        ("", "pkg/value.py", false),
        ("..", "pkg/value.py", true),
        ("..sub", "pkg/sub/value.py", true),
        ("..wrong", "pkg/sub/value.py", false),
        ("pkg", "pkg/value.py", true),
        ("pkg", "root/pkg/value.py", true),
        ("wrong", "pkg/value.py", false),
    ] { assert_eq!(py_mod_matches_pkg(module, caller, Path::new(decl)), expected); }
}

#[test]
fn coverage_python_binding_matrix() {
    let caller = Path::new("pkg/caller.py");
    let decl = Path::new("pkg/value.py");
    for (source, names, receiver, proven) in [
        ("from .value import *", vec!["work"], "api", false),
        ("from .value import (work as local, other)", vec!["local"], "local", false),
        ("from . import value as api", vec!["work"], "api", true),
        ("from . import value", vec!["work"], "value", true),
        ("import pkg.value as api", vec!["work"], "api", true),
        ("import pkg.value", vec!["work"], "pkg.value", true),
        ("import pkg.value", vec!["work"], "value", true),
        ("import pkg.other", vec![], "api", false),
        ("from .value import unknown", vec![], "unknown", false),
        ("from . import unknown", vec![], "unknown", false),
        ("from .value", vec![], "api", false),
        ("from unrelated import value as api", vec![], "api", false),
    ] {
        assert_eq!(python_imported_symbols(source, caller, decl, "work"), names, "{source}");
        assert_eq!(is_python_namespace_import(source, receiver, caller, decl), proven, "{source}");
    }
}
