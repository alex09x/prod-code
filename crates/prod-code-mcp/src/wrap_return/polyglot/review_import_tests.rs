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
use std::path::Path;

#[test]
fn python_sibling_absolute_import_does_not_prove_the_selected_module() {
    let source = "from external import db\ndb.find_user()\n";
    assert!(
        super::python_imported_symbols(
            source,
            Path::new("pkg/client.py"),
            Path::new("pkg/db.py"),
            "find_user",
        )
        .is_empty()
    );
}

#[test]
fn python_multiline_namespace_import_proves_the_local_alias() {
    let source = "from . import (\n    db as database,\n)\ndatabase.find_user()\n";
    assert!(super::is_proven_namespace_import(
        source,
        "database",
        Path::new("pkg/client.py"),
        Path::new("pkg/db.py"),
        Language::Python,
    ));
}

#[test]
fn typescript_multiline_reexport_proves_the_forwarded_symbol() {
    let root = tempfile::tempdir().unwrap();
    let selected = root.path().join("selected.ts");
    let barrel = root.path().join("index.ts");
    std::fs::write(&selected, "export function retry() {}\n").unwrap();
    std::fs::write(&barrel, "export {\n  retry,\n} from \"./selected\";\n").unwrap();

    let caller = root.path().join("caller.ts");
    let source = "import { retry } from \"./index\";\nretry();\n";
    assert_eq!(
        super::imported_caller_symbols(source, &caller, &selected, "retry", Language::TypeScript,),
        vec!["retry"]
    );
}

#[test]
fn typescript_reexport_waits_for_module_specifier_on_next_line() {
    let root = tempfile::tempdir().unwrap();
    let selected = root.path().join("selected.ts");
    let barrel = root.path().join("index.ts");
    std::fs::write(&selected, "export function retry() {}\n").unwrap();
    std::fs::write(&barrel, "export { retry } from\n  \"./selected\";\n").unwrap();

    let caller = root.path().join("caller.ts");
    let source = "import { retry } from \"./index\";\nretry();\n";
    assert_eq!(
        super::imported_caller_symbols(source, &caller, &selected, "retry", Language::TypeScript,),
        vec!["retry"]
    );
}

#[test]
fn typescript_reexport_keyword_is_not_taken_from_an_identifier() {
    let root = tempfile::tempdir().unwrap();
    let selected = root.path().join("codec.ts");
    let barrel = root.path().join("index.ts");
    std::fs::write(&selected, "export function fromJSON() {}\n").unwrap();
    std::fs::write(&barrel, "export { fromJSON } from \"./codec\";\n").unwrap();

    let caller = root.path().join("caller.ts");
    let source = "import { fromJSON } from \"./index\";\nfromJSON();\n";
    assert_eq!(
        super::imported_caller_symbols(
            source,
            &caller,
            &selected,
            "fromJSON",
            Language::TypeScript,
        ),
        vec!["fromJSON"]
    );
}

#[test]
fn typescript_named_default_reexport_proves_the_exported_name() {
    let root = tempfile::tempdir().unwrap();
    let selected = root.path().join("selected.ts");
    let barrel = root.path().join("index.ts");
    std::fs::write(&selected, "export default function retry() {}\n").unwrap();
    std::fs::write(
        &barrel,
        "export { default as retry } from \"./selected\";\n",
    )
    .unwrap();

    let caller = root.path().join("caller.ts");
    let source = "import { retry } from \"./index\";\nretry();\n";
    assert_eq!(
        super::imported_caller_symbols(source, &caller, &selected, "retry", Language::TypeScript,),
        vec!["retry"]
    );
}

#[test]
fn commonjs_callable_import_uses_the_local_binding() {
    let source = "const setup = 1\nconst again = require(\"./selected\").retry\nagain()\n";
    assert_eq!(
        super::imported_caller_symbols(
            source,
            Path::new("src/caller.js"),
            Path::new("src/selected.js"),
            "retry",
            Language::JavaScript,
        ),
        vec!["again"]
    );
}

#[test]
fn named_import_alias_does_not_prove_an_unrelated_member_receiver() {
    let source = "import { retry as again } from \"./selected\";\nother.again();\n";
    assert_eq!(
        super::imported_caller_symbols(
            source,
            Path::new("src/caller.ts"),
            Path::new("src/selected.ts"),
            "retry",
            Language::TypeScript,
        ),
        vec!["again"]
    );
    assert!(!super::is_proven_namespace_import(
        source,
        "other",
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        Language::TypeScript,
    ));
}

#[test]
fn go_cross_package_import_proves_its_qualified_call() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("go.mod"), "module example.com/app\n").unwrap();
    let decl = root.path().join("pkg/store/retry.go");
    let caller = root.path().join("cmd/main.go");
    std::fs::create_dir_all(decl.parent().unwrap()).unwrap();
    std::fs::create_dir_all(caller.parent().unwrap()).unwrap();
    std::fs::write(&decl, "package store\nfunc Retry() {}\n").unwrap();

    let source =
        "package main\nimport \"example.com/app/pkg/store\"\nfunc main() { store.Retry() }\n";
    assert!(
        super::imported_caller_symbols(source, &caller, &decl, "Retry", Language::Go,)
            .contains(&"Retry".to_string())
    );
    assert!(super::is_proven_namespace_import(
        source,
        "store",
        &caller,
        &decl,
        Language::Go,
    ));
}
