/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Import parsers of the module-level dependency graph, one table per language.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use super::languages::{
    parse_java_imports, parse_python_imports, parse_rust_imports, parse_ts_imports,
};

type Parser = fn(&str, &HashMap<String, PathBuf>, &mut BTreeSet<String>);

fn known(names: &[&str]) -> HashMap<String, PathBuf> {
    names
        .iter()
        .map(|n| (n.to_string(), PathBuf::from(format!("{n}.src"))))
        .collect()
}

fn run_table(parser: Parser, modules: &[&str], cases: &[(&str, &[&str])]) {
    let known = known(modules);
    for (content, expected) in cases {
        let mut deps = BTreeSet::new();
        parser(content, &known, &mut deps);
        let got: Vec<&str> = deps.iter().map(String::as_str).collect();
        let mut want = expected.to_vec();
        want.sort();
        assert_eq!(got, want, "content: {content:?}");
    }
}

#[test]
fn rust_imports_resolve_use_and_mod_declarations() {
    let modules = [
        "src::util",
        "src::net::mod",
        "src::net::client",
        "src::main",
    ];
    run_table(
        parse_rust_imports,
        &modules,
        &[
            ("use crate::util::helper;", &["src::util"]),
            (
                "use super::net::client;",
                &["src::net::client", "src::net::mod"],
            ),
            ("pub use crate::util;", &["src::util"]),
            ("    use crate::util as u;", &["src::util"]),
            ("use crate::util::{a, b};", &["src::util"]),
            ("mod util;", &["src::util"]),
            ("pub mod client;", &["src::net::client"]),
            ("pub(crate) mod net;", &["src::net::mod"]),
            ("pub(super)  mod util;", &["src::util"]),
            ("mod missing;", &[]),
            ("mod inline { fn f() {} }", &[]),
            ("use crate::{util, net};", &[]),
            ("use super::*;", &[]),
            ("use std::fmt;", &[]),
            ("public_thing(1);", &[]),
            ("pub(broken mod util;", &[]),
            ("", &[]),
        ],
    );
}

#[test]
fn python_imports_resolve_dotted_and_relative_names() {
    let modules = ["pkg::util", "pkg::sub::models", "app"];
    run_table(
        parse_python_imports,
        &modules,
        &[
            ("import pkg.util", &["pkg::util"]),
            ("import app", &["app"]),
            ("import app as a, os", &["app"]),
            ("from .util import f", &["pkg::util"]),
            ("from pkg.sub.models import M", &["pkg::sub::models"]),
            ("from ..sub.models import M", &["pkg::sub::models"]),
            // Bare relative imports name no module: they must not match everything.
            ("from . import x", &[]),
            ("from .. import y", &[]),
            ("import   app", &[]),
            ("import numpy", &[]),
            ("# import app", &[]),
            ("", &[]),
        ],
    );
}

#[test]
fn typescript_imports_resolve_relative_paths_with_either_quote() {
    let modules = ["src::api::client", "src::util"];
    run_table(
        parse_ts_imports,
        &modules,
        &[
            ("import { a } from \"./util\";", &["src::util"]),
            ("import a from '../api/client';", &["src::api::client"]),
            ("export * from './util'", &["src::util"]),
            ("import x from \"./util\"; // \"./client\"", &["src::util"]),
            ("import x from \"react\";", &[]),
            ("import \"./util\";", &[]),
            ("import x from \"./\";", &[]),
            ("import x from \"./missing\";", &[]),
            ("import x from './util", &[]),
            ("export const from = 1;", &[]),
            ("const util = require('./util');", &[]),
            ("", &[]),
        ],
    );
}

#[test]
fn java_imports_match_by_qualified_name_or_class_name() {
    let modules = [
        "src::main::java::com::acme::Store",
        "src::main::java::com::acme::util::Strings",
        "src::main::kotlin::Other",
    ];
    let store = "src::main::java::com::acme::Store";
    run_table(
        parse_java_imports,
        &modules,
        &[
            ("import com.acme.Store;", &[store]),
            ("import com.other.Store;", &[store]),
            ("import com.acme.Store", &[store]),
            (
                "import static com.acme.util.Strings;",
                &["src::main::java::com::acme::util::Strings"],
            ),
            (
                "import kotlin.Other\nimport java.util.List;",
                &["src::main::kotlin::Other"],
            ),
            ("import java.util.List;", &[]),
            ("import com.acme.*;", &[]),
            ("important = 1;", &[]),
            ("", &[]),
        ],
    );
}
