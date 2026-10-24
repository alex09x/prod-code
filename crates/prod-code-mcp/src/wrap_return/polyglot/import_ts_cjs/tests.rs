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
fn coverage_commonjs_binding_and_namespace_matrix() {
    let caller = Path::new("src/caller.js");
    let decl = Path::new("src/value.js");
    for (text, symbols, namespace) in [
        ("const api = require('./value');", vec!["api"], true),
        ("let api = require ( './value' );", vec!["api"], true),
        ("var alias = require('./value').work;", vec!["alias"], false),
        ("const alias = require('./value'). work;", vec!["alias"], false),
        ("const { work, other } = require('./value');", vec!["work"], false),
        ("const { work: local, other: ignored } = require('./value');", vec!["local"], false),
        ("const alias = require('./value').other;", vec![], false),
        ("const api = require('./other');", vec![], false),
        ("const api = prerequire('./value');", vec![], false),
        ("const api = requireExtra('./value');", vec![], false),
        ("const api = require './value';", vec![], false),
        ("const api = require('./value'", vec![], false),
        ("require('./value').work;", vec![], false),
        ("object.api = require('./value').work;", vec![], false),
        ("object.api = require('./value');", vec![], false),
        ("require('./value');", vec![], false),
        ("const { other } = require('./value');", vec![], false),
        ("// require('./value')\nconst api = require('./value');", vec!["api"], true),
        ("const s = \"require('./value')\";", vec![], false),
        ("", vec![], false),
    ] {
        let mask = crate::caller_migration::mask::lexical_code_mask(text, crate::parameter_object::Language::JavaScript);
        assert_eq!(ts_js_require_symbols(text, caller, decl, "work", &mask), symbols, "{text}");
        assert_eq!(is_ts_js_require_namespace(text, "api", caller, decl, &mask), namespace, "{text}");
        assert!(!is_ts_js_require_namespace(text, "missing", caller, decl, &mask), "{text}");
    }
    let text = "const api = require('./value');";
    assert_eq!(ts_js_require_symbols(text, caller, decl, "work", &[]), Vec::<String>::new());
    assert!(!is_ts_js_require_namespace(text, "api", caller, decl, &[]));
}
