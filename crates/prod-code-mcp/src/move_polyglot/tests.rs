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

use super::callers::rewrite_caller_imports;
use super::decl::find_polyglot_decl;
use super::execute::move_item;
use super::go_imports::{go_module_import_path, remove_unused_go_imports, rewrite_go_call_sites};
use super::imports::remove_from_braced_ts_import;
use super::specifiers::{python_module_specifier, relative_import_specifier};
use super::target::{cpp_move_target_is_implementation, format_item_for_target};

#[test]
fn unused_go_imports_are_removed_without_dropping_used_group_members() {
    let source = "package p\nimport \"fmt\"\nimport (\n\t\"os\"\n\t\"strings\"\n)\nfunc keep() { fmt.Println(strings.TrimSpace(\" x \")) }\n";
    let result = remove_unused_go_imports(source);

    assert!(!result.contains("import \"os\""));
    assert!(result.contains("\"fmt\""));
    assert!(result.contains("\"strings\""));
}

#[test]
fn fully_unused_go_import_group_is_removed() {
    let source = "package p\nimport (\n\t\"fmt\"\n\t\"os\"\n)\nfunc keep() {}\n";
    let result = remove_unused_go_imports(source);

    assert!(!result.contains("import ("));
    assert!(!result.contains("\"fmt\""));
    assert!(!result.contains("\"os\""));
}

#[test]
fn relative_import_specifiers_compute_accurately() {
    assert_eq!(
        relative_import_specifier(
            Path::new("src/features/foo.ts"),
            Path::new("src/common/utils.ts")
        ),
        "../common/utils"
    );
    assert_eq!(
        relative_import_specifier(Path::new("src/index.ts"), Path::new("src/utils.ts")),
        "./utils"
    );
    assert_eq!(
        relative_import_specifier(Path::new("index.ts"), Path::new("utils.ts")),
        "./utils"
    );
}

#[test]
fn default_import_follows_a_moved_default_export() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("src/source.ts");
    let target = temp.path().join("src/helpers.ts");
    let caller = temp.path().join("src/caller.ts");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "export default function Foo() {}\n").unwrap();

    let (rewritten, note) = rewrite_caller_imports(
        "import Foo from \"./source\";\nFoo();\n",
        "Foo",
        &caller,
        &source,
        &target,
        temp.path(),
        Language::TypeScript,
    )
    .unwrap();

    assert!(rewritten.contains("import Foo from \"./helpers\";"));
    assert!(!rewritten.contains("import { Foo }"));
    assert!(note.contains("default import"));
}

#[test]
fn python_module_specifiers_compute_accurately() {
    let root = Path::new("/workspace");
    assert_eq!(
        python_module_specifier(
            Path::new("/workspace/pkg/utils.py"),
            Path::new("/workspace/pkg/helpers.py"),
            root
        ),
        "pkg.helpers"
    );
}

#[test]
fn find_polyglot_decl_typescript_function() {
    let ts = "/**\n * Helper\n */\nexport function add(a: number, b: number): number {\n    return a + b;\n}\n";
    let (name, start, end) = find_polyglot_decl(ts, 4, Language::TypeScript).unwrap();
    assert_eq!(name, "add");
    assert_eq!(start, 1);
    assert_eq!(end, 6);
}

#[test]
fn find_polyglot_decl_python_function() {
    let py = "@deco\ndef helper(x):\n    return x * 2\n";
    let (name, start, end) = find_polyglot_decl(py, 2, Language::Python).unwrap();
    assert_eq!(name, "helper");
    assert_eq!(start, 1);
    assert_eq!(end, 3);
}

#[test]
fn find_polyglot_decl_go_function() {
    let go = "// Add doc\nfunc Add(a, b int) int {\n\treturn a + b\n}\n";
    let (name, start, end) = find_polyglot_decl(go, 2, Language::Go).unwrap();
    assert_eq!(name, "Add");
    assert_eq!(start, 1);
    assert_eq!(end, 4);
}

#[test]
fn format_item_adds_export_in_typescript() {
    let item = "function calculate(x: number) {\n    return x;\n}";
    let formatted = format_item_for_target(item, Language::TypeScript);
    assert!(formatted.starts_with("export function calculate"));
}

#[test]
fn remove_from_braced_ts_import_works() {
    let line = "import { A, B } from \"./foo\";";
    let (shrunk, removed) = remove_from_braced_ts_import(line, "A");
    assert!(removed);
    assert_eq!(shrunk, "import { B } from \"./foo\";");

    let line_single = "import { A } from \"./foo\";";
    let (shrunk2, removed2) = remove_from_braced_ts_import(line_single, "A");
    assert!(removed2);
    assert_eq!(shrunk2, "");
}

#[test]
fn go_call_rewriting_skips_comments_strings_and_identifier_substrings() {
    let source = "package p\nfunc use() {\n Add(1)\n _ = \"Add failed\"\n // Add remains a comment\n AddSuffix()\n}\n";
    let (rewritten, count) = rewrite_go_call_sites(source, "Add", "helpers").unwrap();

    assert_eq!(count, 1);
    assert!(rewritten.contains("helpers.Add(1)"));
    assert!(rewritten.contains("\"Add failed\""));
    assert!(rewritten.contains("// Add remains a comment"));
    assert!(rewritten.contains("AddSuffix()"));
}

#[test]
fn go_import_path_keeps_the_module_prefix() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("go.mod"), "module example.com/app\n").unwrap();
    let target = temp.path().join("internal/helpers");
    std::fs::create_dir_all(&target).unwrap();

    assert_eq!(
        go_module_import_path(&target).unwrap(),
        "example.com/app/internal/helpers"
    );
}

#[test]
fn cpp_moves_reject_implementation_file_targets() {
    assert!(cpp_move_target_is_implementation(
        Language::Cpp,
        Path::new("helpers.cpp")
    ));
    assert!(!cpp_move_target_is_implementation(
        Language::Cpp,
        Path::new("helpers.hpp")
    ));
    assert!(!cpp_move_target_is_implementation(
        Language::TypeScript,
        Path::new("helpers.cpp")
    ));
}

#[tokio::test]
async fn cpp_move_to_implementation_file_refuses_before_writing() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.cpp");
    let target = temp.path().join("helpers.cpp");
    let original = "double area(Shape s) { return s.area(); }\n";
    std::fs::write(&source, original).unwrap();

    let error = move_item(
        "127.0.0.1:9400".parse().unwrap(),
        temp.path(),
        &source,
        1,
        0,
        &target,
        true,
        true,
    )
    .await
    .unwrap_err();

    assert!(
        format!("{error:#}")
            .contains("cannot move a C/C++ declaration into an implementation file")
    );
    assert_eq!(std::fs::read_to_string(source).unwrap(), original);
    assert!(!target.exists());
}

#[test]
fn go_move_to_new_file_initializes_package_header() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    let target = pkg_dir.join("b.go");

    std::fs::write(pkg_dir.join("a.go"), "package pkg\n\nfunc A() {}\n").unwrap();

    let header = super::target::initial_file_header(&target, Language::Go);
    assert_eq!(header, "package pkg\n\n");
}

#[test]
fn go_move_carries_imports_after_initial_package_header() {
    let temp = tempfile::tempdir().unwrap();
    let pkg_dir = temp.path().join("pkg");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    let source = pkg_dir.join("a.go");
    let target = pkg_dir.join("b.go");
    std::fs::write(
        &source,
        "package pkg\n\nimport \"fmt\"\n\nfunc Helper() { fmt.Println(\"hi\") }\n",
    )
    .unwrap();

    let target_header = super::target::initial_file_header(&target, Language::Go);
    let item = "func Helper() { fmt.Println(\"hi\") }\n";
    let (target_with_carried, _notes) = super::imports::carry_imports_polyglot(
        "package pkg\n\nimport \"fmt\"\n\nfunc Helper() { fmt.Println(\"hi\") }\n",
        item,
        &target_header,
        &source,
        &target,
        Language::Go,
        temp.path(),
    );
    assert!(target_with_carried.starts_with("package pkg\n\nimport \"fmt\"\n"));
    let target_new = crate::move_item::append_item(&target_with_carried, item);
    assert_eq!(
        target_new,
        "package pkg\n\nimport \"fmt\"\n\nfunc Helper() { fmt.Println(\"hi\") }\n"
    );
}
